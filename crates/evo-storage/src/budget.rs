//! Persistent root monetary budget and per-call accounting.
//!
//! Monetary values are integer micros in the root's authorized currency/pricing
//! version. No floating-point conversion occurs here.

use super::lifecycle::{BudgetRequestSources, budget_request_source_ids};
use super::{MVP_MAX_ACTIVE_LEASES, Session, Store, count_active_leases, internal};
use crate::typed_budget::{self, E16CallMode};
use evo_core::capacity::CapacityItem;
use evo_core::evaluation::{OptimizationStage, RootStageBudgetPlanV1, money_micros};
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier, text};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Acquire, Row, Sqlite, Transaction};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStage {
    Reflection,
    Merge,
    Ranking,
    DevelopmentExecution,
    DevelopmentScoring,
    Practice,
    Consolidation,
    Guidance,
    TaskExecution,
    CandidateGeneration,
    FormalEvaluation,
    Curriculum,
    MetaEvaluation,
    HistoryCollection,
    StorageCpu,
    GrayOperations,
    HumanReview,
}

impl BudgetStage {
    pub const ALL: [Self; 17] = [
        Self::Reflection,
        Self::Merge,
        Self::Ranking,
        Self::DevelopmentExecution,
        Self::DevelopmentScoring,
        Self::Practice,
        Self::Consolidation,
        Self::Guidance,
        Self::TaskExecution,
        Self::CandidateGeneration,
        Self::FormalEvaluation,
        Self::Curriculum,
        Self::MetaEvaluation,
        Self::HistoryCollection,
        Self::StorageCpu,
        Self::GrayOperations,
        Self::HumanReview,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reflection => "reflection",
            Self::Merge => "merge",
            Self::Ranking => "ranking",
            Self::DevelopmentExecution => "development_execution",
            Self::DevelopmentScoring => "development_scoring",
            Self::Practice => "practice",
            Self::Consolidation => "consolidation",
            Self::Guidance => "guidance",
            Self::TaskExecution => "task_execution",
            Self::CandidateGeneration => "candidate_generation",
            Self::FormalEvaluation => "formal_evaluation",
            Self::Curriculum => "curriculum",
            Self::MetaEvaluation => "meta_evaluation",
            Self::HistoryCollection => "history_collection",
            Self::StorageCpu => "storage_cpu",
            Self::GrayOperations => "gray_operations",
            Self::HumanReview => "human_review",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "reflection" => Ok(Self::Reflection),
            "merge" => Ok(Self::Merge),
            "ranking" => Ok(Self::Ranking),
            "development_execution" => Ok(Self::DevelopmentExecution),
            "development_scoring" => Ok(Self::DevelopmentScoring),
            "practice" => Ok(Self::Practice),
            "consolidation" => Ok(Self::Consolidation),
            "guidance" => Ok(Self::Guidance),
            "task_execution" => Ok(Self::TaskExecution),
            "candidate_generation" => Ok(Self::CandidateGeneration),
            "formal_evaluation" => Ok(Self::FormalEvaluation),
            "curriculum" => Ok(Self::Curriculum),
            "meta_evaluation" => Ok(Self::MetaEvaluation),
            "history_collection" => Ok(Self::HistoryCollection),
            "storage_cpu" => Ok(Self::StorageCpu),
            "gray_operations" => Ok(Self::GrayOperations),
            "human_review" => Ok(Self::HumanReview),
            _ => Err(Error::Internal),
        }
    }
}

impl From<OptimizationStage> for BudgetStage {
    fn from(value: OptimizationStage) -> Self {
        match value {
            OptimizationStage::HistoryCollection => Self::HistoryCollection,
            OptimizationStage::Reflection => Self::Reflection,
            OptimizationStage::Merge => Self::Merge,
            OptimizationStage::Ranking => Self::Ranking,
            OptimizationStage::DevelopmentExecution => Self::DevelopmentExecution,
            OptimizationStage::DevelopmentScoring => Self::DevelopmentScoring,
            OptimizationStage::Practice => Self::Practice,
            OptimizationStage::Consolidation => Self::Consolidation,
            OptimizationStage::Guidance => Self::Guidance,
            OptimizationStage::HumanReview => Self::HumanReview,
            OptimizationStage::StorageCpu => Self::StorageCpu,
        }
    }
}

/// Determinate refusals made by the reservation/dispatch gates. These describe
/// the attempted operation, never prove ownership of a call or authorize a refund.
/// Storage failures and stale fences remain in the outer `Result`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetPreDispatchRefusal {
    RootStopped,
    GroupStopped,
    LeaseExpired,
    SourceRevoked,
    RootConcurrencyLimit,
    CallIdReused,
    RootAuthorizationMissing,
    PerCallCapExceeded,
    RootBudgetExhausted,
    BudgetArithmeticOverflow,
    ActiveLeaseCapacity {
        used: u64,
        limit: u64,
    },
    Unauthorized,
    CallNamespaceMismatch,
    GroupNamespaceMismatch,
    InvalidRequest {
        message: String,
    },
    InvalidArtifactDigest,
    DuplicateSource,
    SourceClosureChanged,
    /// A bound root's stage plan refused the call before any business write.
    StagePlan {
        stage: BudgetStage,
        kind: StagePlanRefusalKind,
    },
}

/// The determinate reasons a bound root's stage plan can refuse a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagePlanRefusalKind {
    Unknown,
    Inactive,
    Unallocated,
    PerCallExceeded,
    Exhausted,
    /// An acceptance stage (`task_execution`/`formal_evaluation`) on a bound
    /// root without the engine-verified E05 ticket witness.
    E05Required,
}

impl StagePlanRefusalKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unknown => "stage_plan_unknown",
            Self::Inactive => "stage_plan_inactive",
            Self::Unallocated => "stage_plan_unallocated",
            Self::PerCallExceeded => "stage_plan_per_call_exceeded",
            Self::Exhausted => "stage_plan_exhausted",
            Self::E05Required => "stage_plan_e05_ticket_required",
        }
    }
}

/// `Ok(Err(..))` is a known refusal; `Err(..)` is not evidence of no dispatch.
pub type BudgetPreDispatchResult<T> = Result<std::result::Result<T, BudgetPreDispatchRefusal>>;

impl BudgetPreDispatchRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RootStopped => "root_stopped",
            Self::GroupStopped => "group_stopped",
            Self::LeaseExpired => "lease_expired",
            Self::SourceRevoked => "source_revoked",
            Self::RootConcurrencyLimit => "root_concurrency_limit",
            Self::CallIdReused => "call_id_reused",
            Self::RootAuthorizationMissing => "root_authorization_missing",
            Self::PerCallCapExceeded => "per_call_cap_exceeded",
            Self::RootBudgetExhausted => "root_budget_exhausted",
            Self::BudgetArithmeticOverflow => "budget_arithmetic_overflow",
            Self::ActiveLeaseCapacity { .. } => "active_lease_capacity",
            Self::Unauthorized | Self::CallNamespaceMismatch => "unauthorized",
            Self::GroupNamespaceMismatch => "group_namespace_mismatch",
            Self::InvalidRequest { .. } | Self::InvalidArtifactDigest => "invalid_request",
            Self::DuplicateSource => "duplicate_source",
            Self::SourceClosureChanged => "source_closure_changed",
            Self::StagePlan { kind, .. } => kind.code(),
        }
    }

    pub fn is_stage_plan(&self) -> bool {
        matches!(self, Self::StagePlan { .. })
    }

    /// Exact historical reservation error, including validation/capacity text.
    pub fn into_reservation_error(self) -> Error {
        match self {
            Self::SourceRevoked => Error::Forbidden,
            other => other.into_dispatch_error(),
        }
    }

    /// Exact historical begin error. A revoked source was `Cancelled` here.
    pub fn into_dispatch_error(self) -> Error {
        match self {
            Self::RootStopped | Self::GroupStopped | Self::LeaseExpired | Self::SourceRevoked => {
                Error::Cancelled
            }
            Self::RootConcurrencyLimit => Error::Conflict("root_budget_concurrency_limit".into()),
            Self::CallIdReused => Error::Conflict("call_id_reused_with_different_content".into()),
            Self::RootAuthorizationMissing
            | Self::PerCallCapExceeded
            | Self::RootBudgetExhausted
            | Self::BudgetArithmeticOverflow
            | Self::StagePlan { .. } => Error::Budget,
            Self::ActiveLeaseCapacity { used, limit } => {
                Error::Capacity(evo_core::capacity::CapacityConflict {
                    item: evo_core::capacity::CapacityItem::ActiveLease,
                    used,
                    limit,
                    delta: 1,
                })
            }
            Self::Unauthorized => Error::Forbidden,
            Self::CallNamespaceMismatch => Error::NotFound,
            Self::GroupNamespaceMismatch => {
                Error::Conflict("dispatch_group_owned_by_different_namespace".into())
            }
            Self::InvalidRequest { message } => Error::Invalid(message),
            Self::InvalidArtifactDigest => Error::Conflict("artifact digest mismatch".into()),
            Self::DuplicateSource => Error::Conflict("duplicate budget call source id".into()),
            Self::SourceClosureChanged => {
                Error::Conflict("budget call source closure changed for fixed call".into())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetCallState {
    Reserved,
    Dispatched,
    Uncertain,
    Finalized,
    Released,
    Cancelled,
}

impl BudgetCallState {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "dispatched" => Ok(Self::Dispatched),
            "uncertain" => Ok(Self::Uncertain),
            "finalized" => Ok(Self::Finalized),
            "released" => Ok(Self::Released),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(Error::Internal),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootBudgetAuthorization {
    pub root_budget_id: String,
    pub billing_scope: String,
    pub allowed_namespaces: Vec<String>,
    pub currency: String,
    pub pricing_version: String,
    pub payment_subject: String,
    pub authorization_receipt_digest: String,
    pub per_call_cap_micros: i64,
    pub total_limit_micros: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudgetRecord {
    pub root_budget_id: String,
    pub billing_scope: String,
    pub authorizing_namespace: String,
    pub allowed_namespaces: Vec<String>,
    pub currency: String,
    pub pricing_version: String,
    pub payment_subject: String,
    pub authorization_receipt_digest: String,
    pub per_call_cap_micros: i64,
    pub total_limit_micros: i64,
    pub spent_micros: i64,
    pub reserved_micros: i64,
    pub stopped: bool,
    pub stop_reason: Option<String>,
    pub stop_committed_at: Option<i64>,
    pub created_at: i64,
}

pub const ROOT_STAGE_PLAN_REPORT_SCHEMA: &str = "rsia.root_stage_plan_report.v1";

/// One append-only root stage plan version. `seq` is monotone per billing
/// scope; the greatest `seq` is the only active version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootStagePlanBinding {
    pub billing_scope: String,
    pub seq: i64,
    pub plan_digest: String,
    pub plan: RootStageBudgetPlanV1,
    pub payer_id: String,
    pub root_total_micros: i64,
    pub currency: String,
    pub pricing_version: String,
    pub authorization_receipt_digest: String,
    pub authorizing_namespace: String,
    pub bound_by_actor: String,
    pub created_at: i64,
}

/// A trusted Admin bind request. The persisted `created_at` is always the
/// same-Session trusted clock (`evo_core::now()`); the request carries no
/// caller-supplied time.
#[derive(Debug, Clone)]
pub struct RootStagePlanBindRequest {
    pub billing_scope: String,
    pub expected_seq: i64,
    pub plan: RootStageBudgetPlanV1,
}

/// The immutable control-to-active-plan relation written in the control's own
/// registration Session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootStagePlanControlRef {
    pub billing_scope: String,
    pub namespace: String,
    pub control_kind: String,
    pub control_id: String,
    pub control_digest: String,
    pub binding_seq: i64,
    pub plan_digest: String,
    pub created_at: i64,
}

/// The relation recording that one reserved call was admitted under one
/// binding version. It never authorizes a refund or reinterprets history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootStagePlanCallRef {
    pub billing_scope: String,
    pub call_id: String,
    pub namespace: String,
    pub stage: BudgetStage,
    pub binding_seq: i64,
    pub plan_digest: String,
    pub course_producer_ref: Option<String>,
    pub created_at: i64,
}

/// The actual persisted control a caller names as its plan origin. The storage
/// gate reloads the relation; this selector carries no authority by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagePlanControlSelector {
    pub control_kind: String,
    pub namespace: String,
    pub control_id: String,
}

/// The persisted ticket an engine already verified in this Session. Storage
/// re-reads the ticket object before it lets an acceptance stage pass a bound
/// root. It is an internal trusted-call witness, not an external input schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct E05TicketWitness {
    pub ticket_kind: String,
    pub ticket_id: String,
    pub ticket_storage_id: String,
    pub ticket_digest: String,
}

/// What a reservation claims about its plan origin. `Unattributed` is not
/// evidence of `Outside`: on a bound root it is refused as Unknown. Acceptance
/// stages require the engine-verified `E05Ticket` witness instead.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StagePlanAttribution {
    #[default]
    Unattributed,
    Control(StagePlanControlSelector),
    E05Ticket(E05TicketWitness),
}

/// The single refusal fact an internal trusted refusal path may append to a
/// still-clean Session. It writes no group, call, money or other caller write;
/// `event_at` is the same-Session trusted clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagePlanRefusal {
    pub billing_scope: String,
    pub stage: BudgetStage,
    pub kind: StagePlanRefusalKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootStagePlanEnforcement {
    Enforced,
    NoBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootStagePlanStageAllocation {
    pub stage: BudgetStage,
    pub per_call_limit_micros: Option<i64>,
    pub allocated_micros: Option<i64>,
    pub occupied_micros: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootStagePlanRefusalCount {
    pub stage: BudgetStage,
    pub reason: String,
    pub count: u64,
}

/// A separate report DTO: the original `RootBudgetMonetarySnapshot` wire and
/// digest are unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootStagePlanReport {
    pub schema_version: String,
    pub billing_scope: String,
    pub enforcement: RootStagePlanEnforcement,
    pub binding: Option<RootStagePlanBinding>,
    pub stages: Vec<RootStagePlanStageAllocation>,
    pub refusals: Vec<RootStagePlanRefusalCount>,
    pub course_share_bps: u16,
    pub course_occupied_micros: i64,
    pub course_producer_wired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetCallReservation {
    pub billing_scope: String,
    pub call_id: String,
    pub dispatch_group_id: String,
    pub stage: BudgetStage,
    pub actual_input_digest: String,
    pub request_artifact: Option<BudgetArtifact>,
    pub max_cost_micros: i64,
    pub lease_token: String,
    pub lease_until: i64,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetCallFence {
    pub billing_scope: String,
    pub call_id: String,
    pub actual_input_digest: String,
    pub lease_token: String,
    pub lease_epoch: i64,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetCallRefence {
    pub billing_scope: String,
    pub call_id: String,
    pub expected_epoch: i64,
    pub new_lease_token: String,
    pub new_lease_until: i64,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageCharge {
    pub amount_micros: i64,
    pub currency: String,
    pub pricing_version: String,
    pub provider_request_id: String,
    pub usage_record_id: String,
    pub output_digest: String,
}

/// Non-persistent classification of one reconcile transaction. The overflow
/// variant still carries the post-write call record: the charge-preservation
/// writes are committed by the caller and never rolled back or zeroed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetReconcileOutcome {
    Applied(BudgetCallRecord),
    AlreadyFinalized(BudgetCallRecord),
    PreservedAccountingOverflow(BudgetCallRecord),
}

/// The safe post-reconciliation call projection. It proves the actual stored
/// state and amounts without exposing any request/response/transport body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetCallSettlementView {
    pub billing_scope: String,
    pub call_id: String,
    pub state: BudgetCallState,
    pub dispatch_id: Option<String>,
    pub provider_request_id: Option<String>,
    pub usage_record_id: Option<String>,
    pub output_digest: Option<String>,
    pub reserved_micros: i64,
    pub actual_cost_micros: Option<i64>,
    pub actual_currency: Option<String>,
    pub actual_pricing_version: Option<String>,
    pub execution_closed: bool,
    pub execution_closed_at: Option<i64>,
    pub execution_close_reason: Option<String>,
    pub terminal_reason: Option<String>,
    pub finalized_at: Option<i64>,
    pub root_stopped: bool,
    pub root_stop_reason: Option<String>,
}

impl BudgetCallSettlementView {
    pub fn new(call: &BudgetCallRecord, root: &RootBudgetRecord) -> Self {
        Self {
            billing_scope: call.billing_scope.clone(),
            call_id: call.call_id.clone(),
            state: call.state,
            dispatch_id: call.dispatch_id.clone(),
            provider_request_id: call.provider_request_id.clone(),
            usage_record_id: call.usage_record_id.clone(),
            output_digest: call.output_digest.clone(),
            reserved_micros: call.reserved_micros,
            actual_cost_micros: call.actual_cost_micros,
            actual_currency: call.actual_currency.clone(),
            actual_pricing_version: call.actual_pricing_version.clone(),
            execution_closed: call.execution_closed,
            execution_closed_at: call.execution_closed_at,
            execution_close_reason: call.execution_close_reason.clone(),
            terminal_reason: call.terminal_reason.clone(),
            finalized_at: call.finalized_at,
            root_stopped: root.stopped,
            root_stop_reason: root.stop_reason.clone(),
        }
    }
}

pub const BUDGET_OPEN_CALLS_PAGE_SCHEMA: &str = "rsia.budget_open_calls_page.v1";
pub const BUDGET_LIST_OPEN_MAX_PAGE_SIZE: u16 = 100;

/// One open call in the safe management projection. It carries identifiers,
/// stage/state, dispatch and money metadata only; never the private request,
/// response or transport artifact body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetOpenCallView {
    pub call_id: String,
    pub namespace: String,
    pub stage: BudgetStage,
    pub state: BudgetCallState,
    pub dispatch_group_id: String,
    pub dispatch_id: Option<String>,
    pub provider_request_id: Option<String>,
    pub usage_record_id: Option<String>,
    pub output_digest: Option<String>,
    pub reserved_micros: i64,
    pub actual_cost_micros: Option<i64>,
    pub actual_currency: Option<String>,
    pub actual_pricing_version: Option<String>,
    pub execution_closed: bool,
    pub execution_open: bool,
    pub terminal_reason: Option<String>,
    pub created_at: i64,
    pub dispatched_at: Option<i64>,
    pub finalized_at: Option<i64>,
}

/// One bounded page of the complete open-call view. `anchor` is the digest of
/// the whole projected view; a cursor carries it so a changed view is refused
/// instead of silently splicing two snapshots. `complete` is true only when
/// the scan proved there is no further open call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetOpenCallsPage {
    pub schema_version: String,
    pub billing_scope: String,
    pub anchor: String,
    pub page_size: u16,
    pub entries: Vec<BudgetOpenCallView>,
    pub next_cursor: Option<String>,
    pub complete: bool,
}

/// A non-authoritative continuation position: the caller must still be an
/// Admin member of the root on every page, and the anchor must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetListOpenCursor {
    pub anchor: String,
    pub after_call_id: String,
}

pub fn parse_budget_list_open_cursor(value: &str) -> Result<BudgetListOpenCursor> {
    let mut parts = value.splitn(3, ':');
    if parts.next() != Some("v1") {
        return Err(Error::Invalid("invalid budget list cursor".into()));
    }
    let anchor = parts
        .next()
        .ok_or_else(|| Error::Invalid("invalid budget list cursor".into()))?;
    validate_digest(anchor, "budget list cursor anchor")?;
    let after_call_id = parts
        .next()
        .ok_or_else(|| Error::Invalid("invalid budget list cursor".into()))?;
    identifier(after_call_id)?;
    Ok(BudgetListOpenCursor {
        anchor: anchor.into(),
        after_call_id: after_call_id.into(),
    })
}

fn open_call_view(call: &BudgetCallRecord) -> Option<BudgetOpenCallView> {
    let open = match call.state {
        BudgetCallState::Reserved | BudgetCallState::Dispatched | BudgetCallState::Uncertain => {
            true
        }
        BudgetCallState::Finalized => !call.execution_closed,
        BudgetCallState::Released | BudgetCallState::Cancelled => false,
    };
    open.then(|| BudgetOpenCallView {
        call_id: call.call_id.clone(),
        namespace: call.namespace.clone(),
        stage: call.stage,
        state: call.state,
        dispatch_group_id: call.dispatch_group_id.clone(),
        dispatch_id: call.dispatch_id.clone(),
        provider_request_id: call.provider_request_id.clone(),
        usage_record_id: call.usage_record_id.clone(),
        output_digest: call.output_digest.clone(),
        reserved_micros: call.reserved_micros,
        // Unknown amounts stay None; they are never projected as zero.
        actual_cost_micros: call.actual_cost_micros,
        actual_currency: call.actual_currency.clone(),
        actual_pricing_version: call.actual_pricing_version.clone(),
        execution_closed: call.execution_closed,
        execution_open: !call.execution_closed,
        terminal_reason: call.terminal_reason.clone(),
        created_at: call.created_at,
        dispatched_at: call.dispatched_at,
        finalized_at: call.finalized_at,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetArtifact {
    pub schema_version: String,
    pub digest: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetCallRef {
    pub id: String,
    pub schema_version: String,
    pub namespace: String,
    pub billing_scope: String,
    pub call_id: String,
    pub source_ids: Vec<String>,
}

impl BudgetArtifact {
    pub fn from_serializable<T: Serialize>(
        schema_version: impl Into<String>,
        value: &T,
    ) -> Result<Self> {
        let body = serde_json::to_string(value).map_err(internal)?;
        let artifact = Self {
            schema_version: schema_version.into(),
            digest: hash(body.as_bytes()),
            body,
        };
        artifact.validate()?;
        Ok(artifact)
    }

    fn validate(&self) -> Result<()> {
        self.validate_typed()?
            .map_err(BudgetPreDispatchRefusal::into_reservation_error)
    }

    fn validate_typed(&self) -> BudgetPreDispatchResult<()> {
        if let Err(error) = identifier(&self.schema_version)
            .and_then(|()| validate_digest(&self.digest, "artifact digest"))
        {
            return reservation_validation_refusal(error);
        }
        if self.body.len() > 4 * 1024 * 1024 {
            return Ok(Err(BudgetPreDispatchRefusal::InvalidRequest {
                message: "budget artifact exceeds 4 MiB".into(),
            }));
        }
        if hash(self.body.as_bytes()) != self.digest {
            return Ok(Err(BudgetPreDispatchRefusal::InvalidArtifactDigest));
        }
        serde_json::from_str::<serde_json::Value>(&self.body).map_err(internal)?;
        Ok(Ok(()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetExecutionProvenance {
    Fixture,
    ExternalProvider,
}

impl BudgetExecutionProvenance {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fixture => "fixture",
            Self::ExternalProvider => "external_provider",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "fixture" => Ok(Self::Fixture),
            "external_provider" => Ok(Self::ExternalProvider),
            _ => Err(Error::Internal),
        }
    }
}

/// Request artifact schema of a registered in-process execution row. Its body
/// carries a `source_ids` closure so source revocation can find the row.
pub const REGISTERED_EXECUTION_REQUEST_SCHEMA: &str = "rsia.registered_execution_request.v1";
pub const REGISTERED_EXECUTION_SETTLEMENT_SCHEMA: &str = "rsia.registered_execution_settlement.v1";
pub const REGISTERED_EXECUTION_SETTLEMENT_SCHEMA_V2: &str =
    "rsia.registered_execution_settlement.v2";

/// Provenance of an in-process registered execution. It is recorded in the
/// row's typed settlement artifact because `execution_provenance` is pinned
/// by the 0005 schema to model provenances; it is never a fixture and never a
/// sandbox claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisteredExecutionProvenance {
    RegisteredPureFunction,
}

/// The unchanged v1 submission payload and legacy accounting/audit view.
/// New settlements wrap this data with the actual writer's identity in v2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredExecutionSettlement {
    pub schema_version: String,
    pub provenance: RegisteredExecutionProvenance,
    pub call_id: String,
    pub dispatch_id: String,
    pub request_digest: String,
    pub output_digest: String,
    pub target_id: String,
    pub target_digest: String,
    pub runner_digest: String,
}

impl RegisteredExecutionSettlement {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != REGISTERED_EXECUTION_SETTLEMENT_SCHEMA {
            return Err(Error::Invalid(
                "unsupported registered execution settlement schema".into(),
            ));
        }
        identifier(&self.call_id)?;
        identifier(&self.dispatch_id)?;
        identifier(&self.target_id)?;
        validate_digest(&self.request_digest, "settlement request_digest")?;
        validate_digest(&self.output_digest, "settlement output_digest")?;
        validate_digest(&self.target_digest, "settlement target_digest")?;
        validate_digest(&self.runner_digest, "settlement runner_digest")?;
        Ok(())
    }

    pub fn artifact(&self) -> Result<BudgetArtifact> {
        self.validate()?;
        BudgetArtifact::from_serializable(REGISTERED_EXECUTION_SETTLEMENT_SCHEMA, self)
    }

    /// Reloads the settlement from a persisted row and rechecks that the row's
    /// own columns agree with it. A row without this artifact is not a
    /// registered execution.
    pub fn from_call(call: &BudgetCallRecord) -> Result<Self> {
        let artifact = call.response_artifact.as_ref().ok_or_else(|| {
            Error::Conflict("budget call carries no registered execution settlement".into())
        })?;
        if artifact.schema_version != REGISTERED_EXECUTION_SETTLEMENT_SCHEMA {
            return Err(Error::Conflict(
                "budget call response artifact is not a registered execution settlement".into(),
            ));
        }
        let settlement: Self = serde_json::from_str(&artifact.body)
            .map_err(|_| Error::Conflict("registered execution settlement is invalid".into()))?;
        settlement.validate()?;
        settlement.validate_call_binding(call)?;
        Ok(settlement)
    }

    fn validate_call_binding(&self, call: &BudgetCallRecord) -> Result<()> {
        if self.call_id != call.call_id
            || call.dispatch_id.as_deref() != Some(self.dispatch_id.as_str())
            || call.actual_input_digest != self.request_digest
            || call.output_digest.as_deref() != Some(self.output_digest.as_str())
            || call.execution_provenance == Some(BudgetExecutionProvenance::Fixture)
        {
            return Err(Error::Conflict(
                "registered execution settlement differs from its budget row".into(),
            ));
        }
        Ok(())
    }
}

/// Actual facts written by the successful registered-settlement transaction.
/// This read DTO never grants authority to a caller-supplied v2 body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredExecutionSettlementV2 {
    pub schema_version: String,
    pub settler_actor: String,
    pub settler_role: Role,
    pub settlement: RegisteredExecutionSettlement,
}

/// Static version table. A valid v1 remains readable, but carries no settler
/// witness and cannot be promoted to a current execution fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadRegisteredExecutionSettlement {
    Legacy(RegisteredExecutionSettlement),
    Current(RegisteredExecutionSettlementV2),
}

impl ReadRegisteredExecutionSettlement {
    pub fn from_call(call: &BudgetCallRecord) -> Result<Self> {
        let artifact = call.response_artifact.as_ref().ok_or_else(|| {
            Error::Conflict("budget call carries no registered execution settlement".into())
        })?;
        match artifact.schema_version.as_str() {
            REGISTERED_EXECUTION_SETTLEMENT_SCHEMA => {
                RegisteredExecutionSettlement::from_call(call).map(Self::Legacy)
            }
            REGISTERED_EXECUTION_SETTLEMENT_SCHEMA_V2 => {
                let value: RegisteredExecutionSettlementV2 = serde_json::from_str(&artifact.body)
                    .map_err(|_| {
                    Error::Conflict("registered execution settlement is invalid".into())
                })?;
                if value.schema_version != REGISTERED_EXECUTION_SETTLEMENT_SCHEMA_V2
                    || call.execution_provenance.is_some()
                    || identifier(&value.settler_actor).is_err()
                    || !matches!(value.settler_role, Role::Worker | Role::Host)
                {
                    return Err(Error::Conflict(
                        "registered execution settlement is invalid".into(),
                    ));
                }
                value.settlement.validate()?;
                value.settlement.validate_call_binding(call)?;
                Ok(Self::Current(value))
            }
            _ => Err(Error::Conflict(
                "budget call response artifact is not a registered execution settlement".into(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCallSettlementEvidence {
    pub provenance: BudgetExecutionProvenance,
    pub actual_model_digest: Option<String>,
    pub transport_artifact: BudgetArtifact,
    pub usable_response: Option<BudgetArtifact>,
    pub blocked_response: BudgetArtifact,
    pub forced_block_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCallSettlement {
    pub call: BudgetCallRecord,
    pub response_artifact: BudgetArtifact,
    pub response_usable: bool,
    pub response_block_reason: Option<String>,
    pub accounting_overflow: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetCallRecord {
    pub billing_scope: String,
    pub call_id: String,
    pub dispatch_group_id: String,
    pub namespace: String,
    pub stage: BudgetStage,
    pub actual_input_digest: String,
    pub request_artifact: Option<BudgetArtifact>,
    pub reserved_micros: i64,
    pub state: BudgetCallState,
    pub lease_token: String,
    pub lease_epoch: i64,
    pub lease_until: i64,
    pub dispatch_id: Option<String>,
    pub provider_request_id: Option<String>,
    pub usage_record_id: Option<String>,
    pub output_digest: Option<String>,
    pub actual_cost_micros: Option<i64>,
    pub actual_currency: Option<String>,
    pub actual_pricing_version: Option<String>,
    pub execution_provenance: Option<BudgetExecutionProvenance>,
    pub actual_model_digest: Option<String>,
    pub transport_artifact: Option<BudgetArtifact>,
    pub response_artifact: Option<BudgetArtifact>,
    pub response_usable: Option<bool>,
    pub response_block_reason: Option<String>,
    pub execution_closed: bool,
    pub execution_closed_at: Option<i64>,
    pub execution_close_reason: Option<String>,
    pub terminal_reason: Option<String>,
    pub created_at: i64,
    pub dispatched_at: Option<i64>,
    pub finalized_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchGroupRecord {
    pub billing_scope: String,
    pub dispatch_group_id: String,
    pub owner_namespace: String,
    pub stopped: bool,
    pub stop_reason: Option<String>,
    pub stop_committed_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetDispatchDecision {
    pub call: BudgetCallRecord,
    pub new_dispatch: bool,
}

/// A stage with no monetary calls has zero ledger totals, not evidence that its
/// CPU, storage, human or gray-operation costs were measured. Decimal strings
/// preserve exact totals even when the sum of historic calls exceeds i64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudgetStageMonetaryTotal {
    pub stage: BudgetStage,
    pub call_count: usize,
    pub known_actual_micros: String,
    pub finalized_actual_micros: String,
    pub held_reservation_micros: String,
    pub unknown_actual_call_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootBudgetUnresolvedReason {
    RootNotStopped,
    Reserved,
    Dispatched,
    Uncertain,
    MissingActualCharge,
    PartialActualCharge,
    ChargeIdentityMismatch,
    ExecutionOpen,
    CallNamespaceMismatch,
    GroupNamespaceMismatch,
    MissingDispatchGroup,
    InvalidCallState,
    UnexpectedTerminalCharge,
    CallCostOverrun,
    SpentLedgerMismatch,
    ReservationLedgerMismatch,
    AuthorizationLimitExceeded,
}

/// Unknown amounts stay None. An incompatible charge retains its original
/// amount and denomination here and in the call; it is never added as zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudgetUnresolvedRecord {
    pub namespace: Option<String>,
    pub call_id: Option<String>,
    pub dispatch_group_id: Option<String>,
    pub reason: RootBudgetUnresolvedReason,
    pub actual_cost_micros: Option<i64>,
    pub actual_currency: Option<String>,
    pub actual_pricing_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudgetLedgerEvent {
    pub sequence: i64,
    pub call_id: Option<String>,
    pub event_kind: String,
    pub event_at: i64,
    /// Original JSON text, including incompatible charges and stop reasons.
    pub details: String,
}

/// Admin-only, complete monetary ledger read in the caller's transaction.
/// This deliberately does not attest a complete optimization episode or P.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudgetMonetarySnapshot {
    pub schema_version: String,
    pub root: RootBudgetRecord,
    pub stage_totals: Vec<RootBudgetStageMonetaryTotal>,
    pub dispatch_groups: Vec<DispatchGroupRecord>,
    pub calls: Vec<BudgetCallRecord>,
    pub ledger_events: Vec<RootBudgetLedgerEvent>,
    pub known_actual_micros: String,
    pub finalized_actual_micros: String,
    pub held_reservation_micros: String,
    pub unresolved: Vec<RootBudgetUnresolvedRecord>,
    pub monetary_complete: bool,
    pub complete_episode_cost: bool,
    pub ordinary_p_eligible: bool,
    pub snapshot_digest: String,
}

/// Scoped summary attested by a real Admin through the private budget ledger.
/// Public object writes cannot issue it. Reading it rechecks the current ledger,
/// so a late charge or other changed row cannot inherit a historic seal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedRootBudgetEvidence {
    pub schema_version: String,
    pub root: RootBudgetRecord,
    pub stage_totals: Vec<RootBudgetStageMonetaryTotal>,
    pub call_count: usize,
    pub dispatch_group_count: usize,
    pub ledger_event_count: usize,
    pub known_actual_micros: String,
    pub finalized_actual_micros: String,
    pub held_reservation_micros: String,
    pub unresolved: Vec<RootBudgetUnresolvedRecord>,
    pub monetary_complete: bool,
    pub complete_episode_cost: bool,
    pub ordinary_p_eligible: bool,
    pub snapshot_digest: String,
    pub sealed_by_namespace: String,
    pub sealed_by_actor: String,
    pub sealed_at: i64,
}

/// A budget call named by its key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetCallKey {
    pub billing_scope: String,
    pub call_id: String,
}

/// What one run of `Store::recover_settled_open_executions` did. Both lists are in
/// key order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettledOpenRecovery {
    /// The finalized calls with a persisted response whose execution this run closed.
    pub closed: Vec<BudgetCallKey>,
    /// The finalized calls whose execution is open and that carry no persisted
    /// response. Nothing shows that their execution ended, so they are left open
    /// for an Admin to close or reconcile explicitly.
    pub open_without_artifact: Vec<BudgetCallKey>,
}

impl Store {
    pub async fn authorize_root_budget(
        &self,
        ctx: &Context,
        authorization: &RootBudgetAuthorization,
    ) -> Result<RootBudgetRecord> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        authorize_root_budget_in_tx(&mut tx, ctx, authorization).await?;
        tx.commit().await.map_err(internal)?;
        // The original wrapper commits before this separate Admin read; a
        // Session writer gets the same record from the one transaction.
        self.root_budget(ctx, &authorization.billing_scope)
            .await?
            .ok_or(Error::Internal)
    }

    pub async fn root_budget(
        &self,
        ctx: &Context,
        billing_scope: &str,
    ) -> Result<Option<RootBudgetRecord>> {
        require_budget_reader(ctx)?;
        identifier(billing_scope)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let root = root_budget_in_tx(&mut tx, ctx, billing_scope).await?;
        tx.commit().await.map_err(internal)?;
        Ok(root)
    }

    /// Appends one root stage plan version as a real Admin. The highest seq is
    /// the only active version; old versions are never updated or removed.
    pub async fn bind_root_stage_plan(
        &self,
        ctx: &Context,
        request: &RootStagePlanBindRequest,
    ) -> Result<RootStagePlanBinding> {
        ctx.require(&[Role::Admin])?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let binding = bind_root_stage_plan_in_tx(&mut tx, ctx, request).await?;
        tx.commit().await.map_err(internal)?;
        Ok(binding)
    }

    /// Reads the active (highest-seq) binding. A malformed or unsupported
    /// active version fails closed and never falls back to an older one.
    pub async fn root_stage_plan_binding(
        &self,
        ctx: &Context,
        billing_scope: &str,
    ) -> Result<Option<RootStagePlanBinding>> {
        require_budget_reader(ctx)?;
        identifier(billing_scope)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        require_namespace(&mut tx, billing_scope, ctx.namespace()).await?;
        let binding = active_root_stage_plan_in_tx(&mut tx, billing_scope).await?;
        tx.commit().await.map_err(internal)?;
        Ok(binding)
    }

    /// Reads one stored binding version by seq, including superseded legacy
    /// versions. The full plan body and digest are revalidated; a tampered or
    /// unsupported version fails closed instead of being treated as active.
    pub async fn root_stage_plan_binding_version(
        &self,
        ctx: &Context,
        billing_scope: &str,
        seq: i64,
    ) -> Result<Option<RootStagePlanBinding>> {
        require_budget_reader(ctx)?;
        identifier(billing_scope)?;
        if seq <= 0 {
            return Err(Error::Invalid(
                "root stage plan version seq must be positive".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(internal)?;
        require_namespace(&mut tx, billing_scope, ctx.namespace()).await?;
        let binding = binding_version_in_tx(&mut tx, billing_scope, seq).await?;
        tx.commit().await.map_err(internal)?;
        Ok(binding)
    }

    /// Lists every append-only binding version oldest first. The highest seq is
    /// the active version; every lower seq is a read-only legacy version and is
    /// never rewritten or removed here.
    pub async fn root_stage_plan_binding_versions(
        &self,
        ctx: &Context,
        billing_scope: &str,
    ) -> Result<Vec<RootStagePlanBinding>> {
        require_budget_reader(ctx)?;
        identifier(billing_scope)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        require_namespace(&mut tx, billing_scope, ctx.namespace()).await?;
        let rows = sqlx::query(
            "SELECT * FROM root_budget_stage_plan_bindings
             WHERE billing_scope=? ORDER BY seq ASC LIMIT 101",
        )
        .bind(billing_scope)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal)?;
        if rows.len() > 100 {
            return Err(Error::Capacity(evo_core::capacity::CapacityConflict {
                item: evo_core::capacity::CapacityItem::RootStagePlanVersionPerRoot,
                used: u64::try_from(rows.len()).map_err(|_| Error::Internal)?,
                limit: 100,
                delta: 1,
            }));
        }
        let mut versions = Vec::with_capacity(rows.len());
        for row in &rows {
            versions.push(validate_binding_version(row)?);
        }
        tx.commit().await.map_err(internal)?;
        Ok(versions)
    }

    /// Admin-only stage plan report. It is a new DTO derived from the binding,
    /// relation and event facts; the monetary snapshot wire is unchanged.
    pub async fn root_stage_plan_report(
        &self,
        ctx: &Context,
        billing_scope: &str,
    ) -> Result<RootStagePlanReport> {
        ctx.require(&[Role::Admin])?;
        identifier(billing_scope)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        require_namespace(&mut tx, billing_scope, ctx.namespace()).await?;
        let report = root_stage_plan_report_in_tx(&mut tx, billing_scope).await?;
        tx.commit().await.map_err(internal)?;
        Ok(report)
    }

    /// Reserves a call with no registered source closure. The reservation has no
    /// source for a revocation tombstone to refuse. The dispatch and the model
    /// settlement of the call read the closure its persisted request names instead
    /// (`begin_budget_dispatch`): a request that names a revoked run is refused at the
    /// dispatch and blocked at the settlement, a request whose closure cannot be read
    /// fails closed, and a call whose request names no source, or that carries no
    /// request, is not refused for its sources. The revocation cleanup rescans the
    /// request bodies of the calls that carry no closure. A call derived from runs
    /// belongs on `reserve_budget_call_with_sources`.
    pub async fn reserve_budget_call(
        &self,
        ctx: &Context,
        request: &BudgetCallReservation,
    ) -> Result<BudgetCallRecord> {
        self.reserve_budget_call_with_sources(ctx, request, &[])
            .await
    }

    /// Reserves a call and registers `source_ids`, the run ids its request is
    /// derived from, as its source closure (a budget call reference with one typed
    /// edge to each run).
    ///
    /// A source revoked as a `run` refuses a call that does not exist yet with
    /// `Forbidden` and writes nothing (plan §11: the revocation is written first, so
    /// new use is refused at once). A call that already exists is returned as
    /// before, so recovery and reconciliation are not blocked. A call reserved
    /// without `source_ids` has no closure to judge.
    pub async fn reserve_budget_call_with_sources(
        &self,
        ctx: &Context,
        request: &BudgetCallReservation,
        source_ids: &[String],
    ) -> Result<BudgetCallRecord> {
        self.reserve_budget_call_with_sources_typed(ctx, request, source_ids)
            .await?
            .map_err(BudgetPreDispatchRefusal::into_reservation_error)
    }

    pub async fn reserve_budget_call_typed(
        &self,
        ctx: &Context,
        request: &BudgetCallReservation,
    ) -> BudgetPreDispatchResult<BudgetCallRecord> {
        self.reserve_budget_call_with_sources_typed(ctx, request, &[])
            .await
    }

    /// Parallel typed gate. General database, commit, and unreadable source-ref
    /// errors are deliberately left in the outer result.
    pub async fn reserve_budget_call_with_sources_typed(
        &self,
        ctx: &Context,
        request: &BudgetCallReservation,
        source_ids: &[String],
    ) -> BudgetPreDispatchResult<BudgetCallRecord> {
        if require_budget_caller(ctx).is_err() {
            return Ok(Err(BudgetPreDispatchRefusal::Unauthorized));
        }
        if request
            .request_artifact
            .as_ref()
            .is_some_and(typed_budget::request_marks_e16)
        {
            return Err(typed_budget::mode_mismatch());
        }
        if let Err(refusal) = validate_reservation_typed(request)? {
            return Ok(Err(refusal));
        }
        let source_ids = match canonical_source_ids_typed(source_ids)? {
            Ok(ids) => ids,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let mut tx = self.pool.begin().await.map_err(internal)?;
        if let Some(existing) = load_call(&mut tx, &request.billing_scope, &request.call_id).await?
            && existing.namespace == ctx.namespace()
            && typed_budget::call_mode_in_tx(&mut tx, &existing).await? != E16CallMode::Legacy
        {
            return Err(typed_budget::mode_mismatch());
        }
        if typed_budget::new_ref_exists_in_tx(
            &mut tx,
            ctx.namespace(),
            &request.billing_scope,
            &request.call_id,
        )
        .await?
        {
            return Err(typed_budget::mode_mismatch());
        }
        let outcome = reserve_budget_call_with_sources_in_tx_typed(
            &mut tx,
            ctx,
            request,
            &source_ids,
            &StagePlanAttribution::Unattributed,
        )
        .await?;
        if outcome.is_ok() {
            tx.commit().await.map_err(internal)?;
        }
        Ok(outcome)
    }

    /// Reserve the independent E16 mode atomically with its complete raw-byte
    /// source bindings. This snapshot is not import/use/outbound authorization.
    pub async fn reserve_e16_budget_call(
        &self,
        ctx: &Context,
        request: &BudgetCallReservation,
    ) -> Result<BudgetCallRecord> {
        ctx.require(&[Role::Host, Role::Admin])?;
        let binding = request
            .request_artifact
            .as_ref()
            .and_then(typed_budget::parse_request)
            .ok_or_else(typed_budget::request_invalid)?;
        if binding.namespace != ctx.namespace()
            || binding.billing_scope != request.billing_scope
            || binding.call_id != request.call_id
            || binding.input_artifact.digest != request.actual_input_digest
        {
            return Err(typed_budget::request_invalid());
        }
        if let Err(refusal) = validate_reservation_typed(request)? {
            return Err(match refusal {
                BudgetPreDispatchRefusal::InvalidRequest { .. }
                | BudgetPreDispatchRefusal::InvalidArtifactDigest => {
                    typed_budget::request_invalid()
                }
                other => other.into_reservation_error(),
            });
        }
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let existing = load_call(&mut tx, &request.billing_scope, &request.call_id).await?;
        if let Some(call) = &existing {
            require_call_access(&mut tx, ctx, call).await?;
            if call.namespace != ctx.namespace() {
                return Err(Error::Forbidden);
            }
            if typed_budget::call_mode_in_tx(&mut tx, call).await? != E16CallMode::E16 {
                return Err(typed_budget::mode_mismatch());
            }
            if !typed_budget::call_sources_live_in_tx(&mut tx, call).await? {
                return Err(typed_budget::sources_unavailable());
            }
        } else {
            let legacy_digest = fingerprint(&(
                "rsia.budget_call_ref.v1",
                ctx.namespace(),
                &request.billing_scope,
                &request.call_id,
            ))?;
            let legacy_exists: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM objects WHERE namespace=? AND kind='artifact' AND id=?)")
                .bind(ctx.namespace()).bind(format!("budget-ref-{}", &legacy_digest[..32]))
                .fetch_one(&mut *tx).await.map_err(internal)?;
            if legacy_exists != 0
                || typed_budget::new_ref_exists_in_tx(
                    &mut tx,
                    ctx.namespace(),
                    &request.billing_scope,
                    &request.call_id,
                )
                .await?
            {
                return Err(typed_budget::mode_mismatch());
            }
        }
        if !typed_budget::snapshots_live_in_tx(&mut tx, ctx.namespace(), &binding.object_refs)
            .await?
        {
            return Err(typed_budget::sources_unavailable());
        }
        let call = reserve_budget_call_with_sources_in_tx_typed(
            &mut tx,
            ctx,
            request,
            &[],
            &StagePlanAttribution::Unattributed,
        )
        .await?
        .map_err(BudgetPreDispatchRefusal::into_reservation_error)?;
        if existing.is_none() {
            typed_budget::create_ref_in_tx(&mut tx, ctx, &call).await?;
        }
        tx.commit().await.map_err(internal)?;
        Ok(call)
    }

    /// The explicit new-mode content outlet. Historical budget getters and
    /// accounting transitions are not authorization to reuse their bodies.
    pub async fn consume_e16_budget_response(
        &self,
        ctx: &Context,
        billing_scope: &str,
        call_id: &str,
    ) -> Result<BudgetArtifact> {
        ctx.require(&[Role::Host, Role::Admin])?;
        identifier(billing_scope)?;
        identifier(call_id)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = load_call(&mut tx, billing_scope, call_id)
            .await?
            .ok_or(Error::NotFound)?;
        require_call_access(&mut tx, ctx, &call).await?;
        if typed_budget::call_mode_in_tx(&mut tx, &call).await? != E16CallMode::E16 {
            return Err(typed_budget::mode_mismatch());
        }
        if !typed_budget::call_sources_live_in_tx(&mut tx, &call).await?
            || call.response_usable != Some(true)
        {
            return Err(typed_budget::sources_unavailable());
        }
        let response = call
            .response_artifact
            .filter(|artifact| artifact.schema_version != "rsia.redacted.v1")
            .ok_or_else(typed_budget::sources_unavailable)?;
        tx.commit().await.map_err(internal)?;
        Ok(response)
    }

    pub async fn refence_reserved_budget_call(
        &self,
        ctx: &Context,
        request: &BudgetCallRefence,
    ) -> Result<BudgetCallRecord> {
        validate_budget_refence(ctx, request)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let updated = refence_reserved_budget_call_in_tx(&mut tx, ctx, request).await?;
        tx.commit().await.map_err(internal)?;
        Ok(updated)
    }

    /// Moves a reserved call to dispatched. Besides the lease, the root and the
    /// dispatch group, the source closure of the call is read: the registered one
    /// (its budget call reference) or, for a legacy call whose reference is missing,
    /// the closure its persisted request names, read as the revocation cleanup reads
    /// it. A call over a source revoked as a `run` is not dispatched and the decision
    /// is `Cancelled`, the one a caller releases the undispatched reservation on. A
    /// missing reference does not shrink the closure to nothing: when the request
    /// cannot be read for a closure, the dispatch fails closed with
    /// `Conflict("budget_sources_unverifiable")` and writes nothing (plan §7.2.1: a
    /// revocation rechecks a queued action before dispatch). A call whose request
    /// names no source, or that carries no request, is not refused for its sources.
    pub async fn begin_budget_dispatch(
        &self,
        ctx: &Context,
        fence: &BudgetCallFence,
    ) -> Result<BudgetDispatchDecision> {
        self.begin_budget_dispatch_typed(ctx, fence)
            .await?
            .map_err(BudgetPreDispatchRefusal::into_dispatch_error)
    }

    pub async fn begin_budget_dispatch_typed(
        &self,
        ctx: &Context,
        fence: &BudgetCallFence,
    ) -> BudgetPreDispatchResult<BudgetDispatchDecision> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let decision = begin_budget_dispatch_in_tx_typed(&mut tx, ctx, fence).await?;
        if decision.is_err() {
            return Ok(decision);
        }
        tx.commit().await.map_err(internal)?;
        Ok(decision)
    }

    /// Read for the broker's idempotency gate with determinate access refusals.
    /// The legacy lookup continues to return its original access errors.
    pub async fn budget_call_typed(
        &self,
        ctx: &Context,
        billing_scope: &str,
        call_id: &str,
    ) -> BudgetPreDispatchResult<Option<BudgetCallRecord>> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let result = budget_call_in_tx_typed(&mut tx, ctx, billing_scope, call_id).await?;
        if result.is_err() {
            return Ok(result);
        }
        tx.commit().await.map_err(internal)?;
        Ok(result)
    }

    pub async fn finalize_budget_call(
        &self,
        ctx: &Context,
        fence: &BudgetCallFence,
        charge: &UsageCharge,
    ) -> Result<BudgetCallRecord> {
        require_budget_caller(ctx)?;
        validate_fence(fence)?;
        validate_charge(charge)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = load_call(&mut tx, &fence.billing_scope, &fence.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        require_call_access(&mut tx, ctx, &call).await?;
        fence_call(&call, fence, false)?;
        if call.state == BudgetCallState::Finalized {
            ensure_same_charge(&call, charge)?;
            tx.commit().await.map_err(internal)?;
            return Ok(call);
        }
        if call.state != BudgetCallState::Dispatched {
            return Err(Error::Conflict("finalize_requires_dispatched_call".into()));
        }
        if fence.now > call.lease_until {
            return Err(Error::Cancelled);
        }
        let accounting_overflow =
            apply_charge(&mut tx, &call, charge, fence.now, "finalized").await?;
        let updated = load_call(&mut tx, &fence.billing_scope, &fence.call_id)
            .await?
            .ok_or(Error::Internal)?;
        tx.commit().await.map_err(internal)?;
        if accounting_overflow {
            return Err(Error::Conflict("cost_accounting_overflow".into()));
        }
        Ok(updated)
    }

    /// Atomically finalizes a registered in-process execution: a zero known
    /// charge in the root's pricing identity, the typed settlement as the row's
    /// response artifact, and execution closed. Only the executing Worker or
    /// Host may settle; Admin cannot mint registered execution evidence.
    pub async fn settle_registered_execution_call(
        &self,
        ctx: &Context,
        fence: &BudgetCallFence,
        charge: &UsageCharge,
        settlement: &RegisteredExecutionSettlement,
    ) -> Result<BudgetCallRecord> {
        ctx.require(&[Role::Worker, Role::Host])?;
        validate_fence(fence)?;
        validate_charge(charge)?;
        settlement.validate()?;
        if charge.amount_micros != 0 {
            return Err(Error::Invalid(
                "registered pure-function execution has no monetary cost".into(),
            ));
        }
        if settlement.call_id != fence.call_id
            || settlement.request_digest != fence.actual_input_digest
            || settlement.output_digest != charge.output_digest
        {
            return Err(Error::Conflict(
                "registered settlement differs from its fence and charge".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = load_call(&mut tx, &fence.billing_scope, &fence.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        require_call_access(&mut tx, ctx, &call).await?;
        if call.namespace != ctx.namespace() {
            return Err(Error::NotFound);
        }
        fence_call(&call, fence, false)?;
        if typed_budget::call_mode_in_tx(&mut tx, &call).await? != E16CallMode::Legacy {
            return Err(typed_budget::mode_mismatch());
        }
        if !matches!(
            call.stage,
            BudgetStage::DevelopmentExecution | BudgetStage::DevelopmentScoring
        ) {
            return Err(Error::Conflict(
                "registered settlement requires a development stage".into(),
            ));
        }
        if call.dispatch_id.as_deref() != Some(settlement.dispatch_id.as_str()) {
            return Err(Error::Conflict(
                "registered settlement dispatch differs from the row".into(),
            ));
        }
        if let Some(existing) = &call.response_artifact {
            let same_submission = match ReadRegisteredExecutionSettlement::from_call(&call)? {
                ReadRegisteredExecutionSettlement::Legacy(_) => {
                    *existing
                        == BudgetArtifact::from_serializable(
                            REGISTERED_EXECUTION_SETTLEMENT_SCHEMA,
                            settlement,
                        )?
                }
                ReadRegisteredExecutionSettlement::Current(previous) => {
                    previous.settlement == *settlement
                }
            };
            if !same_submission || !call.execution_closed {
                return Err(Error::Conflict(
                    "registered settlement reused with different content".into(),
                ));
            }
            ensure_same_charge(&call, charge)?;
            tx.commit().await.map_err(internal)?;
            return Ok(call);
        }
        if call.state != BudgetCallState::Dispatched {
            return Err(Error::Conflict(
                "registered settlement requires dispatched call".into(),
            ));
        }
        if fence.now > call.lease_until {
            return Err(Error::Cancelled);
        }
        let root_stopped: i64 =
            sqlx::query_scalar("SELECT stopped FROM root_budgets WHERE billing_scope=?")
                .bind(&fence.billing_scope)
                .fetch_one(&mut *tx)
                .await
                .map_err(internal)?;
        let group_stopped: i64 = sqlx::query_scalar(
            "SELECT stopped FROM root_budget_dispatch_groups
             WHERE billing_scope=? AND dispatch_group_id=?",
        )
        .bind(&fence.billing_scope)
        .bind(&call.dispatch_group_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        let block_reason = if root_stopped != 0 {
            Some("root_stopped")
        } else if group_stopped != 0 {
            Some("dispatch_group_stopped")
        } else {
            None
        };
        let artifact = BudgetArtifact::from_serializable(
            REGISTERED_EXECUTION_SETTLEMENT_SCHEMA_V2,
            &RegisteredExecutionSettlementV2 {
                schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA_V2.into(),
                settler_actor: ctx.actor().into(),
                settler_role: ctx.role(),
                settlement: settlement.clone(),
            },
        )?;
        let accounting_overflow = apply_charge(
            &mut tx,
            &call,
            charge,
            fence.now,
            "registered_execution_settled",
        )
        .await?;
        sqlx::query(
            "UPDATE root_budget_calls SET
               response_artifact_schema=?,response_artifact_digest=?,response_artifact_body=?,
               response_usable=?,response_block_reason=?,
               execution_closed=1,execution_closed_at=?,execution_close_reason=?
             WHERE billing_scope=? AND call_id=?",
        )
        .bind(&artifact.schema_version)
        .bind(&artifact.digest)
        .bind(&artifact.body)
        .bind(block_reason.is_none())
        .bind(block_reason)
        .bind(fence.now)
        .bind("registered_execution_settled")
        .bind(&call.billing_scope)
        .bind(&call.call_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        insert_event(
            &mut tx,
            &call.billing_scope,
            Some(&call.call_id),
            "registered_execution_settled",
            fence.now,
            json!({
                "dispatch_id": settlement.dispatch_id,
                "request_digest": settlement.request_digest,
                "output_digest": settlement.output_digest,
                "provenance": settlement.provenance,
                "target_id": settlement.target_id,
                "runner_digest": settlement.runner_digest,
                "response_usable": block_reason.is_none(),
                "response_block_reason": block_reason,
            }),
        )
        .await?;
        let updated = load_call(&mut tx, &fence.billing_scope, &fence.call_id)
            .await?
            .ok_or(Error::Internal)?;
        tx.commit().await.map_err(internal)?;
        if accounting_overflow {
            return Err(Error::Conflict("cost_accounting_overflow".into()));
        }
        Ok(updated)
    }

    /// Atomically accounts a model charge and stores the exact request/transport/response facts.
    /// Stop and lease state are re-read in this transaction after the provider await.
    ///
    /// The source closure of the call is read in this transaction too, as the
    /// dispatch reads it: the registered one (its budget call reference) or, for a
    /// legacy call whose reference is missing, the closure its persisted request
    /// names. A response can arrive after a source was revoked as a `run`, and the
    /// cleanup never reads a call whose request it has already redacted, so nothing
    /// else would stop its output from being stored as it came (plan §11: a late
    /// output forms no usable candidate, its real cost is still booked; §11.5: the
    /// late result of a dispatched call cannot regain validity). When any source of
    /// the call is revoked:
    ///
    /// * the charge is booked exactly as without a revocation;
    /// * the response is blocked, so the usable response, which carries the output,
    ///   is never stored. The reason is `source_revoked`, which comes first among the
    ///   reasons that held before the response arrived (an expired lease, a stopped
    ///   root or dispatch group); a pricing mismatch, a reason the caller forces, an
    ///   accounting overflow and a cost overrun still take precedence, as before;
    /// * the transport body, which carries the output too, is stored as its
    ///   `rsia.redacted.v1` redaction whichever reason is recorded.
    ///
    /// A missing reference does not shrink the closure to nothing. When the request
    /// of such a call cannot be read for a closure, whether a source was revoked
    /// cannot be told, and the settlement fails closed the same way: the charge is
    /// booked, the response is blocked as `budget_sources_unverifiable` (in the place
    /// of `source_revoked`) and the transport is stored as its redaction. A request
    /// that names no source has no source to judge. The cleanup rescan, which finds a
    /// call by the closure its request names, still redacts what the call holds. A
    /// reference that exists but cannot be read fails the settlement with `Internal`
    /// and writes nothing, like the dispatch: the call stays dispatched and its cost
    /// can still be reconciled (`reconcile_budget_call_cost`).
    pub async fn settle_model_budget_call(
        &self,
        ctx: &Context,
        fence: &BudgetCallFence,
        charge: &UsageCharge,
        evidence: &ModelCallSettlementEvidence,
    ) -> Result<ModelCallSettlement> {
        ctx.require(&[Role::Host, Role::Admin])?;
        validate_fence(fence)?;
        validate_charge(charge)?;
        validate_model_settlement_evidence(evidence)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = load_call(&mut tx, &fence.billing_scope, &fence.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        require_call_access(&mut tx, ctx, &call).await?;
        fence_call(&call, fence, false)?;
        if call.response_artifact.is_some() {
            ensure_same_charge(&call, charge)?;
            match typed_budget::call_mode_in_tx(&mut tx, &call).await? {
                E16CallMode::Mismatch => return Err(typed_budget::mode_mismatch()),
                E16CallMode::E16 => {
                    if !typed_budget::call_sources_live_in_tx(&mut tx, &call).await? {
                        return Err(typed_budget::sources_unavailable());
                    }
                }
                E16CallMode::Legacy => {}
            }
            ensure_same_model_settlement(&call, evidence)?;
            let settlement = settlement_from_call(call, false)?;
            tx.commit().await.map_err(internal)?;
            return Ok(settlement);
        }
        let e16_mode = typed_budget::call_mode_in_tx(&mut tx, &call).await?;
        if e16_mode == E16CallMode::Mismatch {
            return Err(typed_budget::mode_mismatch());
        }
        if call.state != BudgetCallState::Dispatched {
            return Err(Error::Conflict(
                "model settlement requires dispatched call".into(),
            ));
        }
        if call.request_artifact.is_none() {
            return Err(Error::Conflict(
                "model settlement requires persisted request artifact".into(),
            ));
        }
        let root = sqlx::query(
            "SELECT stopped,currency,pricing_version FROM root_budgets WHERE billing_scope=?",
        )
        .bind(&fence.billing_scope)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        let root_stopped: i64 = root.try_get("stopped").map_err(internal)?;
        let root_currency: String = root.try_get("currency").map_err(internal)?;
        let root_pricing_version: String = root.try_get("pricing_version").map_err(internal)?;
        let group_stopped: i64 = sqlx::query_scalar(
            "SELECT stopped FROM root_budget_dispatch_groups
             WHERE billing_scope=? AND dispatch_group_id=?",
        )
        .bind(&fence.billing_scope)
        .bind(&call.dispatch_group_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        // The tombstone is committed before the cleanup starts, so a source is revoked
        // from the moment `begin_revoke` commits, whatever the cleanup has done since.
        // A legacy call whose reference is missing is judged by the closure its request
        // names; when that cannot be read the call is `unverifiable`.
        let (source_revoked, unverifiable, e16_unavailable) = match e16_mode {
            E16CallMode::Mismatch => return Err(typed_budget::mode_mismatch()),
            E16CallMode::E16 => (
                false,
                false,
                !typed_budget::call_sources_live_in_tx(&mut tx, &call).await?,
            ),
            E16CallMode::Legacy => match legacy_call_sources(&mut tx, &call).await? {
                LegacyCallSources::Named(source_ids) => {
                    let roots: Vec<_> = source_ids.iter().map(|id| ("run", id.as_str())).collect();
                    let status =
                        crate::revocation::status_in_tx(&mut tx, &call.namespace, &roots, false)
                            .await?;
                    (
                        status == crate::revocation::RevocationStatus::Blocked,
                        status == crate::revocation::RevocationStatus::Unverifiable,
                        false,
                    )
                }
                LegacyCallSources::Unverifiable => (false, true, false),
            },
        };
        let preexisting_block = if e16_unavailable {
            Some(typed_budget::E16_SOURCE_UNAVAILABLE)
        } else if source_revoked {
            Some(SOURCE_REVOKED)
        } else if unverifiable {
            Some(BUDGET_SOURCES_UNVERIFIABLE)
        } else if fence.now > call.lease_until {
            Some("lease_expired")
        } else if root_stopped != 0 {
            Some("root_stopped")
        } else if group_stopped != 0 {
            Some("dispatch_group_stopped")
        } else {
            None
        };
        let pricing_mismatch =
            root_currency != charge.currency || root_pricing_version != charge.pricing_version;
        let accounting_overflow = if pricing_mismatch {
            preserve_charge_identity_mismatch(&mut tx, &call, charge, fence.now).await?;
            false
        } else {
            apply_charge(&mut tx, &call, charge, fence.now, "model_cost_settled").await?
        };
        let block_reason = if pricing_mismatch {
            Some("charge_pricing_identity_mismatch")
        } else if let Some(reason) = evidence.forced_block_reason.as_deref() {
            Some(reason)
        } else if accounting_overflow {
            Some("cost_accounting_overflow")
        } else if charge.amount_micros > call.reserved_micros {
            Some("cost_overrun")
        } else {
            preexisting_block
        };
        let response_usable = block_reason.is_none();
        let response = if response_usable {
            evidence.usable_response.as_ref().ok_or(Error::Internal)?
        } else {
            &evidence.blocked_response
        };
        // The transport body carries the model output. Over a revoked source, or a
        // closure that cannot be verified, it is stored as its redaction, whichever
        // reason won the chain above: the stored digest is that of the redaction (every
        // read of the row rechecks it against the stored body) and the redaction names
        // the digest of the body it replaces.
        let redaction_reason = if e16_unavailable {
            typed_budget::E16_SOURCE_UNAVAILABLE
        } else if unverifiable {
            BUDGET_SOURCES_UNVERIFIABLE
        } else {
            SOURCE_REVOKED
        };
        let redacted_transport = if source_revoked || unverifiable || e16_unavailable {
            redacted_artifact(Some(&evidence.transport_artifact), redaction_reason)?
        } else {
            None
        };
        let transport = redacted_transport
            .as_ref()
            .unwrap_or(&evidence.transport_artifact);
        // Invalid E16 sources never leave request or arbitrary blocked-response
        // content in either the persisted row or the settlement's nested call.
        let redacted_response = if e16_unavailable {
            redacted_artifact(Some(response), typed_budget::E16_SOURCE_UNAVAILABLE)?
        } else {
            None
        };
        let response = redacted_response.as_ref().unwrap_or(response);
        let request = if e16_unavailable {
            redacted_artifact(
                call.request_artifact.as_ref(),
                typed_budget::E16_SOURCE_UNAVAILABLE,
            )?
        } else {
            call.request_artifact.clone()
        };
        sqlx::query(
            "UPDATE root_budget_calls SET
               request_artifact_schema=?,request_artifact_digest=?,request_artifact_body=?,
               execution_provenance=?,actual_model_digest=?,
               transport_artifact_schema=?,transport_artifact_digest=?,transport_artifact_body=?,
               response_artifact_schema=?,response_artifact_digest=?,response_artifact_body=?,
               response_usable=?,response_block_reason=?
             WHERE billing_scope=? AND call_id=?",
        )
        .bind(request.as_ref().map(|value| value.schema_version.as_str()))
        .bind(request.as_ref().map(|value| value.digest.as_str()))
        .bind(request.as_ref().map(|value| value.body.as_str()))
        .bind(evidence.provenance.as_str())
        .bind(&evidence.actual_model_digest)
        .bind(&transport.schema_version)
        .bind(&transport.digest)
        .bind(&transport.body)
        .bind(&response.schema_version)
        .bind(&response.digest)
        .bind(&response.body)
        .bind(response_usable)
        .bind(block_reason)
        .bind(&call.billing_scope)
        .bind(&call.call_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        let mut details = json!({
            "request_artifact_digest": call.request_artifact.as_ref().map(|value| &value.digest),
            "transport_artifact_digest": transport.digest,
            "response_artifact_digest": response.digest,
            "response_usable": response_usable,
            "response_block_reason": block_reason,
            "execution_provenance": evidence.provenance,
            "actual_model_digest": evidence.actual_model_digest,
        });
        if source_revoked {
            // The reason on the row is whichever one came first in the chain above,
            // so the revocation is recorded here whichever that was. The digest of
            // the transport as the provider sent it is a digest, not content.
            details["source_revoked"] = json!(true);
            details["transport_original_digest"] = json!(evidence.transport_artifact.digest);
        }
        if unverifiable {
            // Recorded whichever reason the row carries, as a revocation is.
            details[BUDGET_SOURCES_UNVERIFIABLE] = json!(true);
            details["transport_original_digest"] = json!(evidence.transport_artifact.digest);
        }
        if e16_unavailable {
            details["e16_source_unavailable"] = json!(true);
            details["transport_original_digest"] = json!(evidence.transport_artifact.digest);
        }
        insert_event(
            &mut tx,
            &call.billing_scope,
            Some(&call.call_id),
            "model_response_persisted",
            fence.now,
            details,
        )
        .await?;
        let call = load_call(&mut tx, &fence.billing_scope, &fence.call_id)
            .await?
            .ok_or(Error::Internal)?;
        let settlement = settlement_from_call(call, accounting_overflow)?;
        tx.commit().await.map_err(internal)?;
        Ok(settlement)
    }

    pub async fn mark_budget_call_uncertain(
        &self,
        ctx: &Context,
        fence: &BudgetCallFence,
        reason: &str,
    ) -> Result<BudgetCallRecord> {
        require_budget_caller(ctx)?;
        validate_fence(fence)?;
        validate_reason(reason)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = load_call(&mut tx, &fence.billing_scope, &fence.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        require_call_access(&mut tx, ctx, &call).await?;
        fence_call(&call, fence, false)?;
        if call.state == BudgetCallState::Uncertain {
            tx.commit().await.map_err(internal)?;
            return Ok(call);
        }
        if call.state != BudgetCallState::Dispatched {
            return Err(Error::Conflict("uncertain_requires_dispatched_call".into()));
        }
        sqlx::query(
            "UPDATE root_budget_calls SET state='uncertain',terminal_reason=?
             WHERE billing_scope=? AND call_id=?",
        )
        .bind(reason)
        .bind(&fence.billing_scope)
        .bind(&fence.call_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        insert_event(
            &mut tx,
            &fence.billing_scope,
            Some(&fence.call_id),
            "usage_uncertain",
            fence.now,
            json!({"reason": reason}),
        )
        .await?;
        let updated = load_call(&mut tx, &fence.billing_scope, &fence.call_id)
            .await?
            .ok_or(Error::Internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(updated)
    }

    pub async fn release_undispatched_budget_call(
        &self,
        ctx: &Context,
        fence: &BudgetCallFence,
        reason: &str,
    ) -> Result<BudgetCallRecord> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = release_undispatched_in_tx(&mut tx, ctx, fence, reason).await?;
        tx.commit().await.map_err(internal)?;
        Ok(call)
    }

    pub async fn cancel_budget_call(
        &self,
        ctx: &Context,
        fence: &BudgetCallFence,
        reason: &str,
    ) -> Result<BudgetCallRecord> {
        require_budget_caller(ctx)?;
        validate_fence(fence)?;
        validate_reason(reason)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = load_call(&mut tx, &fence.billing_scope, &fence.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        require_call_access(&mut tx, ctx, &call).await?;
        fence_call(&call, fence, false)?;
        match call.state {
            BudgetCallState::Reserved => {
                release_reserved(
                    &mut tx,
                    &call,
                    BudgetCallState::Cancelled,
                    reason,
                    fence.now,
                )
                .await?;
            }
            BudgetCallState::Dispatched => {
                sqlx::query(
                    "UPDATE root_budget_calls SET state='uncertain',terminal_reason=?
                     WHERE billing_scope=? AND call_id=?",
                )
                .bind(reason)
                .bind(&fence.billing_scope)
                .bind(&fence.call_id)
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
                insert_event(
                    &mut tx,
                    &fence.billing_scope,
                    Some(&fence.call_id),
                    "cancel_requested_after_dispatch",
                    fence.now,
                    json!({"reason": reason, "usage": "uncertain"}),
                )
                .await?;
            }
            BudgetCallState::Uncertain => {}
            _ => {
                return Err(Error::Conflict("call_not_cancellable".into()));
            }
        }
        let updated = load_call(&mut tx, &fence.billing_scope, &fence.call_id)
            .await?
            .ok_or(Error::Internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(updated)
    }

    pub async fn reconcile_budget_call_cost(
        &self,
        ctx: &Context,
        billing_scope: &str,
        call_id: &str,
        charge: &UsageCharge,
        now: i64,
    ) -> Result<BudgetCallRecord> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let outcome =
            reconcile_budget_call_cost_in_tx(&mut tx, ctx, billing_scope, call_id, charge, now)
                .await?;
        tx.commit().await.map_err(internal)?;
        // The original wrapper committed the preserved overflow facts before
        // reporting this Conflict; that order is unchanged.
        match outcome {
            BudgetReconcileOutcome::Applied(call)
            | BudgetReconcileOutcome::AlreadyFinalized(call) => Ok(call),
            BudgetReconcileOutcome::PreservedAccountingOverflow(_) => {
                Err(Error::Conflict("cost_accounting_overflow".into()))
            }
        }
    }

    pub async fn close_budget_call_execution(
        &self,
        ctx: &Context,
        billing_scope: &str,
        call_id: &str,
        dispatch_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<BudgetCallRecord> {
        require_budget_caller(ctx)?;
        identifier(billing_scope)?;
        identifier(call_id)?;
        identifier(dispatch_id)?;
        validate_reason(reason)?;
        valid_time(now)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = load_call(&mut tx, billing_scope, call_id)
            .await?
            .ok_or(Error::NotFound)?;
        require_call_access(&mut tx, ctx, &call).await?;
        if call.dispatch_id.as_deref() != Some(dispatch_id) {
            return Err(Error::Cancelled);
        }
        if call.execution_closed {
            tx.commit().await.map_err(internal)?;
            return Ok(call);
        }
        if !matches!(
            call.state,
            BudgetCallState::Dispatched | BudgetCallState::Uncertain | BudgetCallState::Finalized
        ) {
            return Err(Error::Conflict("execution_not_closeable".into()));
        }
        sqlx::query(
            "UPDATE root_budget_calls SET execution_closed=1,execution_closed_at=?,execution_close_reason=?
             WHERE billing_scope=? AND call_id=?",
        )
        .bind(now)
        .bind(reason)
        .bind(billing_scope)
        .bind(call_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        insert_event(
            &mut tx,
            billing_scope,
            Some(call_id),
            "execution_closed",
            now,
            json!({"dispatch_id": dispatch_id, "reason": reason}),
        )
        .await?;
        let updated = load_call(&mut tx, billing_scope, call_id)
            .await?
            .ok_or(Error::Internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(updated)
    }

    /// Recovers the execution closes a crash lost; `rsia serve` runs it at startup.
    ///
    /// A model call is settled (its charge booked and its response persisted) and its
    /// execution closed in two commits. A crash between them leaves the call finalized
    /// with its execution open, and only a replay of the same request closes it then.
    /// A call that is never replayed holds its billing root: the dispatch gate counts
    /// it as active, so every later call of the root is refused with
    /// `root_concurrency_limit`, and the monetary snapshot reports it as
    /// `ExecutionOpen`.
    ///
    /// This reads the finalized calls whose execution is open, of every namespace, in
    /// the billing scopes `ctx` is a member of, page by page in key order, so that it
    /// depends on no scan of a fixed size (plan E08: the cleanup goes on after a
    /// crash). What it does with a call is derived from the call row alone, as plan
    /// §7.2.1 rebuilds its recovery queue from durable records and keeps no fact source
    /// of its own:
    ///
    /// * a call that carries a persisted response, usable, blocked or redacted, had its
    ///   charge booked and its execution ended. It is closed through
    ///   `close_budget_call_execution` with the reason
    ///   `recovered_persisted_response_at_startup`, which writes the three execution
    ///   columns and one `execution_closed` event and no amount, state or response
    ///   (plan §11.5: recovery restores the cost state of what was dispatched; E08: no
    ///   real bill is cleared). A finalized call with an open execution always holds
    ///   its dispatch id: the dispatch writes it in the update that opens the execution.
    /// * a call that carries no response is listed in `open_without_artifact` and left
    ///   open: nothing shows that its execution ended (plan §6.6: a status does not
    ///   report as exited what may not have exited), so an Admin closes or reconciles
    ///   it explicitly (E04: what is unknown is left to a manual reconciliation and
    ///   nothing is feigned).
    ///
    /// Dispatched and uncertain calls are not read (plan §6.6: a dispatched call whose
    /// cost is unknown stays uncertain until it is reconciled), nor are closed calls,
    /// so a second run closes nothing and writes nothing. The calls of a root `ctx` is
    /// not a member of are neither read nor listed, as an Admin outside a root reads
    /// and closes none of its calls. Only an Admin runs it. An error stops the run,
    /// and the calls it closed before stay closed.
    pub async fn recover_settled_open_executions(
        &self,
        ctx: &Context,
        now: i64,
    ) -> Result<SettledOpenRecovery> {
        // The reason this recovery records its closes with, and the calls it reads at
        // a time.
        const REASON: &str = "recovered_persisted_response_at_startup";
        const PAGE: i64 = 128;
        ctx.require(&[Role::Admin])?;
        valid_time(now)?;
        let mut recovery = SettledOpenRecovery::default();
        let mut cursor: Option<(String, String)> = None;
        loop {
            let (after_scope, after_call) = match &cursor {
                Some((scope, call)) => (Some(scope.as_str()), Some(call.as_str())),
                None => (None, None),
            };
            // A page is read and released before anything is closed: the store has a
            // single connection, and every close takes it in a transaction of its own.
            let mut tx = self.pool.begin().await.map_err(internal)?;
            let rows = sqlx::query(
                "SELECT billing_scope,call_id,dispatch_id,
                   response_artifact_body IS NOT NULL AS has_response
                 FROM root_budget_calls
                 WHERE state='finalized' AND execution_closed=0
                   AND ((response_artifact_body IS NOT NULL AND dispatch_id IS NOT NULL)
                     OR response_artifact_body IS NULL)
                   AND EXISTS(SELECT 1 FROM root_budget_namespaces AS member
                     WHERE member.billing_scope=root_budget_calls.billing_scope
                       AND member.namespace=?)
                   AND (? IS NULL OR billing_scope>? OR (billing_scope=? AND call_id>?))
                 ORDER BY billing_scope,call_id LIMIT ?",
            )
            .bind(ctx.namespace())
            .bind(after_scope)
            .bind(after_scope)
            .bind(after_scope)
            .bind(after_call)
            .bind(PAGE)
            .fetch_all(&mut *tx)
            .await
            .map_err(internal)?;
            tx.commit().await.map_err(internal)?;
            if rows.is_empty() {
                return Ok(recovery);
            }
            for row in &rows {
                let key = BudgetCallKey {
                    billing_scope: row.try_get("billing_scope").map_err(internal)?,
                    call_id: row.try_get("call_id").map_err(internal)?,
                };
                cursor = Some((key.billing_scope.clone(), key.call_id.clone()));
                if row.try_get::<i64, _>("has_response").map_err(internal)? == 0 {
                    recovery.open_without_artifact.push(key);
                    continue;
                }
                let dispatch_id: String = row.try_get("dispatch_id").map_err(internal)?;
                let call = self
                    .close_budget_call_execution(
                        ctx,
                        &key.billing_scope,
                        &key.call_id,
                        &dispatch_id,
                        REASON,
                        now,
                    )
                    .await?;
                // A call its own caller closed after the page was read is closed, but
                // not by this run.
                if call.execution_close_reason.as_deref() == Some(REASON) {
                    recovery.closed.push(key);
                }
            }
        }
    }

    pub async fn stop_root_budget(
        &self,
        ctx: &Context,
        billing_scope: &str,
        reason: &str,
        now: i64,
    ) -> Result<RootBudgetRecord> {
        ctx.require(&[Role::Admin])?;
        identifier(billing_scope)?;
        validate_reason(reason)?;
        valid_time(now)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let root = stop_root_budget_in_tx(&mut tx, ctx, billing_scope, reason, now).await?;
        tx.commit().await.map_err(internal)?;
        Ok(root)
    }

    pub async fn stop_dispatch_group(
        &self,
        ctx: &Context,
        billing_scope: &str,
        dispatch_group_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<DispatchGroupRecord> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let updated =
            stop_dispatch_group_in_tx(&mut tx, ctx, billing_scope, dispatch_group_id, reason, now)
                .await?;
        tx.commit().await.map_err(internal)?;
        Ok(updated)
    }

    pub async fn dispatch_group(
        &self,
        ctx: &Context,
        billing_scope: &str,
        dispatch_group_id: &str,
    ) -> Result<Option<DispatchGroupRecord>> {
        require_budget_reader(ctx)?;
        identifier(billing_scope)?;
        identifier(dispatch_group_id)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        require_namespace(&mut tx, billing_scope, ctx.namespace()).await?;
        let group = load_dispatch_group(&mut tx, billing_scope, dispatch_group_id).await?;
        if let Some(group) = &group
            && ctx.role() != Role::Admin
            && group.owner_namespace != ctx.namespace()
        {
            return Err(Error::NotFound);
        }
        tx.commit().await.map_err(internal)?;
        Ok(group)
    }

    pub async fn budget_call(
        &self,
        ctx: &Context,
        billing_scope: &str,
        call_id: &str,
    ) -> Result<Option<BudgetCallRecord>> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = budget_call_in_tx(&mut tx, ctx, billing_scope, call_id).await?;
        tx.commit().await.map_err(internal)?;
        Ok(call)
    }
}

impl Session {
    /// Reads root authorization in this Session; never acquires another pool
    /// connection while the caller owns the instance's single connection.
    pub async fn root_budget(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
    ) -> Result<Option<RootBudgetRecord>> {
        root_budget_in_tx(&mut self.tx, ctx, billing_scope).await
    }

    /// Writes the real root authorization in the caller's Session. Nothing is
    /// committed here: the caller's one transaction also holds the management
    /// job terminal state.
    pub async fn authorize_root_budget(
        &mut self,
        ctx: &Context,
        authorization: &RootBudgetAuthorization,
    ) -> Result<RootBudgetRecord> {
        authorize_root_budget_in_tx(&mut self.tx, ctx, authorization).await
    }

    /// Applies one real cost reconciliation in the caller's Session and
    /// returns its typed outcome. An accounting overflow keeps the committed
    /// charge-preservation facts in the caller's transaction; it is never
    /// turned into a rollback, refund or zero amount here.
    pub async fn reconcile_budget_call_cost(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
        call_id: &str,
        charge: &UsageCharge,
        now: i64,
    ) -> Result<BudgetReconcileOutcome> {
        reconcile_budget_call_cost_in_tx(&mut self.tx, ctx, billing_scope, call_id, charge, now)
            .await
    }

    /// The complete, bounded, read-only open-call projection of one authorized
    /// root across namespaces. It scans the whole billing scope in keyset pages
    /// and never narrows to the caller's namespace or dispatch group. The
    /// cursor is a position, not authority: every page re-checks the Admin role
    /// and root membership.
    pub async fn list_open_budget_calls(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
        page_size: u16,
        cursor: Option<&str>,
    ) -> Result<BudgetOpenCallsPage> {
        ctx.require(&[Role::Admin])?;
        identifier(billing_scope)?;
        if !(1..=BUDGET_LIST_OPEN_MAX_PAGE_SIZE).contains(&page_size) {
            return Err(Error::Invalid(format!(
                "budget list page_size must be 1..={BUDGET_LIST_OPEN_MAX_PAGE_SIZE}"
            )));
        }
        let root = root_budget_in_tx(&mut self.tx, ctx, billing_scope)
            .await?
            .ok_or(Error::NotFound)?;
        let cursor = cursor.map(parse_budget_list_open_cursor).transpose()?;

        // Pass 1: the anchor covers the whole projected view in call_id order.
        let mut anchor = hash(
            format!(
                "{}\0{}\0{}\0{}\0{}",
                BUDGET_OPEN_CALLS_PAGE_SCHEMA,
                billing_scope,
                root.stopped,
                root.spent_micros,
                root.reserved_micros
            )
            .as_bytes(),
        );
        let mut after = String::new();
        loop {
            let rows = sqlx::query(
                "SELECT * FROM root_budget_calls WHERE billing_scope=? AND call_id>? ORDER BY call_id LIMIT 1000",
            )
            .bind(billing_scope)
            .bind(&after)
            .fetch_all(&mut *self.tx)
            .await
            .map_err(internal)?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let call = call_from_row(&row)?;
                after = call.call_id.clone();
                if let Some(view) = open_call_view(&call) {
                    let body = serde_json::to_string(&view).map_err(|_| Error::Internal)?;
                    anchor = hash(format!("{anchor}\0{body}").as_bytes());
                }
            }
        }
        if let Some(cursor) = &cursor
            && cursor.anchor != anchor
        {
            return Err(Error::Conflict("budget_list_open_anchor_changed".into()));
        }

        // Pass 2: collect only the requested page (plus one lookahead entry).
        let mut after = cursor
            .as_ref()
            .map(|cursor| cursor.after_call_id.clone())
            .unwrap_or_default();
        let mut entries = Vec::with_capacity(usize::from(page_size) + 1);
        loop {
            let rows = sqlx::query(
                "SELECT * FROM root_budget_calls WHERE billing_scope=? AND call_id>? ORDER BY call_id LIMIT 1000",
            )
            .bind(billing_scope)
            .bind(&after)
            .fetch_all(&mut *self.tx)
            .await
            .map_err(internal)?;
            if rows.is_empty() {
                break;
            }
            let mut last = None;
            for row in rows {
                let call = call_from_row(&row)?;
                last = Some(call.call_id.clone());
                if let Some(view) = open_call_view(&call) {
                    entries.push(view);
                    if entries.len() > usize::from(page_size) {
                        break;
                    }
                }
            }
            if entries.len() > usize::from(page_size) {
                break;
            }
            match last {
                Some(last) => after = last,
                None => break,
            }
        }
        let complete = entries.len() <= usize::from(page_size);
        if !complete {
            entries.truncate(usize::from(page_size));
        }
        let next_cursor = if complete {
            None
        } else {
            let last = entries.last().ok_or(Error::Internal)?;
            Some(format!("v1:{anchor}:{}", last.call_id))
        };
        Ok(BudgetOpenCallsPage {
            schema_version: BUDGET_OPEN_CALLS_PAGE_SCHEMA.into(),
            billing_scope: billing_scope.into(),
            anchor,
            page_size,
            entries,
            next_cursor,
            complete,
        })
    }

    /// Appends one root stage plan version in the caller's Session. The
    /// control registration Session uses this so its relation and the binding
    /// share one commit.
    pub async fn bind_root_stage_plan(
        &mut self,
        ctx: &Context,
        request: &RootStagePlanBindRequest,
    ) -> Result<RootStagePlanBinding> {
        ctx.require(&[Role::Admin])?;
        bind_root_stage_plan_in_tx(&mut self.tx, ctx, request).await
    }

    /// Reads the active (highest-seq) binding in this Session.
    pub async fn active_root_stage_plan(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
    ) -> Result<Option<RootStagePlanBinding>> {
        require_budget_reader(ctx)?;
        identifier(billing_scope)?;
        require_namespace(&mut self.tx, billing_scope, ctx.namespace()).await?;
        active_root_stage_plan_in_tx(&mut self.tx, billing_scope).await
    }

    /// Writes the immutable control-to-active-plan relation in the control's
    /// registration Session. The binding itself never stores a control ref.
    /// `created_at` is the same-Session trusted clock. Relation identity
    /// `billing_scope + binding_seq + plan_digest` is immutable across active
    /// versions; the same writer may update only its own `control_digest`
    /// within one active version (W1 2026-11-08 ruling).
    pub async fn record_root_stage_plan_control(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
        control_kind: &str,
        control_id: &str,
        control_digest: &str,
    ) -> Result<RootStagePlanControlRef> {
        ctx.require(&[Role::Admin, Role::Evaluator])?;
        identifier(billing_scope)?;
        identifier(control_kind)?;
        identifier(control_id)?;
        validate_digest(control_digest, "control_digest")?;
        let trusted_now = evo_core::now()?;
        valid_time(trusted_now)?;
        require_namespace(&mut self.tx, billing_scope, ctx.namespace()).await?;
        let active = active_root_stage_plan_in_tx(&mut self.tx, billing_scope)
            .await?
            .ok_or_else(|| {
                Error::Conflict("registered_control_requires_active_root_stage_plan".into())
            })?;
        if let Some(existing) =
            load_root_stage_plan_control(&mut self.tx, ctx.namespace(), control_kind, control_id)
                .await?
        {
            // W1 ruling (2026-11-08): the relation identity
            // `billing_scope + binding_seq + plan_digest` is immutable. A
            // re-registration that names another active version is refused, so
            // an old control never becomes current again. Within the same
            // active version the same writer may update its own
            // `control_digest`; that is the real re-registration semantic (for
            // example a changed regression threshold) and it does not touch the
            // binding sequence or the plan digest.
            if existing.billing_scope != billing_scope
                || existing.binding_seq != active.seq
                || existing.plan_digest != active.plan_digest
            {
                return Err(Error::Conflict(
                    "root_stage_plan_control_ref_is_immutable".into(),
                ));
            }
            if existing.control_digest == control_digest {
                return Ok(existing);
            }
            sqlx::query(
                "UPDATE root_budget_stage_plan_controls
                 SET control_digest=?,created_at=?
                 WHERE namespace=? AND control_kind=? AND control_id=?",
            )
            .bind(control_digest)
            .bind(trusted_now)
            .bind(ctx.namespace())
            .bind(control_kind)
            .bind(control_id)
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
            return load_root_stage_plan_control(
                &mut self.tx,
                ctx.namespace(),
                control_kind,
                control_id,
            )
            .await?
            .ok_or(Error::Internal);
        }
        sqlx::query(
            "INSERT INTO root_budget_stage_plan_controls(
               billing_scope,namespace,control_kind,control_id,control_digest,
               binding_seq,plan_digest,created_at
             ) VALUES(?,?,?,?,?,?,?,?)",
        )
        .bind(billing_scope)
        .bind(ctx.namespace())
        .bind(control_kind)
        .bind(control_id)
        .bind(control_digest)
        .bind(active.seq)
        .bind(&active.plan_digest)
        .bind(trusted_now)
        .execute(&mut *self.tx)
        .await
        .map_err(internal)?;
        load_root_stage_plan_control(&mut self.tx, ctx.namespace(), control_kind, control_id)
            .await?
            .ok_or(Error::Internal)
    }

    /// Reads the relation written by a control registration. The caller still
    /// compares its `binding_seq`/`plan_digest` with the active version.
    pub async fn root_stage_plan_control_ref(
        &mut self,
        ctx: &Context,
        control_kind: &str,
        control_id: &str,
    ) -> Result<Option<RootStagePlanControlRef>> {
        require_budget_reader(ctx)?;
        identifier(control_kind)?;
        identifier(control_id)?;
        load_root_stage_plan_control(&mut self.tx, ctx.namespace(), control_kind, control_id).await
    }

    /// Appends exactly one `stage_plan_refused` event to a caller-held clean
    /// Session. It is an internal trusted refusal path (`#[doc(hidden)]`): it
    /// writes no group, call, amount or other caller fact and does not commit;
    /// the caller owns the commit. `event_at` is the same-Session trusted clock.
    #[doc(hidden)]
    pub async fn append_stage_plan_refusal(
        &mut self,
        ctx: &Context,
        refusal: &StagePlanRefusal,
    ) -> Result<()> {
        require_budget_caller(ctx)?;
        identifier(&refusal.billing_scope)?;
        let trusted_now = evo_core::now()?;
        valid_time(trusted_now)?;
        if matches!(
            refusal.stage,
            BudgetStage::TaskExecution | BudgetStage::FormalEvaluation
        ) {
            return Err(Error::Invalid(
                "acceptance stages do not carry a stage plan refusal".into(),
            ));
        }
        require_namespace(&mut self.tx, &refusal.billing_scope, ctx.namespace()).await?;
        let active = active_root_stage_plan_in_tx(&mut self.tx, &refusal.billing_scope)
            .await?
            .ok_or_else(|| Error::Conflict("stage_plan_refusal_requires_active_plan".into()))?;
        insert_event(
            &mut self.tx,
            &refusal.billing_scope,
            None,
            STAGE_PLAN_REFUSED_EVENT,
            trusted_now,
            json!({
                "stage": refusal.stage,
                "reason": refusal.kind.code(),
                "binding_seq": active.seq,
            }),
        )
        .await
    }

    pub async fn reserve_budget_call(
        &mut self,
        ctx: &Context,
        request: &BudgetCallReservation,
    ) -> Result<BudgetCallRecord> {
        self.reserve_budget_call_with_sources(ctx, request, &[])
            .await
    }

    pub async fn reserve_budget_call_with_sources(
        &mut self,
        ctx: &Context,
        request: &BudgetCallReservation,
        source_ids: &[String],
    ) -> Result<BudgetCallRecord> {
        self.reserve_budget_call_with_sources_typed(ctx, request, source_ids)
            .await?
            .map_err(BudgetPreDispatchRefusal::into_reservation_error)
    }

    pub async fn reserve_budget_call_typed(
        &mut self,
        ctx: &Context,
        request: &BudgetCallReservation,
    ) -> BudgetPreDispatchResult<BudgetCallRecord> {
        self.reserve_budget_call_with_sources_typed(ctx, request, &[])
            .await
    }

    /// A savepoint on the caller's connection isolates even a late typed
    /// refusal (for example a source-ref collision after tentative insertion).
    /// Success releases only the savepoint: reservation, candidate, proposal,
    /// dependency edges and audit still require the caller's single commit.
    pub async fn reserve_budget_call_with_sources_typed(
        &mut self,
        ctx: &Context,
        request: &BudgetCallReservation,
        source_ids: &[String],
    ) -> BudgetPreDispatchResult<BudgetCallRecord> {
        self.reserve_budget_call_with_stage_plan_typed(
            ctx,
            request,
            source_ids,
            &StagePlanAttribution::Unattributed,
        )
        .await
    }

    /// Reserves a call whose producer names a real persisted control relation.
    /// The stage gate reads that relation and the active version in this same
    /// Session before the first group/call/event/ref write.
    pub async fn reserve_budget_call_with_stage_plan_typed(
        &mut self,
        ctx: &Context,
        request: &BudgetCallReservation,
        source_ids: &[String],
        attribution: &StagePlanAttribution,
    ) -> BudgetPreDispatchResult<BudgetCallRecord> {
        let mut savepoint = (&mut self.tx).begin().await.map_err(internal)?;
        let result = reserve_budget_call_with_sources_in_tx_typed(
            &mut savepoint,
            ctx,
            request,
            source_ids,
            attribution,
        )
        .await;
        match result {
            Ok(Ok(call)) => {
                savepoint.commit().await.map_err(internal)?;
                Ok(Ok(call))
            }
            result => {
                savepoint.rollback().await.map_err(internal)?;
                result
            }
        }
    }

    /// Refences an expired reserved call in the caller's transaction. Success
    /// releases only the savepoint; an error rolls back the lease update and
    /// its event even if the caller later commits the outer Session.
    pub async fn refence_reserved_budget_call(
        &mut self,
        ctx: &Context,
        request: &BudgetCallRefence,
    ) -> Result<BudgetCallRecord> {
        validate_budget_refence(ctx, request)?;
        let mut savepoint = (&mut self.tx).begin().await.map_err(internal)?;
        let result = refence_reserved_budget_call_in_tx(&mut savepoint, ctx, request).await;
        match result {
            Ok(call) => {
                savepoint.commit().await.map_err(internal)?;
                Ok(call)
            }
            Err(error) => {
                savepoint.rollback().await.map_err(internal)?;
                Err(error)
            }
        }
    }

    /// Complete cross-namespace ledger bodies are available only to an actual
    /// Admin who is a member of this root's immutable authorization scope.
    pub async fn root_budget_monetary_snapshot(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
    ) -> Result<RootBudgetMonetarySnapshot> {
        ctx.require(&[Role::Admin])?;
        root_budget_in_tx(&mut self.tx, ctx, billing_scope)
            .await?
            .ok_or(Error::NotFound)?;
        root_budget_monetary_snapshot_in_tx(&mut self.tx, billing_scope).await
    }

    /// Stops new reservations and dispatches on the same transaction fence.
    /// Late settlement, reconciliation and execution closure remain possible.
    pub async fn stop_root_budget(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
        reason: &str,
        now: i64,
    ) -> Result<RootBudgetRecord> {
        stop_root_budget_in_tx(&mut self.tx, ctx, billing_scope, reason, now).await
    }

    /// A seal is an immutable event in the private budget ledger. An unresolved
    /// root is still stopped and recorded, with monetary_complete=false; it
    /// must not be promoted by a consumer. Nothing here commits the Session.
    pub async fn seal_root_budget(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
        reason: &str,
        now: i64,
    ) -> Result<SealedRootBudgetEvidence> {
        stop_root_budget_in_tx(&mut self.tx, ctx, billing_scope, reason, now).await?;
        let snapshot = root_budget_monetary_snapshot_in_tx(&mut self.tx, billing_scope).await?;
        let latest_event = snapshot
            .ledger_events
            .iter()
            .map(|event| event.event_at)
            .max()
            .unwrap_or(snapshot.root.created_at);
        if now < snapshot.root.created_at
            || now < latest_event
            || now < snapshot.root.stop_committed_at.ok_or(Error::Internal)?
        {
            return Err(Error::Conflict(
                "root monetary seal precedes its ledger".into(),
            ));
        }
        let evidence = root_budget_seal_summary(&snapshot, ctx.namespace(), ctx.actor(), now);
        insert_event(
            &mut self.tx,
            billing_scope,
            None,
            ROOT_MONETARY_SEAL_EVENT,
            now,
            serde_json::to_value(&evidence).map_err(internal)?,
        )
        .await?;
        Ok(evidence)
    }

    /// Returns only scoped, currently valid sealed evidence. A changed ledger
    /// requires a real Admin to reseal; no synthetic Admin context is created.
    pub async fn sealed_root_budget_evidence(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
    ) -> Result<Option<SealedRootBudgetEvidence>> {
        let Some(root) = root_budget_in_tx(&mut self.tx, ctx, billing_scope).await? else {
            return Ok(None);
        };
        let row = sqlx::query(
            "SELECT event_at,details FROM root_budget_events WHERE billing_scope=? AND call_id IS NULL AND event_kind=? ORDER BY seq DESC LIMIT 1",
        )
        .bind(billing_scope)
        .bind(ROOT_MONETARY_SEAL_EVENT)
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(internal)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let details: String = row.try_get("details").map_err(internal)?;
        let evidence: SealedRootBudgetEvidence =
            serde_json::from_str(&details).map_err(internal)?;
        if evidence.schema_version != ROOT_MONETARY_SEAL_SCHEMA
            || evidence.root != root
            || !root.stopped
            || !root
                .allowed_namespaces
                .contains(&evidence.sealed_by_namespace)
            || evidence.sealed_at != row.try_get::<i64, _>("event_at").map_err(internal)?
            || evidence.sealed_at < root.stop_committed_at.ok_or(Error::Internal)?
        {
            return Err(Error::Conflict(
                "root monetary seal is stale or invalid".into(),
            ));
        }
        identifier(&evidence.sealed_by_actor)?;
        let snapshot = root_budget_monetary_snapshot_in_tx(&mut self.tx, billing_scope).await?;
        let current = root_budget_seal_summary(
            &snapshot,
            &evidence.sealed_by_namespace,
            &evidence.sealed_by_actor,
            evidence.sealed_at,
        );
        if current != evidence {
            return Err(Error::Conflict(
                "root monetary seal is stale or invalid".into(),
            ));
        }
        Ok(Some(evidence))
    }

    pub async fn redact_budget_call_content(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
        call_id: &str,
        reason: &str,
    ) -> Result<BudgetCallRecord> {
        redact_budget_call_content_in_tx(&mut self.tx, ctx, billing_scope, call_id, reason).await
    }

    /// Same-Session consistency only; this unit result authenticates no receipt/caller.
    pub async fn require_task_execution_call_binding(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
        call_id: &str,
        dispatch_group_id: &str,
        actual_input_digest: &str,
        output_digest: &str,
    ) -> Result<()> {
        let mismatch = || Error::Conflict("task execution call binding mismatch".into());
        identifier(billing_scope)
            .and_then(|()| identifier(call_id))
            .and_then(|()| identifier(dispatch_group_id))
            .and_then(|()| validate_digest(actual_input_digest, "actual_input_digest"))
            .and_then(|()| validate_digest(output_digest, "output_digest"))
            .map_err(|_| mismatch())?;
        let matched: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM root_budget_calls AS c
             WHERE c.billing_scope=? AND c.call_id=? AND c.namespace=?
               AND c.dispatch_group_id=? AND c.stage='task_execution'
               AND c.actual_input_digest=? AND c.dispatch_id IS NOT NULL
               AND c.state IN ('dispatched','uncertain','finalized')
               AND (c.output_digest IS NULL OR c.output_digest=?)
               AND EXISTS(SELECT 1 FROM root_budget_namespaces AS n
                 WHERE n.billing_scope=c.billing_scope AND n.namespace=c.namespace)",
        )
        .bind(billing_scope)
        .bind(call_id)
        .bind(ctx.namespace())
        .bind(dispatch_group_id)
        .bind(actual_input_digest)
        .bind(output_digest)
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(internal)?;
        matched.map(|_| ()).ok_or_else(mismatch)
    }

    pub async fn budget_call(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
        call_id: &str,
    ) -> Result<Option<BudgetCallRecord>> {
        budget_call_in_tx(&mut self.tx, ctx, billing_scope, call_id).await
    }

    pub async fn budget_calls_for_group(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
        dispatch_group_id: &str,
    ) -> Result<Vec<BudgetCallRecord>> {
        budget_calls_for_group_in_tx(&mut self.tx, ctx, billing_scope, dispatch_group_id).await
    }

    /// The billing scopes under which `dispatch_group_id` has call rows in the
    /// caller's namespace, without repeats and in byte order. Read-only; a group
    /// with no calls yields an empty list. Rows of other namespaces are never
    /// returned, even for a group of the same name.
    ///
    /// A dispatch group is keyed by its billing scope, so one group name can hold
    /// calls under several roots. A reader that totals a group under one scope
    /// (`budget_calls_for_group`) uses this to see whether any call of the group
    /// sits under another one.
    pub async fn budget_call_scopes_for_group(
        &mut self,
        ctx: &Context,
        dispatch_group_id: &str,
    ) -> Result<Vec<String>> {
        budget_call_scopes_for_group_in_tx(&mut self.tx, ctx, dispatch_group_id).await
    }

    /// `Store::begin_budget_dispatch` in the caller's transaction, with the same
    /// refusals (`Cancelled` for a revoked source included).
    pub async fn begin_budget_dispatch(
        &mut self,
        ctx: &Context,
        fence: &BudgetCallFence,
    ) -> Result<BudgetDispatchDecision> {
        begin_budget_dispatch_in_tx(&mut self.tx, ctx, fence).await
    }

    pub async fn begin_budget_dispatch_typed(
        &mut self,
        ctx: &Context,
        fence: &BudgetCallFence,
    ) -> BudgetPreDispatchResult<BudgetDispatchDecision> {
        begin_budget_dispatch_in_tx_typed(&mut self.tx, ctx, fence).await
    }

    pub async fn release_undispatched_budget_call(
        &mut self,
        ctx: &Context,
        fence: &BudgetCallFence,
        reason: &str,
    ) -> Result<BudgetCallRecord> {
        release_undispatched_in_tx(&mut self.tx, ctx, fence, reason).await
    }

    /// Persists the dispatch-group stop in the caller's existing transaction.
    /// This lets an evaluation ticket stop and its dispatch barrier commit atomically.
    pub async fn stop_dispatch_group(
        &mut self,
        ctx: &Context,
        billing_scope: &str,
        dispatch_group_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<DispatchGroupRecord> {
        stop_dispatch_group_in_tx(
            &mut self.tx,
            ctx,
            billing_scope,
            dispatch_group_id,
            reason,
            now,
        )
        .await
    }
}

fn validate_budget_refence(ctx: &Context, request: &BudgetCallRefence) -> Result<()> {
    require_budget_caller(ctx)?;
    identifier(&request.billing_scope)?;
    identifier(&request.call_id)?;
    identifier(&request.new_lease_token)?;
    valid_time(request.now)?;
    if request.new_lease_until <= request.now {
        return Err(Error::Invalid("new lease must expire after now".into()));
    }
    Ok(())
}

async fn refence_reserved_budget_call_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    request: &BudgetCallRefence,
) -> Result<BudgetCallRecord> {
    let call = load_call(tx, &request.billing_scope, &request.call_id)
        .await?
        .ok_or(Error::NotFound)?;
    require_call_access(tx, ctx, &call).await?;
    if call.state != BudgetCallState::Reserved
        || call.lease_epoch != request.expected_epoch
        || request.now <= call.lease_until
    {
        return Err(Error::Cancelled);
    }
    evo_core::capacity::CapacityItem::ActiveLease
        .admit(count_active_leases(tx, request.now).await?, 1)?;
    let next_epoch = request
        .expected_epoch
        .checked_add(1)
        .ok_or(Error::Internal)?;
    sqlx::query(
        "UPDATE root_budget_calls SET lease_token=?,lease_epoch=?,lease_until=?
             WHERE billing_scope=? AND call_id=?",
    )
    .bind(&request.new_lease_token)
    .bind(next_epoch)
    .bind(request.new_lease_until)
    .bind(&request.billing_scope)
    .bind(&request.call_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_event(
        tx,
        &request.billing_scope,
        Some(&request.call_id),
        "refenced",
        request.now,
        json!({"lease_epoch": next_epoch, "lease_until": request.new_lease_until}),
    )
    .await?;
    let updated = load_call(tx, &request.billing_scope, &request.call_id)
        .await?
        .ok_or(Error::Internal)?;
    Ok(updated)
}

const ROOT_MONETARY_SNAPSHOT_SCHEMA: &str = "rsia.root_budget_monetary_snapshot.v1";
const ROOT_MONETARY_SEAL_SCHEMA: &str = "rsia.root_budget_monetary_seal.v1";
const ROOT_MONETARY_SEAL_EVENT: &str = "root_monetary_sealed_v1";

fn root_unresolved(
    call: Option<&BudgetCallRecord>,
    reason: RootBudgetUnresolvedReason,
) -> RootBudgetUnresolvedRecord {
    RootBudgetUnresolvedRecord {
        namespace: call.map(|call| call.namespace.clone()),
        call_id: call.map(|call| call.call_id.clone()),
        dispatch_group_id: call.map(|call| call.dispatch_group_id.clone()),
        reason,
        actual_cost_micros: call.and_then(|call| call.actual_cost_micros),
        actual_currency: call.and_then(|call| call.actual_currency.clone()),
        actual_pricing_version: call.and_then(|call| call.actual_pricing_version.clone()),
    }
}

fn add_exact_micros(total: &mut i128, amount: i64) -> Result<()> {
    if amount < 0 {
        return Err(Error::Internal);
    }
    *total = total
        .checked_add(i128::from(amount))
        .ok_or(Error::Internal)?;
    Ok(())
}

/// This private reader does not impersonate an Admin. The public Admin snapshot
/// and the scoped sealed reader establish authorization before calling it.
/// Every page is read from the caller's transaction and from the whole billing
/// scope; neither caller namespace nor dispatch group can narrow the accounting.
async fn root_budget_monetary_snapshot_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
) -> Result<RootBudgetMonetarySnapshot> {
    let row = sqlx::query("SELECT * FROM root_budgets WHERE billing_scope=?")
        .bind(billing_scope)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
        .ok_or(Error::NotFound)?;
    let root = root_from_row(&row, load_namespaces(tx, billing_scope).await?)?;
    if root.allowed_namespaces.is_empty()
        || root.allowed_namespaces.len() > 32
        || !root
            .allowed_namespaces
            .contains(&root.authorizing_namespace)
        || root
            .allowed_namespaces
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(Error::Internal);
    }
    let mut dispatch_groups = Vec::new();
    let mut after = String::new();
    loop {
        let rows = sqlx::query(
            "SELECT * FROM root_budget_dispatch_groups WHERE billing_scope=? AND dispatch_group_id>? ORDER BY dispatch_group_id LIMIT 1000",
        )
        .bind(billing_scope)
        .bind(&after)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let group = dispatch_group_from_row(&row)?;
            after = group.dispatch_group_id.clone();
            dispatch_groups.push(group);
        }
    }
    let mut calls = Vec::new();
    after.clear();
    loop {
        let rows = sqlx::query(
            "SELECT * FROM root_budget_calls WHERE billing_scope=? AND call_id>? ORDER BY call_id LIMIT 1000",
        )
        .bind(billing_scope)
        .bind(&after)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let call = call_from_row(&row)?;
            after = call.call_id.clone();
            calls.push(call);
        }
    }
    let mut ledger_events = Vec::new();
    let mut after_sequence = 0_i64;
    loop {
        // Seals attest the ledger, not earlier seals. Excluding only our own
        // event kind makes an unchanged ledger stable without recursive proof.
        let rows = sqlx::query(
            "SELECT seq,call_id,event_kind,event_at,details FROM root_budget_events WHERE billing_scope=? AND seq>? AND event_kind<>? ORDER BY seq LIMIT 1000",
        )
        .bind(billing_scope)
        .bind(after_sequence)
        .bind(ROOT_MONETARY_SEAL_EVENT)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let event = RootBudgetLedgerEvent {
                sequence: row.try_get("seq").map_err(internal)?,
                call_id: row.try_get("call_id").map_err(internal)?,
                event_kind: row.try_get("event_kind").map_err(internal)?,
                event_at: row.try_get("event_at").map_err(internal)?,
                details: row.try_get("details").map_err(internal)?,
            };
            serde_json::from_str::<serde_json::Value>(&event.details).map_err(internal)?;
            after_sequence = event.sequence;
            ledger_events.push(event);
        }
    }

    let groups: BTreeMap<_, _> = dispatch_groups
        .iter()
        .map(|group| (group.dispatch_group_id.as_str(), group))
        .collect();
    let mut totals: BTreeMap<_, (usize, i128, i128, i128, usize)> = BudgetStage::ALL
        .into_iter()
        .map(|stage| (stage, (0, 0, 0, 0, 0)))
        .collect();
    let mut unresolved = Vec::new();
    if !root.stopped {
        unresolved.push(root_unresolved(
            None,
            RootBudgetUnresolvedReason::RootNotStopped,
        ));
    }
    for group in &dispatch_groups {
        if !root.allowed_namespaces.contains(&group.owner_namespace) {
            let mut issue =
                root_unresolved(None, RootBudgetUnresolvedReason::GroupNamespaceMismatch);
            issue.namespace = Some(group.owner_namespace.clone());
            issue.dispatch_group_id = Some(group.dispatch_group_id.clone());
            unresolved.push(issue);
        }
    }
    for call in &calls {
        let total = totals.get_mut(&call.stage).ok_or(Error::Internal)?;
        total.0 = total.0.checked_add(1).ok_or(Error::Internal)?;
        if !root.allowed_namespaces.contains(&call.namespace) {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::CallNamespaceMismatch,
            ));
        }
        match groups.get(call.dispatch_group_id.as_str()) {
            None => unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::MissingDispatchGroup,
            )),
            Some(group) if group.owner_namespace != call.namespace => {
                unresolved.push(root_unresolved(
                    Some(call),
                    RootBudgetUnresolvedReason::GroupNamespaceMismatch,
                ));
            }
            Some(_) => {}
        }
        let invalid_state = match call.state {
            BudgetCallState::Reserved => call.dispatch_id.is_some() || !call.execution_closed,
            BudgetCallState::Dispatched | BudgetCallState::Uncertain => {
                call.dispatch_id.is_none() || call.dispatched_at.is_none()
            }
            BudgetCallState::Finalized => {
                call.dispatch_id.is_none()
                    || call.dispatched_at.is_none()
                    || call.finalized_at.is_none()
                    || call.provider_request_id.is_none()
                    || call.usage_record_id.is_none()
                    || call.output_digest.is_none()
            }
            BudgetCallState::Released | BudgetCallState::Cancelled => call.dispatch_id.is_some(),
        };
        if invalid_state {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::InvalidCallState,
            ));
        }
        let pending = match call.state {
            BudgetCallState::Reserved => Some(RootBudgetUnresolvedReason::Reserved),
            BudgetCallState::Dispatched => Some(RootBudgetUnresolvedReason::Dispatched),
            BudgetCallState::Uncertain => Some(RootBudgetUnresolvedReason::Uncertain),
            BudgetCallState::Finalized | BudgetCallState::Released | BudgetCallState::Cancelled => {
                None
            }
        };
        if let Some(reason) = pending {
            add_exact_micros(&mut total.3, call.reserved_micros)?;
            unresolved.push(root_unresolved(Some(call), reason));
        }
        if !call.execution_closed {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::ExecutionOpen,
            ));
        }
        let any_charge = call.actual_cost_micros.is_some()
            || call.actual_currency.is_some()
            || call.actual_pricing_version.is_some();
        let complete_charge = call.actual_cost_micros.is_some()
            && call.actual_currency.is_some()
            && call.actual_pricing_version.is_some();
        let charge_matches = complete_charge
            && call.actual_currency.as_deref() == Some(root.currency.as_str())
            && call.actual_pricing_version.as_deref() == Some(root.pricing_version.as_str());
        if any_charge && !complete_charge {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::PartialActualCharge,
            ));
        } else if complete_charge && !charge_matches {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::ChargeIdentityMismatch,
            ));
        }
        if charge_matches {
            let actual = call.actual_cost_micros.ok_or(Error::Internal)?;
            add_exact_micros(&mut total.1, actual)?;
            if call.state == BudgetCallState::Finalized {
                add_exact_micros(&mut total.2, actual)?;
            }
            if actual > call.reserved_micros || actual > root.per_call_cap_micros {
                unresolved.push(root_unresolved(
                    Some(call),
                    RootBudgetUnresolvedReason::CallCostOverrun,
                ));
            }
        }
        let undispatched_terminal = matches!(
            call.state,
            BudgetCallState::Released | BudgetCallState::Cancelled
        ) && call.dispatch_id.is_none();
        if undispatched_terminal && any_charge {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::UnexpectedTerminalCharge,
            ));
        }
        if !charge_matches && !undispatched_terminal {
            total.4 = total.4.checked_add(1).ok_or(Error::Internal)?;
            if !any_charge && call.state != BudgetCallState::Reserved {
                unresolved.push(root_unresolved(
                    Some(call),
                    RootBudgetUnresolvedReason::MissingActualCharge,
                ));
            }
        }
    }
    let mut known_actual = 0_i128;
    let mut finalized_actual = 0_i128;
    let mut held_reservation = 0_i128;
    let mut stage_totals = Vec::with_capacity(BudgetStage::ALL.len());
    for (stage, (call_count, known, finalized, held, unknown_actual_call_count)) in totals {
        known_actual = known_actual.checked_add(known).ok_or(Error::Internal)?;
        finalized_actual = finalized_actual
            .checked_add(finalized)
            .ok_or(Error::Internal)?;
        held_reservation = held_reservation.checked_add(held).ok_or(Error::Internal)?;
        stage_totals.push(RootBudgetStageMonetaryTotal {
            stage,
            call_count,
            known_actual_micros: known.to_string(),
            finalized_actual_micros: finalized.to_string(),
            held_reservation_micros: held.to_string(),
            unknown_actual_call_count,
        });
    }
    if finalized_actual != i128::from(root.spent_micros) {
        unresolved.push(root_unresolved(
            None,
            RootBudgetUnresolvedReason::SpentLedgerMismatch,
        ));
    }
    if held_reservation != i128::from(root.reserved_micros) {
        unresolved.push(root_unresolved(
            None,
            RootBudgetUnresolvedReason::ReservationLedgerMismatch,
        ));
    }
    if known_actual > i128::from(root.total_limit_micros)
        || finalized_actual
            .checked_add(held_reservation)
            .ok_or(Error::Internal)?
            > i128::from(root.total_limit_micros)
    {
        unresolved.push(root_unresolved(
            None,
            RootBudgetUnresolvedReason::AuthorizationLimitExceeded,
        ));
    }
    let snapshot_digest = fingerprint(&(
        ROOT_MONETARY_SNAPSHOT_SCHEMA,
        &root,
        &dispatch_groups,
        &calls,
        &ledger_events,
    ))?;
    Ok(RootBudgetMonetarySnapshot {
        schema_version: ROOT_MONETARY_SNAPSHOT_SCHEMA.into(),
        root,
        stage_totals,
        dispatch_groups,
        calls,
        ledger_events,
        known_actual_micros: known_actual.to_string(),
        finalized_actual_micros: finalized_actual.to_string(),
        held_reservation_micros: held_reservation.to_string(),
        monetary_complete: unresolved.is_empty(),
        unresolved,
        complete_episode_cost: false,
        ordinary_p_eligible: false,
        snapshot_digest,
    })
}

fn root_budget_seal_summary(
    snapshot: &RootBudgetMonetarySnapshot,
    namespace: &str,
    actor: &str,
    now: i64,
) -> SealedRootBudgetEvidence {
    SealedRootBudgetEvidence {
        schema_version: ROOT_MONETARY_SEAL_SCHEMA.into(),
        root: snapshot.root.clone(),
        stage_totals: snapshot.stage_totals.clone(),
        call_count: snapshot.calls.len(),
        dispatch_group_count: snapshot.dispatch_groups.len(),
        ledger_event_count: snapshot.ledger_events.len(),
        known_actual_micros: snapshot.known_actual_micros.clone(),
        finalized_actual_micros: snapshot.finalized_actual_micros.clone(),
        held_reservation_micros: snapshot.held_reservation_micros.clone(),
        unresolved: snapshot.unresolved.clone(),
        monetary_complete: snapshot.monetary_complete,
        complete_episode_cost: false,
        ordinary_p_eligible: false,
        snapshot_digest: snapshot.snapshot_digest.clone(),
        sealed_by_namespace: namespace.into(),
        sealed_by_actor: actor.into(),
        sealed_at: now,
    }
}

/// The real root authorization writer, shared by the Store wrapper and the
/// management Session. It never commits: the caller owns the one transaction.
async fn authorize_root_budget_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    authorization: &RootBudgetAuthorization,
) -> Result<RootBudgetRecord> {
    ctx.require(&[Role::Admin])?;
    let namespaces = validate_authorization(ctx, authorization)?;
    if let Some(row) = sqlx::query("SELECT * FROM root_budgets WHERE billing_scope=?")
        .bind(&authorization.billing_scope)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
    {
        let existing_namespaces = load_namespaces(tx, &authorization.billing_scope).await?;
        let existing = root_from_row(&row, existing_namespaces)?;
        if existing.root_budget_id != authorization.root_budget_id
            || existing.authorizing_namespace != ctx.namespace()
            || existing.allowed_namespaces != namespaces
            || existing.currency != authorization.currency
            || existing.pricing_version != authorization.pricing_version
            || existing.payment_subject != authorization.payment_subject
            || existing.authorization_receipt_digest != authorization.authorization_receipt_digest
            || existing.per_call_cap_micros != authorization.per_call_cap_micros
            || existing.total_limit_micros != authorization.total_limit_micros
        {
            return Err(Error::Conflict(
                "billing_scope_already_bound_to_different_root_or_authorization".into(),
            ));
        }
        return Ok(existing);
    }

    sqlx::query(
        "INSERT INTO root_budgets(
           billing_scope,root_budget_id,authorizing_namespace,currency,pricing_version,
           payment_subject,authorization_receipt_digest,per_call_cap_micros,total_limit_micros,created_at
         ) VALUES(?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&authorization.billing_scope)
    .bind(&authorization.root_budget_id)
    .bind(ctx.namespace())
    .bind(&authorization.currency)
    .bind(&authorization.pricing_version)
    .bind(&authorization.payment_subject)
    .bind(&authorization.authorization_receipt_digest)
    .bind(authorization.per_call_cap_micros)
    .bind(authorization.total_limit_micros)
    .bind(authorization.created_at)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    for namespace in &namespaces {
        sqlx::query("INSERT INTO root_budget_namespaces(billing_scope,namespace) VALUES(?,?)")
            .bind(&authorization.billing_scope)
            .bind(namespace)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
    }
    insert_event(
        tx,
        &authorization.billing_scope,
        None,
        "root_authorized",
        authorization.created_at,
        json!({
            "root_budget_id": authorization.root_budget_id,
            "currency": authorization.currency,
            "pricing_version": authorization.pricing_version,
            "payment_subject": authorization.payment_subject,
            "authorization_receipt_digest": authorization.authorization_receipt_digest,
            "per_call_cap_micros": authorization.per_call_cap_micros,
            "total_limit_micros": authorization.total_limit_micros,
            "allowed_namespaces": namespaces,
        }),
    )
    .await?;
    let row = sqlx::query("SELECT * FROM root_budgets WHERE billing_scope=?")
        .bind(&authorization.billing_scope)
        .fetch_one(&mut **tx)
        .await
        .map_err(internal)?;
    let namespaces = load_namespaces(tx, &authorization.billing_scope).await?;
    root_from_row(&row, namespaces)
}

async fn root_budget_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
) -> Result<Option<RootBudgetRecord>> {
    require_budget_reader(ctx)?;
    identifier(billing_scope)?;
    let Some(row) = sqlx::query("SELECT * FROM root_budgets WHERE billing_scope=?")
        .bind(billing_scope)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
    else {
        return Ok(None);
    };
    let namespaces: Vec<String> = sqlx::query_scalar(
        "SELECT namespace FROM root_budget_namespaces WHERE billing_scope=? ORDER BY namespace",
    )
    .bind(billing_scope)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    if !namespaces.iter().any(|value| value == ctx.namespace()) {
        return Err(Error::NotFound);
    }
    Ok(Some(root_from_row(&row, namespaces)?))
}

async fn stop_root_budget_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    reason: &str,
    now: i64,
) -> Result<RootBudgetRecord> {
    ctx.require(&[Role::Admin])?;
    identifier(billing_scope)?;
    validate_reason(reason)?;
    valid_time(now)?;
    require_namespace(tx, billing_scope, ctx.namespace()).await?;
    let row = sqlx::query("SELECT stopped,stop_reason FROM root_budgets WHERE billing_scope=?")
        .bind(billing_scope)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
        .ok_or(Error::NotFound)?;
    if row.try_get::<i64, _>("stopped").map_err(internal)? == 0 {
        sqlx::query(
                "UPDATE root_budgets SET stopped=1,stop_reason=?,stop_committed_at=? WHERE billing_scope=?",
            )
            .bind(reason)
            .bind(now)
            .bind(billing_scope)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        insert_event(
            tx,
            billing_scope,
            None,
            "root_stopped",
            now,
            json!({"reason": reason}),
        )
        .await?;
    }
    root_budget_in_tx(tx, ctx, billing_scope)
        .await?
        .ok_or(Error::Internal)
}

async fn reserve_budget_call_with_sources_in_tx_typed(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    request: &BudgetCallReservation,
    source_ids: &[String],
    attribution: &StagePlanAttribution,
) -> BudgetPreDispatchResult<BudgetCallRecord> {
    if require_budget_caller(ctx).is_err() {
        return Ok(Err(BudgetPreDispatchRefusal::Unauthorized));
    }
    if let Err(refusal) = validate_reservation_typed(request)? {
        return Ok(Err(refusal));
    }
    let source_ids = match canonical_source_ids_typed(source_ids)? {
        Ok(ids) => ids,
        Err(refusal) => return Ok(Err(refusal)),
    };
    if let Some(existing) = load_call(tx, &request.billing_scope, &request.call_id).await? {
        if !same_reservation(ctx, &existing, request) {
            return Ok(Err(BudgetPreDispatchRefusal::CallIdReused));
        }
        if !source_ids.is_empty() {
            // A historical call whose ref is intact stays a read. Repairing a
            // lost ref is a new content-dependent write and passes the gate first.
            if registered_source_ids(tx, &existing).await?.is_none()
                && source_closure_blocked(tx, ctx.namespace(), &source_ids).await?
            {
                return Ok(Err(BudgetPreDispatchRefusal::SourceRevoked));
            }
            if let Err(refusal) =
                ensure_budget_call_ref_typed(tx, ctx, &existing, &source_ids).await?
            {
                return Ok(Err(refusal));
            }
        }
        return Ok(Ok(existing));
    }
    // The revocation tombstone is committed before its cleanup runs, so it is
    // read here, in the transaction that would write the reservation and its
    // request body: nothing below has run when a source is refused.
    if source_closure_blocked(tx, ctx.namespace(), &source_ids).await? {
        return Ok(Err(BudgetPreDispatchRefusal::SourceRevoked));
    }
    let row = sqlx::query("SELECT * FROM root_budgets WHERE billing_scope=?")
        .bind(&request.billing_scope)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?;
    let Some(row) = row else {
        return Ok(Err(BudgetPreDispatchRefusal::RootAuthorizationMissing));
    };
    if !namespace_allowed(tx, &request.billing_scope, ctx.namespace()).await? {
        return Ok(Err(BudgetPreDispatchRefusal::Unauthorized));
    }
    if row.try_get::<i64, _>("stopped").map_err(internal)? != 0 {
        return Ok(Err(BudgetPreDispatchRefusal::RootStopped));
    }
    let per_call_cap: i64 = row.try_get("per_call_cap_micros").map_err(internal)?;
    let total_limit: i64 = row.try_get("total_limit_micros").map_err(internal)?;
    let spent: i64 = row.try_get("spent_micros").map_err(internal)?;
    let reserved: i64 = row.try_get("reserved_micros").map_err(internal)?;
    if request.max_cost_micros > per_call_cap {
        return Ok(Err(BudgetPreDispatchRefusal::PerCallCapExceeded));
    }
    let committed = spent
        .checked_add(reserved)
        .and_then(|value| value.checked_add(request.max_cost_micros));
    let Some(committed) = committed else {
        return Ok(Err(BudgetPreDispatchRefusal::BudgetArithmeticOverflow));
    };
    if committed > total_limit {
        return Ok(Err(BudgetPreDispatchRefusal::RootBudgetExhausted));
    }
    let group = load_dispatch_group(tx, &request.billing_scope, &request.dispatch_group_id).await?;
    if let Some(group) = &group {
        if group.owner_namespace != ctx.namespace() {
            return Ok(Err(BudgetPreDispatchRefusal::GroupNamespaceMismatch));
        }
        if group.stopped {
            return Ok(Err(BudgetPreDispatchRefusal::GroupStopped));
        }
    }
    let active_leases = count_active_leases(tx, request.now).await?;
    if active_leases >= MVP_MAX_ACTIVE_LEASES {
        return Ok(Err(BudgetPreDispatchRefusal::ActiveLeaseCapacity {
            used: active_leases,
            limit: MVP_MAX_ACTIVE_LEASES,
        }));
    }
    let root_currency: String = row.try_get("currency").map_err(internal)?;
    let root_pricing: String = row.try_get("pricing_version").map_err(internal)?;
    let stage_binding = match stage_plan_admission_in_tx(
        tx,
        ctx,
        request,
        attribution,
        &root_currency,
        &root_pricing,
    )
    .await?
    {
        Ok(binding) => binding,
        Err(refusal) => return Ok(Err(refusal)),
    };
    if group.is_none() {
        sqlx::query(
            "INSERT INTO root_budget_dispatch_groups(
                       billing_scope,dispatch_group_id,owner_namespace,created_at
                     ) VALUES(?,?,?,?)",
        )
        .bind(&request.billing_scope)
        .bind(&request.dispatch_group_id)
        .bind(ctx.namespace())
        .bind(request.now)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        insert_event(
            tx,
            &request.billing_scope,
            None,
            "dispatch_group_created",
            request.now,
            json!({
                "dispatch_group_id": request.dispatch_group_id,
                "owner_namespace": ctx.namespace(),
            }),
        )
        .await?;
    }
    let (request_schema, request_digest, request_body) = request
        .request_artifact
        .as_ref()
        .map(|artifact| {
            (
                Some(artifact.schema_version.as_str()),
                Some(artifact.digest.as_str()),
                Some(artifact.body.as_str()),
            )
        })
        .unwrap_or((None, None, None));
    let Some(next_reserved) = reserved.checked_add(request.max_cost_micros) else {
        return Ok(Err(BudgetPreDispatchRefusal::BudgetArithmeticOverflow));
    };
    sqlx::query("UPDATE root_budgets SET reserved_micros=? WHERE billing_scope=?")
        .bind(next_reserved)
        .bind(&request.billing_scope)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    sqlx::query(
            "INSERT INTO root_budget_calls(
               billing_scope,call_id,dispatch_group_id,namespace,stage,actual_input_digest,
               request_artifact_schema,request_artifact_digest,request_artifact_body,reserved_micros,state,
               lease_token,lease_epoch,lease_until,execution_closed,created_at
             ) VALUES(?,?,?,?,?,?,?,?,?,?,'reserved',?,1,?,1,?)",
        )
        .bind(&request.billing_scope)
        .bind(&request.call_id)
        .bind(&request.dispatch_group_id)
        .bind(ctx.namespace())
        .bind(request.stage.as_str())
        .bind(&request.actual_input_digest)
        .bind(request_schema)
        .bind(request_digest)
        .bind(request_body)
        .bind(request.max_cost_micros)
        .bind(&request.lease_token)
        .bind(request.lease_until)
        .bind(request.now)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    insert_event(
        tx,
        &request.billing_scope,
        Some(&request.call_id),
        "reserved",
        request.now,
        json!({
            "namespace": ctx.namespace(),
            "dispatch_group_id": request.dispatch_group_id,
            "stage": request.stage,
            "actual_input_digest": request.actual_input_digest,
            "reserved_micros": request.max_cost_micros,
            "lease_epoch": 1,
        }),
    )
    .await?;
    let call = load_call(tx, &request.billing_scope, &request.call_id)
        .await?
        .ok_or(Error::Internal)?;
    if let Some((binding_seq, plan_digest)) = stage_binding {
        // The call relation's occurrence time comes from the same-Session
        // trusted clock, not the reservation caller's `now`.
        let relation_now = evo_core::now()?;
        valid_time(relation_now)?;
        sqlx::query(
            "INSERT INTO root_budget_stage_plan_calls(
               billing_scope,call_id,namespace,stage,binding_seq,plan_digest,
               course_producer_ref,created_at
             ) VALUES(?,?,?,?,?,?,NULL,?)",
        )
        .bind(&request.billing_scope)
        .bind(&request.call_id)
        .bind(ctx.namespace())
        .bind(request.stage.as_str())
        .bind(binding_seq)
        .bind(plan_digest)
        .bind(relation_now)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    }
    if !source_ids.is_empty()
        && let Err(refusal) = ensure_budget_call_ref_typed(tx, ctx, &call, &source_ids).await?
    {
        return Ok(Err(refusal));
    }
    Ok(Ok(call))
}

pub(crate) async fn index_budget_call_ref_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    call_id: &str,
    source_ids: &[String],
) -> Result<BudgetCallRef> {
    let source_ids = canonical_source_ids(source_ids)?;
    let call = load_call(tx, billing_scope, call_id)
        .await?
        .ok_or(Error::NotFound)?;
    require_call_access(tx, ctx, &call).await?;
    ensure_budget_call_ref(tx, ctx, &call, &source_ids).await
}

pub(crate) async fn redact_budget_call_content_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    call_id: &str,
    reason: &str,
) -> Result<BudgetCallRecord> {
    redact_budget_call_content_with_block_in_tx(
        tx,
        ctx,
        billing_scope,
        call_id,
        reason,
        SOURCE_REVOKED,
    )
    .await
}

/// New-mode cleanup reports only actual content/flag changes as progress.
pub(crate) async fn redact_e16_budget_call_content_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    call_id: &str,
) -> Result<bool> {
    let call = load_call(tx, billing_scope, call_id)
        .await?
        .ok_or(Error::NotFound)?;
    require_call_access(tx, ctx, &call).await?;
    let needs_redaction = [
        &call.request_artifact,
        &call.transport_artifact,
        &call.response_artifact,
    ]
    .into_iter()
    .any(|artifact| {
        artifact
            .as_ref()
            .is_some_and(|a| a.schema_version != "rsia.redacted.v1")
    }) || call.response_usable == Some(true);
    if !needs_redaction {
        return Ok(false);
    }
    // Keep the settlement's higher-priority pricing/forced/overflow/overrun reason.
    let block_reason = call
        .response_block_reason
        .as_deref()
        .filter(|reason| {
            !matches!(
                *reason,
                "lease_expired"
                    | "root_stopped"
                    | "dispatch_group_stopped"
                    | SOURCE_REVOKED
                    | typed_budget::E16_SOURCE_UNAVAILABLE
            )
        })
        .unwrap_or(typed_budget::E16_SOURCE_UNAVAILABLE);
    redact_budget_call_content_with_block_in_tx(
        tx,
        ctx,
        billing_scope,
        call_id,
        typed_budget::E16_SOURCE_UNAVAILABLE,
        block_reason,
    )
    .await?;
    Ok(true)
}

pub(crate) async fn redact_budget_call_content_with_block_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    call_id: &str,
    reason: &str,
    block_reason: &str,
) -> Result<BudgetCallRecord> {
    require_budget_caller(ctx)?;
    identifier(billing_scope)?;
    identifier(call_id)?;
    text(reason, "budget redaction reason", 512)?;
    let call = load_call(tx, billing_scope, call_id)
        .await?
        .ok_or(Error::NotFound)?;
    require_call_access(tx, ctx, &call).await?;
    let request = redacted_artifact(call.request_artifact.as_ref(), reason)?;
    let transport = redacted_artifact(call.transport_artifact.as_ref(), reason)?;
    let response = redacted_artifact(call.response_artifact.as_ref(), reason)?;
    sqlx::query(
        "UPDATE root_budget_calls SET
           request_artifact_schema=?,request_artifact_digest=?,request_artifact_body=?,
           transport_artifact_schema=?,transport_artifact_digest=?,transport_artifact_body=?,
           response_artifact_schema=?,response_artifact_digest=?,response_artifact_body=?,
           response_usable=CASE WHEN response_artifact_body IS NULL THEN response_usable ELSE 0 END,
           response_block_reason=CASE WHEN response_artifact_body IS NULL THEN response_block_reason ELSE ? END
         WHERE billing_scope=? AND call_id=?",
    )
    .bind(request.as_ref().map(|value| value.schema_version.as_str()))
    .bind(request.as_ref().map(|value| value.digest.as_str()))
    .bind(request.as_ref().map(|value| value.body.as_str()))
    .bind(transport.as_ref().map(|value| value.schema_version.as_str()))
    .bind(transport.as_ref().map(|value| value.digest.as_str()))
    .bind(transport.as_ref().map(|value| value.body.as_str()))
    .bind(response.as_ref().map(|value| value.schema_version.as_str()))
    .bind(response.as_ref().map(|value| value.digest.as_str()))
    .bind(response.as_ref().map(|value| value.body.as_str()))
    .bind(block_reason)
    .bind(billing_scope)
    .bind(call_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_event(
        tx,
        billing_scope,
        Some(call_id),
        "content_redacted",
        evo_core::now()?,
        json!({"reason":reason,"accounting_state_preserved":true}),
    )
    .await?;
    load_call(tx, billing_scope, call_id)
        .await?
        .ok_or(Error::Internal)
}

async fn budget_call_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    call_id: &str,
) -> Result<Option<BudgetCallRecord>> {
    budget_call_in_tx_typed(tx, ctx, billing_scope, call_id)
        .await?
        .map_err(BudgetPreDispatchRefusal::into_dispatch_error)
}

async fn budget_call_in_tx_typed(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    call_id: &str,
) -> BudgetPreDispatchResult<Option<BudgetCallRecord>> {
    if require_budget_reader(ctx).is_err() {
        return Ok(Err(BudgetPreDispatchRefusal::Unauthorized));
    }
    if let Err(error) = identifier(billing_scope).and_then(|()| identifier(call_id)) {
        return reservation_validation_refusal(error);
    }
    let call = load_call(tx, billing_scope, call_id).await?;
    if let Some(call) = &call {
        if !namespace_allowed(tx, billing_scope, ctx.namespace()).await? {
            return Ok(Err(BudgetPreDispatchRefusal::Unauthorized));
        }
        if ctx.role() != Role::Admin && call.namespace != ctx.namespace() {
            return Ok(Err(BudgetPreDispatchRefusal::CallNamespaceMismatch));
        }
    }
    Ok(Ok(call))
}

async fn budget_calls_for_group_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    dispatch_group_id: &str,
) -> Result<Vec<BudgetCallRecord>> {
    require_budget_reader(ctx)?;
    identifier(billing_scope)?;
    identifier(dispatch_group_id)?;
    require_namespace(tx, billing_scope, ctx.namespace()).await?;
    const PAGE_SIZE: i64 = 1_000;
    let mut calls = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let rows = sqlx::query(
            "SELECT * FROM root_budget_calls
             WHERE billing_scope=? AND dispatch_group_id=?
               AND (? IS NULL OR call_id>?)
             ORDER BY call_id LIMIT ?",
        )
        .bind(billing_scope)
        .bind(dispatch_group_id)
        .bind(&cursor)
        .bind(&cursor)
        .bind(PAGE_SIZE)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
        if rows.is_empty() {
            break;
        }
        let row_count = rows.len();
        for row in rows {
            let call = call_from_row(&row)?;
            require_call_access(tx, ctx, &call).await?;
            cursor = Some(call.call_id.clone());
            calls.push(call);
        }
        if row_count < PAGE_SIZE as usize {
            break;
        }
    }
    Ok(calls)
}

async fn budget_call_scopes_for_group_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    dispatch_group_id: &str,
) -> Result<Vec<String>> {
    require_budget_reader(ctx)?;
    identifier(dispatch_group_id)?;
    sqlx::query_scalar(
        "SELECT DISTINCT billing_scope FROM root_budget_calls
         WHERE namespace=? AND dispatch_group_id=?
         ORDER BY billing_scope",
    )
    .bind(ctx.namespace())
    .bind(dispatch_group_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)
}

async fn begin_budget_dispatch_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    fence: &BudgetCallFence,
) -> Result<BudgetDispatchDecision> {
    begin_budget_dispatch_in_tx_typed(tx, ctx, fence)
        .await?
        .map_err(BudgetPreDispatchRefusal::into_dispatch_error)
}

async fn begin_budget_dispatch_in_tx_typed(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    fence: &BudgetCallFence,
) -> BudgetPreDispatchResult<BudgetDispatchDecision> {
    if require_budget_caller(ctx).is_err() {
        return Ok(Err(BudgetPreDispatchRefusal::Unauthorized));
    }
    if let Err(error) = validate_fence(fence) {
        return reservation_validation_refusal(error);
    }
    let call = load_call(tx, &fence.billing_scope, &fence.call_id)
        .await?
        .ok_or(Error::NotFound)?;
    if !namespace_allowed(tx, &call.billing_scope, ctx.namespace()).await? {
        return Ok(Err(BudgetPreDispatchRefusal::Unauthorized));
    }
    if ctx.role() != Role::Admin && call.namespace != ctx.namespace() {
        return Ok(Err(BudgetPreDispatchRefusal::CallNamespaceMismatch));
    }
    // A mismatched epoch/token/digest is still an outer error. It is not the
    // expiry of this lease and must never authorize releasing another lease.
    fence_call(&call, fence, false)?;
    let e16_mode = typed_budget::call_mode_in_tx(tx, &call).await?;
    match e16_mode {
        E16CallMode::Mismatch => return Err(typed_budget::mode_mismatch()),
        E16CallMode::E16 => {
            ctx.require(&[Role::Host, Role::Admin])?;
            if !typed_budget::call_sources_live_in_tx(tx, &call).await? {
                return Err(typed_budget::sources_unavailable());
            }
        }
        E16CallMode::Legacy => {}
    }
    if call.state != BudgetCallState::Reserved {
        return Ok(Ok(BudgetDispatchDecision {
            call,
            new_dispatch: false,
        }));
    }
    if fence.now > call.lease_until {
        return Ok(Err(BudgetPreDispatchRefusal::LeaseExpired));
    }
    let stopped: i64 = sqlx::query_scalar("SELECT stopped FROM root_budgets WHERE billing_scope=?")
        .bind(&fence.billing_scope)
        .fetch_one(&mut **tx)
        .await
        .map_err(internal)?;
    if stopped != 0 {
        return Ok(Err(BudgetPreDispatchRefusal::RootStopped));
    }
    let group_stopped: i64 = sqlx::query_scalar(
        "SELECT stopped FROM root_budget_dispatch_groups
         WHERE billing_scope=? AND dispatch_group_id=?",
    )
    .bind(&fence.billing_scope)
    .bind(&call.dispatch_group_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    if group_stopped != 0 {
        return Ok(Err(BudgetPreDispatchRefusal::GroupStopped));
    }
    // A source revoked after the reservation must not be sent out. The cleanup only
    // redacts the ledger copy of the request; the caller still holds the request
    // itself and would send it, so the revocation is read here, in the transaction
    // that records the dispatch (plan §11: block outbound before the cleanup).
    // `Cancelled` is the decision a caller already releases the reservation on.
    // A legacy call whose reference is missing is judged by the closure its request
    // names, and one whose closure cannot be read is not sent out (plan §7.2.1: a
    // revocation rechecks a queued action before dispatch).
    let source_ids = match e16_mode {
        E16CallMode::Legacy => match legacy_call_sources(tx, &call).await? {
            LegacyCallSources::Named(source_ids) => source_ids,
            LegacyCallSources::Unverifiable => return Err(sources_unverifiable()),
        },
        // An E16 call carries no legacy reference (`call_sources_live_in_tx` refused
        // one above) and its sources were judged above; this read is as it was.
        E16CallMode::E16 | E16CallMode::Mismatch => {
            registered_source_ids(tx, &call).await?.unwrap_or_default()
        }
    };
    if source_closure_blocked(tx, &call.namespace, &source_ids).await? {
        return Ok(Err(BudgetPreDispatchRefusal::SourceRevoked));
    }
    if let Err(refusal) = stage_plan_dispatch_admission_in_tx(tx, &call).await? {
        return Ok(Err(refusal));
    }
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM root_budget_calls
         WHERE billing_scope=? AND call_id<>? AND (
           state IN ('dispatched','uncertain') OR (state='finalized' AND execution_closed=0)
         )",
    )
    .bind(&fence.billing_scope)
    .bind(&fence.call_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    if active != 0 {
        return Ok(Err(BudgetPreDispatchRefusal::RootConcurrencyLimit));
    }
    let dispatch_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "UPDATE root_budget_calls SET state='dispatched',dispatch_id=?,dispatched_at=?,execution_closed=0
         WHERE billing_scope=? AND call_id=?",
    )
    .bind(&dispatch_id)
    .bind(fence.now)
    .bind(&fence.billing_scope)
    .bind(&fence.call_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_event(
        tx,
        &fence.billing_scope,
        Some(&fence.call_id),
        "dispatched",
        fence.now,
        json!({"dispatch_id": dispatch_id, "actual_input_digest": fence.actual_input_digest}),
    )
    .await?;
    let call = load_call(tx, &fence.billing_scope, &fence.call_id)
        .await?
        .ok_or(Error::Internal)?;
    Ok(Ok(BudgetDispatchDecision {
        call,
        new_dispatch: true,
    }))
}

const STAGE_PLAN_REFUSED_EVENT: &str = "stage_plan_refused";

/// The single record kind under which the formal evaluation ticket is
/// persisted as `kind='artifact'`. It is the same literal the engine's
/// `streaming_evaluator::TICKET_KIND` writes and the recovery reader already
/// recognises; storage pins it so an internal caller cannot substitute another
/// artifact record kind for the E05 ticket witness.
const FORMAL_EVALUATION_TICKET_RECORD_KIND: &str = "evaluation_ticket_v2";

fn optimization_stage_for(stage: BudgetStage) -> Option<OptimizationStage> {
    match stage {
        BudgetStage::Reflection => Some(OptimizationStage::Reflection),
        BudgetStage::Merge => Some(OptimizationStage::Merge),
        BudgetStage::Ranking => Some(OptimizationStage::Ranking),
        BudgetStage::DevelopmentExecution => Some(OptimizationStage::DevelopmentExecution),
        BudgetStage::DevelopmentScoring => Some(OptimizationStage::DevelopmentScoring),
        BudgetStage::Practice => Some(OptimizationStage::Practice),
        BudgetStage::Consolidation => Some(OptimizationStage::Consolidation),
        BudgetStage::Guidance => Some(OptimizationStage::Guidance),
        BudgetStage::HistoryCollection => Some(OptimizationStage::HistoryCollection),
        BudgetStage::StorageCpu => Some(OptimizationStage::StorageCpu),
        BudgetStage::HumanReview => Some(OptimizationStage::HumanReview),
        BudgetStage::TaskExecution
        | BudgetStage::CandidateGeneration
        | BudgetStage::FormalEvaluation
        | BudgetStage::Curriculum
        | BudgetStage::MetaEvaluation
        | BudgetStage::GrayOperations => None,
    }
}

/// `(per_call_limit, stage_total)` in exact integer micros. `None` means the
/// stage was not allocated, which is distinct from an allocated zero.
fn plan_stage_limits_micros(
    plan: &RootStageBudgetPlanV1,
    stage: BudgetStage,
) -> Result<Option<(i64, i64)>> {
    let caps = match stage {
        BudgetStage::Curriculum => plan.curriculum.as_ref(),
        BudgetStage::CandidateGeneration => plan.candidate_generation.as_ref(),
        BudgetStage::MetaEvaluation => plan.meta_evaluation.as_ref(),
        BudgetStage::GrayOperations => plan.gray_operations.as_ref(),
        BudgetStage::TaskExecution | BudgetStage::FormalEvaluation => return Ok(None),
        other => {
            let old = optimization_stage_for(other).ok_or(Error::Internal)?;
            return plan
                .optimization_budget
                .stages
                .iter()
                .find(|entry| entry.stage == old)
                .map(|entry| {
                    Ok((
                        money_micros(&entry.per_call_limit, "per_call_limit")?,
                        money_micros(&entry.experiment_total, "experiment_total")?,
                    ))
                })
                .transpose();
        }
    };
    caps.map(|caps| {
        Ok((
            money_micros(&caps.per_call_limit, "per_call_limit")?,
            money_micros(&caps.experiment_total, "experiment_total")?,
        ))
    })
    .transpose()
}

fn binding_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<RootStagePlanBinding> {
    let body: String = row.try_get("plan_body").map_err(internal)?;
    let plan: RootStageBudgetPlanV1 = serde_json::from_str(&body)
        .map_err(|_| Error::Conflict("root_stage_plan_body_invalid".into()))?;
    let plan_schema: String = row.try_get("plan_schema").map_err(internal)?;
    if plan_schema != plan.schema_version {
        return Err(Error::Conflict("root_stage_plan_schema_mismatch".into()));
    }
    Ok(RootStagePlanBinding {
        billing_scope: row.try_get("billing_scope").map_err(internal)?,
        seq: row.try_get("seq").map_err(internal)?,
        plan_digest: row.try_get("plan_digest").map_err(internal)?,
        plan,
        payer_id: row.try_get("payer_id").map_err(internal)?,
        root_total_micros: row.try_get("root_total_micros").map_err(internal)?,
        currency: row.try_get("currency").map_err(internal)?,
        pricing_version: row.try_get("pricing_version").map_err(internal)?,
        authorization_receipt_digest: row
            .try_get("authorization_receipt_digest")
            .map_err(internal)?,
        authorizing_namespace: row.try_get("authorizing_namespace").map_err(internal)?,
        bound_by_actor: row.try_get("bound_by_actor").map_err(internal)?,
        created_at: row.try_get("created_at").map_err(internal)?,
    })
}

fn control_ref_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<RootStagePlanControlRef> {
    Ok(RootStagePlanControlRef {
        billing_scope: row.try_get("billing_scope").map_err(internal)?,
        namespace: row.try_get("namespace").map_err(internal)?,
        control_kind: row.try_get("control_kind").map_err(internal)?,
        control_id: row.try_get("control_id").map_err(internal)?,
        control_digest: row.try_get("control_digest").map_err(internal)?,
        binding_seq: row.try_get("binding_seq").map_err(internal)?,
        plan_digest: row.try_get("plan_digest").map_err(internal)?,
        created_at: row.try_get("created_at").map_err(internal)?,
    })
}

async fn load_root_stage_plan_control(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    control_kind: &str,
    control_id: &str,
) -> Result<Option<RootStagePlanControlRef>> {
    let row = sqlx::query(
        "SELECT * FROM root_budget_stage_plan_controls
         WHERE namespace=? AND control_kind=? AND control_id=?",
    )
    .bind(namespace)
    .bind(control_kind)
    .bind(control_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    row.as_ref().map(control_ref_from_row).transpose()
}

async fn binding_version_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
    seq: i64,
) -> Result<Option<RootStagePlanBinding>> {
    let row = sqlx::query(
        "SELECT * FROM root_budget_stage_plan_bindings
         WHERE billing_scope=? AND seq=?",
    )
    .bind(billing_scope)
    .bind(seq)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    row.as_ref().map(validate_binding_version).transpose()
}

fn validate_binding_version(row: &sqlx::sqlite::SqliteRow) -> Result<RootStagePlanBinding> {
    let binding = binding_from_row(row)?;
    binding
        .plan
        .validate()
        .map_err(|_| Error::Conflict("root_stage_plan_version_unsupported".into()))?;
    let digest = binding
        .plan
        .digest()
        .map_err(|_| Error::Conflict("root_stage_plan_version_unsupported".into()))?;
    if digest != binding.plan_digest {
        return Err(Error::Conflict(
            "root_stage_plan_version_digest_mismatch".into(),
        ));
    }
    let budget = &binding.plan.optimization_budget;
    if budget.payer_id != binding.payer_id
        || budget.root_total.currency != binding.currency
        || budget.root_total.pricing_version != binding.pricing_version
        || money_micros(&budget.root_total, "root_total")? != binding.root_total_micros
        || budget.admin_authorization_receipt_digest.as_deref()
            != Some(binding.authorization_receipt_digest.as_str())
        || binding.billing_scope != budget.root_budget_scope_id
    {
        return Err(Error::Conflict(
            "root_stage_plan_version_metadata_mismatch".into(),
        ));
    }
    Ok(binding)
}

async fn active_root_stage_plan_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
) -> Result<Option<RootStagePlanBinding>> {
    let row = sqlx::query(
        "SELECT * FROM root_budget_stage_plan_bindings
         WHERE billing_scope=? ORDER BY seq DESC LIMIT 1",
    )
    .bind(billing_scope)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some(row) = row else { return Ok(None) };
    let binding = binding_from_row(&row)?;
    binding
        .plan
        .validate()
        .map_err(|_| Error::Conflict("active_root_stage_plan_unsupported".into()))?;
    let digest = binding
        .plan
        .digest()
        .map_err(|_| Error::Conflict("active_root_stage_plan_unsupported".into()))?;
    if digest != binding.plan_digest {
        return Err(Error::Conflict(
            "active_root_stage_plan_digest_mismatch".into(),
        ));
    }
    let budget = &binding.plan.optimization_budget;
    if budget.payer_id != binding.payer_id
        || budget.root_total.currency != binding.currency
        || budget.root_total.pricing_version != binding.pricing_version
        || money_micros(&budget.root_total, "root_total")? != binding.root_total_micros
        || budget.admin_authorization_receipt_digest.as_deref()
            != Some(binding.authorization_receipt_digest.as_str())
        || binding.billing_scope != budget.root_budget_scope_id
    {
        return Err(Error::Conflict(
            "active_root_stage_plan_metadata_mismatch".into(),
        ));
    }
    Ok(Some(binding))
}

async fn bind_root_stage_plan_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    request: &RootStagePlanBindRequest,
) -> Result<RootStagePlanBinding> {
    ctx.require(&[Role::Admin])?;
    identifier(&request.billing_scope)?;
    // Binding facts use the same-Session trusted clock, never a caller time.
    let trusted_now = evo_core::now()?;
    valid_time(trusted_now)?;
    if request.expected_seq < 0 {
        return Err(Error::Invalid(
            "root stage plan expected_seq must be nonnegative".into(),
        ));
    }
    request.plan.validate()?;
    let budget = &request.plan.optimization_budget;
    let row = sqlx::query("SELECT * FROM root_budgets WHERE billing_scope=?")
        .bind(&request.billing_scope)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
        .ok_or(Error::Budget)?;
    let root = root_from_row(&row, load_namespaces(tx, &request.billing_scope).await?)?;
    if root.authorizing_namespace != ctx.namespace() {
        return Err(Error::Forbidden);
    }
    if root.stopped {
        return Err(Error::Conflict("root_budget_stopped".into()));
    }
    if budget.root_budget_scope_id != request.billing_scope
        || root.billing_scope != request.billing_scope
    {
        return Err(Error::Conflict("root_stage_plan_scope_mismatch".into()));
    }
    if budget.payer_id != root.payment_subject {
        return Err(Error::Conflict("root_stage_plan_payer_mismatch".into()));
    }
    if budget.root_total.currency != root.currency
        || budget.root_total.pricing_version != root.pricing_version
    {
        return Err(Error::Conflict(
            "root_stage_plan_currency_or_pricing_mismatch".into(),
        ));
    }
    if money_micros(&budget.root_total, "root_total")? != root.total_limit_micros {
        return Err(Error::Conflict(
            "root_stage_plan_root_total_mismatch".into(),
        ));
    }
    let receipt = budget
        .admin_authorization_receipt_digest
        .as_deref()
        .ok_or(Error::Budget)?;
    if receipt != root.authorization_receipt_digest {
        return Err(Error::Conflict("root_stage_plan_receipt_mismatch".into()));
    }
    let max_seq: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(seq) FROM root_budget_stage_plan_bindings WHERE billing_scope=?",
    )
    .bind(&request.billing_scope)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    let max_seq = max_seq.unwrap_or(0);
    let next_seq = request
        .expected_seq
        .checked_add(1)
        .ok_or_else(|| Error::Conflict("root_stage_plan_seq_overflow".into()))?;
    let digest = request.plan.digest()?;
    let body = serde_json::to_string(&request.plan).map_err(|_| Error::Internal)?;
    if let Some(existing_row) =
        sqlx::query("SELECT * FROM root_budget_stage_plan_bindings WHERE billing_scope=? AND seq=?")
            .bind(&request.billing_scope)
            .bind(next_seq)
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal)?
    {
        let existing_body: String = existing_row.try_get("plan_body").map_err(internal)?;
        let existing = binding_from_row(&existing_row)?;
        if existing_body == body
            && existing.plan_digest == digest
            && existing.payer_id == budget.payer_id
            && existing.root_total_micros == root.total_limit_micros
            && existing.currency == root.currency
            && existing.pricing_version == root.pricing_version
            && existing.authorization_receipt_digest == root.authorization_receipt_digest
            && existing.authorizing_namespace == ctx.namespace()
            && existing.billing_scope == request.billing_scope
        {
            return Ok(existing);
        }
        return Err(Error::Conflict(
            "root_stage_plan_version_already_bound".into(),
        ));
    }
    if request.expected_seq != max_seq {
        return Err(Error::Conflict(
            "root_stage_plan_expected_seq_mismatch".into(),
        ));
    }
    let used: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM root_budget_stage_plan_bindings WHERE billing_scope=?",
    )
    .bind(&request.billing_scope)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    CapacityItem::RootStagePlanVersionPerRoot
        .admit(u64::try_from(used).map_err(|_| Error::Internal)?, 1)?;
    for stage in BudgetStage::ALL {
        if matches!(
            stage,
            BudgetStage::TaskExecution | BudgetStage::FormalEvaluation
        ) {
            continue;
        }
        let occupied = stage_occupied_micros_in_tx(
            tx,
            &request.billing_scope,
            stage,
            &root.currency,
            &root.pricing_version,
        )
        .await?;
        let allocation = plan_stage_limits_micros(&request.plan, stage)?
            .map(|(_, total)| total)
            .unwrap_or(0);
        if occupied > i128::from(allocation) {
            return Err(Error::Conflict(
                "root_stage_plan_below_occupied_stage_usage".into(),
            ));
        }
    }
    let insert = sqlx::query(
        "INSERT INTO root_budget_stage_plan_bindings(
           billing_scope,seq,plan_schema,plan_digest,plan_body,payer_id,root_total_micros,
           currency,pricing_version,authorization_receipt_digest,authorizing_namespace,
           bound_by_actor,created_at
         ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&request.billing_scope)
    .bind(next_seq)
    .bind(&request.plan.schema_version)
    .bind(&digest)
    .bind(&body)
    .bind(&budget.payer_id)
    .bind(root.total_limit_micros)
    .bind(&root.currency)
    .bind(&root.pricing_version)
    .bind(&root.authorization_receipt_digest)
    .bind(ctx.namespace())
    .bind(ctx.actor())
    .bind(trusted_now)
    .execute(&mut **tx)
    .await;
    if let Err(error) = insert {
        if error
            .as_database_error()
            .is_some_and(|error| error.is_unique_violation())
        {
            return Err(Error::Conflict("root_stage_plan_seq_cas_conflict".into()));
        }
        return Err(internal(error));
    }
    let row = sqlx::query(
        "SELECT * FROM root_budget_stage_plan_bindings WHERE billing_scope=? AND seq=?",
    )
    .bind(&request.billing_scope)
    .bind(next_seq)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    binding_from_row(&row)
}

async fn stage_occupied_micros_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
    stage: BudgetStage,
    root_currency: &str,
    root_pricing: &str,
) -> Result<i128> {
    let mut occupied = 0_i128;
    let mut after = String::new();
    loop {
        let rows = sqlx::query(
            "SELECT call_id,state,reserved_micros,actual_cost_micros,actual_currency,
                    actual_pricing_version
             FROM root_budget_calls
             WHERE billing_scope=? AND stage=? AND call_id>?
             ORDER BY call_id LIMIT 1000",
        )
        .bind(billing_scope)
        .bind(stage.as_str())
        .bind(&after)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let call_id: String = row.try_get("call_id").map_err(internal)?;
            after = call_id;
            let state = BudgetCallState::parse(row.try_get("state").map_err(internal)?)?;
            let reserved: i64 = row.try_get("reserved_micros").map_err(internal)?;
            let actual: Option<i64> = row.try_get("actual_cost_micros").map_err(internal)?;
            let currency: Option<String> = row.try_get("actual_currency").map_err(internal)?;
            let pricing: Option<String> =
                row.try_get("actual_pricing_version").map_err(internal)?;
            let charging = match state {
                BudgetCallState::Reserved
                | BudgetCallState::Dispatched
                | BudgetCallState::Uncertain => reserved,
                BudgetCallState::Finalized => match actual {
                    Some(actual)
                        if currency.as_deref() == Some(root_currency)
                            && pricing.as_deref() == Some(root_pricing) =>
                    {
                        actual
                    }
                    _ => reserved,
                },
                BudgetCallState::Released | BudgetCallState::Cancelled => 0,
            };
            occupied = occupied
                .checked_add(i128::from(charging))
                .ok_or(Error::Internal)?;
        }
    }
    Ok(occupied)
}

async fn stage_plan_admission_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    request: &BudgetCallReservation,
    attribution: &StagePlanAttribution,
    root_currency: &str,
    root_pricing: &str,
) -> BudgetPreDispatchResult<Option<(i64, String)>> {
    let Some(active) = active_root_stage_plan_in_tx(tx, &request.billing_scope).await? else {
        return Ok(Ok(None));
    };
    if matches!(
        request.stage,
        BudgetStage::TaskExecution | BudgetStage::FormalEvaluation
    ) {
        // The two acceptance stages are root-only, but they are not exempt on a
        // stage label alone. The engine must pass the ticket it verified in this
        // Session; storage re-reads that persisted ticket before admitting the
        // call. Without the witness the bound root refuses the call.
        let StagePlanAttribution::E05Ticket(witness) = attribution else {
            return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
                stage: request.stage,
                kind: StagePlanRefusalKind::E05Required,
            }));
        };
        if let Err(error) = identifier(&witness.ticket_kind)
            .and_then(|_| identifier(&witness.ticket_id))
            .and_then(|_| identifier(&witness.ticket_storage_id))
            .and_then(|_| validate_digest(&witness.ticket_digest, "ticket_digest"))
        {
            return reservation_validation_refusal(error);
        }
        // The witness may only name the real formal ticket record kind. A
        // caller-supplied kind would otherwise let any artifact with the same
        // payload id stand in for the ticket.
        if witness.ticket_kind != FORMAL_EVALUATION_TICKET_RECORD_KIND {
            return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
                stage: request.stage,
                kind: StagePlanRefusalKind::E05Required,
            }));
        }
        let ticket: Option<String> = sqlx::query_scalar(
            "SELECT body FROM objects
             WHERE namespace=? AND kind='artifact' AND id=? AND json_valid(body)
               AND json_extract(body,'$.record_kind')=?
               AND json_extract(body,'$.payload.id')=?",
        )
        .bind(ctx.namespace())
        .bind(&witness.ticket_storage_id)
        .bind(&witness.ticket_kind)
        .bind(&witness.ticket_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?;
        let Some(body) = ticket else {
            return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
                stage: request.stage,
                kind: StagePlanRefusalKind::E05Required,
            }));
        };
        // Bind the witness to the persisted ticket body itself. The canonical
        // digest is the one cross-side rule in `evo_core`; a typo, an edited
        // envelope or a witness for another ticket no longer matches.
        let body: serde_json::Value = serde_json::from_str(&body).map_err(|_| {
            Error::Conflict("persisted formal evaluation ticket is not valid JSON".into())
        })?;
        let stored_digest = evo_core::canonical_fingerprint(&body)?;
        if stored_digest != witness.ticket_digest {
            return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
                stage: request.stage,
                kind: StagePlanRefusalKind::E05Required,
            }));
        }
        return Ok(Ok(None));
    }
    let selector = match attribution {
        StagePlanAttribution::Unattributed | StagePlanAttribution::E05Ticket(_) => {
            return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
                stage: request.stage,
                kind: StagePlanRefusalKind::Unknown,
            }));
        }
        StagePlanAttribution::Control(selector) => selector,
    };
    if selector.namespace != ctx.namespace() {
        return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
            stage: request.stage,
            kind: StagePlanRefusalKind::Unknown,
        }));
    }
    if let Err(error) =
        identifier(&selector.control_kind).and_then(|_| identifier(&selector.control_id))
    {
        return reservation_validation_refusal(error);
    }
    let Some(relation) = load_root_stage_plan_control(
        tx,
        &selector.namespace,
        &selector.control_kind,
        &selector.control_id,
    )
    .await?
    else {
        return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
            stage: request.stage,
            kind: StagePlanRefusalKind::Unknown,
        }));
    };
    if relation.billing_scope != request.billing_scope
        || relation.binding_seq != active.seq
        || relation.plan_digest != active.plan_digest
    {
        return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
            stage: request.stage,
            kind: StagePlanRefusalKind::Inactive,
        }));
    }
    let Some((per_call, total)) = plan_stage_limits_micros(&active.plan, request.stage)? else {
        return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
            stage: request.stage,
            kind: StagePlanRefusalKind::Unallocated,
        }));
    };
    if total <= 0 {
        return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
            stage: request.stage,
            kind: StagePlanRefusalKind::Unallocated,
        }));
    }
    if request.max_cost_micros > per_call {
        return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
            stage: request.stage,
            kind: StagePlanRefusalKind::PerCallExceeded,
        }));
    }
    let occupied = stage_occupied_micros_in_tx(
        tx,
        &request.billing_scope,
        request.stage,
        root_currency,
        root_pricing,
    )
    .await?;
    let next = occupied
        .checked_add(i128::from(request.max_cost_micros))
        .ok_or(Error::Internal)?;
    if next > i128::from(total) {
        return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
            stage: request.stage,
            kind: StagePlanRefusalKind::Exhausted,
        }));
    }
    Ok(Ok(Some((active.seq, active.plan_digest))))
}

async fn stage_plan_dispatch_admission_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
) -> BudgetPreDispatchResult<()> {
    let Some(active) = active_root_stage_plan_in_tx(tx, &call.billing_scope).await? else {
        return Ok(Ok(()));
    };
    if matches!(
        call.stage,
        BudgetStage::TaskExecution | BudgetStage::FormalEvaluation
    ) {
        return Ok(Ok(()));
    }
    let row = sqlx::query(
        "SELECT binding_seq,plan_digest FROM root_budget_stage_plan_calls
         WHERE billing_scope=? AND call_id=?",
    )
    .bind(&call.billing_scope)
    .bind(&call.call_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some(row) = row else {
        return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
            stage: call.stage,
            kind: StagePlanRefusalKind::Unknown,
        }));
    };
    let binding_seq: i64 = row.try_get("binding_seq").map_err(internal)?;
    let plan_digest: String = row.try_get("plan_digest").map_err(internal)?;
    if binding_seq != active.seq || plan_digest != active.plan_digest {
        return Ok(Err(BudgetPreDispatchRefusal::StagePlan {
            stage: call.stage,
            kind: StagePlanRefusalKind::Inactive,
        }));
    }
    Ok(Ok(()))
}

async fn stage_plan_course_occupied_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
    root_currency: &str,
    root_pricing: &str,
) -> Result<i64> {
    let mut occupied = 0_i128;
    let mut after = String::new();
    loop {
        let rows = sqlx::query(
            "SELECT c.call_id,c.state,c.reserved_micros,c.actual_cost_micros,
                    c.actual_currency,c.actual_pricing_version
             FROM root_budget_stage_plan_calls r
             JOIN root_budget_calls c
               ON c.billing_scope=r.billing_scope AND c.call_id=r.call_id
             WHERE r.billing_scope=? AND r.course_producer_ref IS NOT NULL AND c.call_id>?
             ORDER BY c.call_id LIMIT 1000",
        )
        .bind(billing_scope)
        .bind(&after)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let call_id: String = row.try_get("call_id").map_err(internal)?;
            after = call_id;
            let state = BudgetCallState::parse(row.try_get("state").map_err(internal)?)?;
            let reserved: i64 = row.try_get("reserved_micros").map_err(internal)?;
            let actual: Option<i64> = row.try_get("actual_cost_micros").map_err(internal)?;
            let currency: Option<String> = row.try_get("actual_currency").map_err(internal)?;
            let pricing: Option<String> =
                row.try_get("actual_pricing_version").map_err(internal)?;
            let charging = match state {
                BudgetCallState::Reserved
                | BudgetCallState::Dispatched
                | BudgetCallState::Uncertain => reserved,
                BudgetCallState::Finalized => match actual {
                    Some(actual)
                        if currency.as_deref() == Some(root_currency)
                            && pricing.as_deref() == Some(root_pricing) =>
                    {
                        actual
                    }
                    _ => reserved,
                },
                BudgetCallState::Released | BudgetCallState::Cancelled => 0,
            };
            occupied = occupied
                .checked_add(i128::from(charging))
                .ok_or(Error::Internal)?;
        }
    }
    i64::try_from(occupied)
        .map_err(|_| Error::Conflict("course_stage_occupancy_exceeds_i64".into()))
}

async fn root_stage_plan_report_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
) -> Result<RootStagePlanReport> {
    let row = sqlx::query("SELECT * FROM root_budgets WHERE billing_scope=?")
        .bind(billing_scope)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
        .ok_or(Error::NotFound)?;
    let root = root_from_row(&row, load_namespaces(tx, billing_scope).await?)?;
    let binding = active_root_stage_plan_in_tx(tx, billing_scope).await?;
    let mut stages = Vec::with_capacity(BudgetStage::ALL.len());
    for stage in BudgetStage::ALL {
        if matches!(
            stage,
            BudgetStage::TaskExecution | BudgetStage::FormalEvaluation
        ) {
            continue;
        }
        let limits = match &binding {
            Some(binding) => plan_stage_limits_micros(&binding.plan, stage)?,
            None => None,
        };
        let occupied = stage_occupied_micros_in_tx(
            tx,
            billing_scope,
            stage,
            &root.currency,
            &root.pricing_version,
        )
        .await?;
        stages.push(RootStagePlanStageAllocation {
            stage,
            per_call_limit_micros: limits.map(|(per_call, _)| per_call),
            allocated_micros: limits.map(|(_, total)| total),
            occupied_micros: i64::try_from(occupied)
                .map_err(|_| Error::Conflict("stage_occupancy_exceeds_i64".into()))?,
        });
    }
    let mut counts: BTreeMap<(BudgetStage, String), u64> = BTreeMap::new();
    let mut after_sequence = 0_i64;
    loop {
        let rows = sqlx::query(
            "SELECT seq,details FROM root_budget_events
             WHERE billing_scope=? AND event_kind=? AND seq>? ORDER BY seq LIMIT 1000",
        )
        .bind(billing_scope)
        .bind(STAGE_PLAN_REFUSED_EVENT)
        .bind(after_sequence)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after_sequence = row.try_get("seq").map_err(internal)?;
            let details: String = row.try_get("details").map_err(internal)?;
            let value: serde_json::Value =
                serde_json::from_str(&details).map_err(|_| Error::Internal)?;
            let stage = value
                .get("stage")
                .and_then(|value| value.as_str())
                .ok_or(Error::Internal)?;
            let reason = value
                .get("reason")
                .and_then(|value| value.as_str())
                .ok_or(Error::Internal)?;
            let stage = BudgetStage::parse(stage)?;
            *counts.entry((stage, reason.to_owned())).or_insert(0) += 1;
        }
    }
    let refusals = counts
        .into_iter()
        .map(|((stage, reason), count)| RootStagePlanRefusalCount {
            stage,
            reason,
            count,
        })
        .collect();
    let course_occupied =
        stage_plan_course_occupied_in_tx(tx, billing_scope, &root.currency, &root.pricing_version)
            .await?;
    Ok(RootStagePlanReport {
        schema_version: ROOT_STAGE_PLAN_REPORT_SCHEMA.into(),
        billing_scope: billing_scope.into(),
        enforcement: if binding.is_some() {
            RootStagePlanEnforcement::Enforced
        } else {
            RootStagePlanEnforcement::NoBinding
        },
        binding,
        stages,
        refusals,
        course_share_bps: evo_core::curriculum::CURRICULUM_BUDGET_SHARE_BPS,
        course_occupied_micros: course_occupied,
        course_producer_wired: false,
    })
}

async fn stop_dispatch_group_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    dispatch_group_id: &str,
    reason: &str,
    now: i64,
) -> Result<DispatchGroupRecord> {
    ctx.require(&[Role::Admin, Role::Evaluator])?;
    identifier(billing_scope)?;
    identifier(dispatch_group_id)?;
    validate_reason(reason)?;
    valid_time(now)?;
    require_namespace(tx, billing_scope, ctx.namespace()).await?;
    let changed = match load_dispatch_group(tx, billing_scope, dispatch_group_id).await? {
        Some(group) => {
            if ctx.role() != Role::Admin && group.owner_namespace != ctx.namespace() {
                return Err(Error::NotFound);
            }
            if !group.stopped {
                sqlx::query(
                    "UPDATE root_budget_dispatch_groups
                     SET stopped=1,stop_reason=?,stop_committed_at=?
                     WHERE billing_scope=? AND dispatch_group_id=?",
                )
                .bind(reason)
                .bind(now)
                .bind(billing_scope)
                .bind(dispatch_group_id)
                .execute(&mut **tx)
                .await
                .map_err(internal)?;
                true
            } else {
                false
            }
        }
        None => {
            sqlx::query(
                "INSERT INTO root_budget_dispatch_groups(
                   billing_scope,dispatch_group_id,owner_namespace,stopped,stop_reason,
                   stop_committed_at,created_at
                 ) VALUES(?,?,?,1,?,?,?)",
            )
            .bind(billing_scope)
            .bind(dispatch_group_id)
            .bind(ctx.namespace())
            .bind(reason)
            .bind(now)
            .bind(now)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
            true
        }
    };
    if changed {
        insert_event(
            tx,
            billing_scope,
            None,
            "dispatch_group_stopped",
            now,
            json!({"dispatch_group_id": dispatch_group_id, "reason": reason}),
        )
        .await?;
    }
    load_dispatch_group(tx, billing_scope, dispatch_group_id)
        .await?
        .ok_or(Error::Internal)
}

async fn release_undispatched_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    fence: &BudgetCallFence,
    reason: &str,
) -> Result<BudgetCallRecord> {
    require_budget_caller(ctx)?;
    validate_fence(fence)?;
    validate_reason(reason)?;
    let call = load_call(tx, &fence.billing_scope, &fence.call_id)
        .await?
        .ok_or(Error::NotFound)?;
    require_call_access(tx, ctx, &call).await?;
    fence_call(&call, fence, false)?;
    if call.state == BudgetCallState::Released {
        return Ok(call);
    }
    if call.state != BudgetCallState::Reserved {
        return Err(Error::Conflict(
            "only_undispatched_reservation_can_be_released".into(),
        ));
    }
    release_reserved(tx, &call, BudgetCallState::Released, reason, fence.now).await?;
    load_call(tx, &fence.billing_scope, &fence.call_id)
        .await?
        .ok_or(Error::Internal)
}

async fn release_reserved(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
    target: BudgetCallState,
    reason: &str,
    now: i64,
) -> Result<()> {
    let reserved: i64 =
        sqlx::query_scalar("SELECT reserved_micros FROM root_budgets WHERE billing_scope=?")
            .bind(&call.billing_scope)
            .fetch_one(&mut **tx)
            .await
            .map_err(internal)?;
    let remaining = reserved
        .checked_sub(call.reserved_micros)
        .ok_or(Error::Internal)?;
    sqlx::query("UPDATE root_budgets SET reserved_micros=? WHERE billing_scope=?")
        .bind(remaining)
        .bind(&call.billing_scope)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    let state = match target {
        BudgetCallState::Released => "released",
        BudgetCallState::Cancelled => "cancelled",
        _ => return Err(Error::Internal),
    };
    sqlx::query(
        "UPDATE root_budget_calls SET state=?,terminal_reason=? WHERE billing_scope=? AND call_id=?",
    )
    .bind(state)
    .bind(reason)
    .bind(&call.billing_scope)
    .bind(&call.call_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_event(
        tx,
        &call.billing_scope,
        Some(&call.call_id),
        state,
        now,
        json!({"reason": reason, "released_micros": call.reserved_micros}),
    )
    .await
}

/// The one real cost-reconciliation writer. The caller owns the transaction;
/// the returned outcome distinguishes an applied charge, an idempotent
/// finalized replay and the committed overflow-preservation facts.
async fn reconcile_budget_call_cost_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    call_id: &str,
    charge: &UsageCharge,
    now: i64,
) -> Result<BudgetReconcileOutcome> {
    ctx.require(&[Role::Admin])?;
    identifier(billing_scope)?;
    identifier(call_id)?;
    valid_time(now)?;
    validate_charge(charge)?;
    let call = load_call(tx, billing_scope, call_id)
        .await?
        .ok_or(Error::NotFound)?;
    require_call_access(tx, ctx, &call).await?;
    if call.state == BudgetCallState::Finalized {
        ensure_same_charge(&call, charge)?;
        return Ok(BudgetReconcileOutcome::AlreadyFinalized(call));
    }
    if !matches!(
        call.state,
        BudgetCallState::Dispatched | BudgetCallState::Uncertain
    ) {
        return Err(Error::Conflict(
            "reconcile_requires_dispatched_or_uncertain_call".into(),
        ));
    }
    let accounting_overflow = apply_charge(tx, &call, charge, now, "cost_reconciled").await?;
    let updated = load_call(tx, billing_scope, call_id)
        .await?
        .ok_or(Error::Internal)?;
    if accounting_overflow {
        Ok(BudgetReconcileOutcome::PreservedAccountingOverflow(updated))
    } else {
        Ok(BudgetReconcileOutcome::Applied(updated))
    }
}

async fn apply_charge(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
    charge: &UsageCharge,
    now: i64,
    event_kind: &str,
) -> Result<bool> {
    let root = sqlx::query(
        "SELECT currency,pricing_version,spent_micros,reserved_micros FROM root_budgets WHERE billing_scope=?",
    )
    .bind(&call.billing_scope)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal)?;
    let currency: String = root.try_get("currency").map_err(internal)?;
    let pricing_version: String = root.try_get("pricing_version").map_err(internal)?;
    if currency != charge.currency || pricing_version != charge.pricing_version {
        return Err(Error::Conflict("charge_pricing_identity_mismatch".into()));
    }
    let spent: i64 = root.try_get("spent_micros").map_err(internal)?;
    let reserved: i64 = root.try_get("reserved_micros").map_err(internal)?;
    let Some(next_spent) = spent.checked_add(charge.amount_micros) else {
        preserve_accounting_overflow(tx, call, charge, now).await?;
        return Ok(true);
    };
    let next_reserved = reserved
        .checked_sub(call.reserved_micros)
        .ok_or(Error::Internal)?;
    let overrun = charge.amount_micros > call.reserved_micros;
    sqlx::query(
        "UPDATE root_budgets SET spent_micros=?,reserved_micros=?,
           stopped=CASE WHEN ? THEN 1 ELSE stopped END,
           stop_reason=CASE WHEN ? AND stopped=0 THEN 'cost_overrun' ELSE stop_reason END,
           stop_committed_at=CASE WHEN ? AND stopped=0 THEN ? ELSE stop_committed_at END
         WHERE billing_scope=?",
    )
    .bind(next_spent)
    .bind(next_reserved)
    .bind(overrun)
    .bind(overrun)
    .bind(overrun)
    .bind(now)
    .bind(&call.billing_scope)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    sqlx::query(
        "UPDATE root_budget_calls SET state='finalized',provider_request_id=?,usage_record_id=?,
           output_digest=?,actual_cost_micros=?,actual_currency=?,actual_pricing_version=?,
           finalized_at=?,terminal_reason=?
         WHERE billing_scope=? AND call_id=?",
    )
    .bind(&charge.provider_request_id)
    .bind(&charge.usage_record_id)
    .bind(&charge.output_digest)
    .bind(charge.amount_micros)
    .bind(&charge.currency)
    .bind(&charge.pricing_version)
    .bind(now)
    .bind(overrun.then_some("cost_overrun"))
    .bind(&call.billing_scope)
    .bind(&call.call_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_event(
        tx,
        &call.billing_scope,
        Some(&call.call_id),
        event_kind,
        now,
        json!({
            "provider_request_id": charge.provider_request_id,
            "usage_record_id": charge.usage_record_id,
            "amount_micros": charge.amount_micros,
            "currency": charge.currency,
            "pricing_version": charge.pricing_version,
            "output_digest": charge.output_digest,
            "cost_overrun": overrun,
        }),
    )
    .await?;
    Ok(false)
}

async fn preserve_accounting_overflow(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
    charge: &UsageCharge,
    now: i64,
) -> Result<()> {
    sqlx::query(
        "UPDATE root_budgets SET stopped=1,
           stop_reason=CASE WHEN stopped=0 THEN 'cost_accounting_overflow' ELSE stop_reason END,
           stop_committed_at=CASE WHEN stopped=0 THEN ? ELSE stop_committed_at END
         WHERE billing_scope=?",
    )
    .bind(now)
    .bind(&call.billing_scope)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    sqlx::query(
        "UPDATE root_budget_calls SET state='uncertain',provider_request_id=?,usage_record_id=?,
           output_digest=?,actual_cost_micros=?,actual_currency=?,actual_pricing_version=?,
           terminal_reason='cost_accounting_overflow'
         WHERE billing_scope=? AND call_id=?",
    )
    .bind(&charge.provider_request_id)
    .bind(&charge.usage_record_id)
    .bind(&charge.output_digest)
    .bind(charge.amount_micros)
    .bind(&charge.currency)
    .bind(&charge.pricing_version)
    .bind(&call.billing_scope)
    .bind(&call.call_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_event(
        tx,
        &call.billing_scope,
        Some(&call.call_id),
        "cost_accounting_overflow",
        now,
        json!({
            "provider_request_id": charge.provider_request_id,
            "usage_record_id": charge.usage_record_id,
            "raw_amount_micros": charge.amount_micros,
            "currency": charge.currency,
            "pricing_version": charge.pricing_version,
        }),
    )
    .await
}

async fn preserve_charge_identity_mismatch(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
    charge: &UsageCharge,
    now: i64,
) -> Result<()> {
    sqlx::query(
        "UPDATE root_budget_calls SET state='uncertain',provider_request_id=?,usage_record_id=?,
           output_digest=?,actual_cost_micros=?,actual_currency=?,actual_pricing_version=?,
           terminal_reason='charge_pricing_identity_mismatch'
         WHERE billing_scope=? AND call_id=?",
    )
    .bind(&charge.provider_request_id)
    .bind(&charge.usage_record_id)
    .bind(&charge.output_digest)
    .bind(charge.amount_micros)
    .bind(&charge.currency)
    .bind(&charge.pricing_version)
    .bind(&call.billing_scope)
    .bind(&call.call_id)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    insert_event(
        tx,
        &call.billing_scope,
        Some(&call.call_id),
        "charge_pricing_identity_mismatch",
        now,
        json!({
            "provider_request_id": charge.provider_request_id,
            "usage_record_id": charge.usage_record_id,
            "raw_amount_micros": charge.amount_micros,
            "currency": charge.currency,
            "pricing_version": charge.pricing_version,
        }),
    )
    .await
}

fn validate_authorization(ctx: &Context, value: &RootBudgetAuthorization) -> Result<Vec<String>> {
    identifier(&value.root_budget_id)?;
    identifier(&value.billing_scope)?;
    identifier(&value.currency)?;
    identifier(&value.pricing_version)?;
    identifier(&value.payment_subject)?;
    validate_digest(
        &value.authorization_receipt_digest,
        "authorization_receipt_digest",
    )?;
    valid_time(value.created_at)?;
    if value.per_call_cap_micros <= 0
        || value.total_limit_micros <= 0
        || value.per_call_cap_micros > value.total_limit_micros
    {
        return Err(Error::Invalid(
            "positive total and per-call monetary authorization required".into(),
        ));
    }
    if value.allowed_namespaces.is_empty() || value.allowed_namespaces.len() > 32 {
        return Err(Error::Invalid(
            "allowed_namespaces must contain 1..=32 entries".into(),
        ));
    }
    let mut namespaces = BTreeSet::new();
    for namespace in &value.allowed_namespaces {
        identifier(namespace)?;
        if !namespaces.insert(namespace.clone()) {
            return Err(Error::Invalid("duplicate allowed namespace".into()));
        }
    }
    if !namespaces.contains(ctx.namespace()) {
        return Err(Error::Forbidden);
    }
    Ok(namespaces.into_iter().collect())
}

fn validate_reservation_typed(value: &BudgetCallReservation) -> BudgetPreDispatchResult<()> {
    let fields = (|| {
        identifier(&value.billing_scope)?;
        identifier(&value.call_id)?;
        identifier(&value.dispatch_group_id)?;
        validate_digest(&value.actual_input_digest, "actual_input_digest")?;
        identifier(&value.lease_token)
    })();
    if let Err(error) = fields {
        return reservation_validation_refusal(error);
    }
    if let Some(artifact) = &value.request_artifact
        && let Err(refusal) = artifact.validate_typed()?
    {
        return Ok(Err(refusal));
    }
    if let Err(error) = valid_time(value.now) {
        return reservation_validation_refusal(error);
    }
    if value.max_cost_micros <= 0 {
        return Ok(Err(BudgetPreDispatchRefusal::InvalidRequest {
            message: "reservation must be positive".into(),
        }));
    }
    if value.lease_until <= value.now {
        return Ok(Err(BudgetPreDispatchRefusal::InvalidRequest {
            message: "lease must expire after reservation".into(),
        }));
    }
    Ok(Ok(()))
}

fn validate_fence(value: &BudgetCallFence) -> Result<()> {
    identifier(&value.billing_scope)?;
    identifier(&value.call_id)?;
    validate_digest(&value.actual_input_digest, "actual_input_digest")?;
    identifier(&value.lease_token)?;
    valid_time(value.now)?;
    if value.lease_epoch <= 0 {
        return Err(Error::Invalid("lease epoch must be positive".into()));
    }
    Ok(())
}

fn validate_charge(value: &UsageCharge) -> Result<()> {
    if value.amount_micros < 0 {
        return Err(Error::Invalid("actual cost must be nonnegative".into()));
    }
    identifier(&value.currency)?;
    identifier(&value.pricing_version)?;
    text(&value.provider_request_id, "provider_request_id", 256)?;
    identifier(&value.usage_record_id)?;
    validate_digest(&value.output_digest, "output_digest")
}

fn validate_model_settlement_evidence(value: &ModelCallSettlementEvidence) -> Result<()> {
    if let Some(digest) = &value.actual_model_digest {
        validate_digest(digest, "actual_model_digest")?;
    }
    value.transport_artifact.validate()?;
    if let Some(response) = &value.usable_response {
        response.validate()?;
    }
    value.blocked_response.validate()?;
    if value
        .usable_response
        .as_ref()
        .is_some_and(|response| response.digest == value.blocked_response.digest)
    {
        return Err(Error::Invalid(
            "usable and blocked model responses must be distinct".into(),
        ));
    }
    if let Some(reason) = &value.forced_block_reason {
        validate_reason(reason)?;
    } else if value.usable_response.is_none() {
        return Err(Error::Invalid(
            "usable response is required without a forced block".into(),
        ));
    }
    Ok(())
}

fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(Error::Invalid(format!(
            "{name}: expected lowercase sha256 digest"
        )));
    }
    Ok(())
}

fn canonical_source_ids(values: &[String]) -> Result<Vec<String>> {
    canonical_source_ids_typed(values)?.map_err(BudgetPreDispatchRefusal::into_reservation_error)
}

fn canonical_source_ids_typed(values: &[String]) -> BudgetPreDispatchResult<Vec<String>> {
    let mut output = values.to_vec();
    for value in &output {
        if let Err(error) = identifier(value) {
            return reservation_validation_refusal(error);
        }
    }
    output.sort();
    if output.windows(2).any(|pair| pair[0] == pair[1]) {
        return Ok(Err(BudgetPreDispatchRefusal::DuplicateSource));
    }
    Ok(Ok(output))
}

async fn ensure_budget_call_ref(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    call: &BudgetCallRecord,
    source_ids: &[String],
) -> Result<BudgetCallRef> {
    ensure_budget_call_ref_typed(tx, ctx, call, source_ids)
        .await?
        .map_err(BudgetPreDispatchRefusal::into_reservation_error)
}

async fn ensure_budget_call_ref_typed(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    call: &BudgetCallRecord,
    source_ids: &[String],
) -> BudgetPreDispatchResult<BudgetCallRef> {
    let source_ids = match canonical_source_ids_typed(source_ids)? {
        Ok(ids) => ids,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let digest = fingerprint(&(
        "rsia.budget_call_ref.v1",
        ctx.namespace(),
        &call.billing_scope,
        &call.call_id,
    ))?;
    let reference = BudgetCallRef {
        id: format!("budget-ref-{}", &digest[..32]),
        schema_version: "rsia.budget_call_ref.v1".into(),
        namespace: ctx.namespace().into(),
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        source_ids,
    };
    let old: Option<String> = sqlx::query_scalar(
        "SELECT body FROM objects WHERE namespace=? AND kind='artifact' AND id=?",
    )
    .bind(ctx.namespace())
    .bind(&reference.id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    if let Some(old) = old {
        let old: BudgetCallRef = serde_json::from_str(&old).map_err(internal)?;
        if old != reference {
            return Ok(Err(BudgetPreDispatchRefusal::SourceClosureChanged));
        }
        return Ok(Ok(old));
    }
    sqlx::query("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,'artifact',?,?,?)")
        .bind(ctx.namespace())
        .bind(&reference.id)
        .bind(ctx.actor())
        .bind(serde_json::to_string(&reference).map_err(internal)?)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    for source_id in &reference.source_ids {
        sqlx::query(
            "INSERT OR IGNORE INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id)
             VALUES(?,'artifact',?,'run',?)",
        )
        .bind(ctx.namespace())
        .bind(&reference.id)
        .bind(source_id)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    }
    Ok(Ok(reference))
}

/// Whether new use of the legacy run roots is refused: a revoked or unreadable
/// node anywhere in their complete upstream closure, or a closure that cannot be
/// completed (an isolated namespace, an overflowing walk). The same raw rule as
/// publication and E16 snapshots. A missing or unqualified run is not a
/// revocation: it keeps the reservation's existing refusal order.
async fn source_closure_blocked(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    source_ids: &[String],
) -> Result<bool> {
    let roots: Vec<(&str, &str)> = source_ids.iter().map(|id| ("run", id.as_str())).collect();
    Ok(
        crate::revocation::status_in_tx(tx, namespace, &roots, false).await?
            != crate::revocation::RevocationStatus::Live,
    )
}

/// What the budget call reference of a legacy call registers as its source
/// closure (`registered_call_source_ids`).
pub(crate) enum RegisteredCallSources {
    /// The call has no reference. A missing reference is not an empty closure: a
    /// call reserved without a closure has none, and neither has a call whose
    /// reference was lost or deleted, and the two cannot be told apart here.
    Missing,
    /// The `source_ids` of the reference: the closure registered when the call was
    /// reserved, the authoritative closure of the call. The reference is the one
    /// `ensure_budget_call_ref` writes for this call: its schema, the namespace,
    /// billing scope and call id of the call itself, and source ids that are
    /// identifiers in ascending order without repeats (its id is the key it is read
    /// from, which the `objects` table checks against the body).
    Named(Vec<String>),
    /// A reference that decodes but is not the one `ensure_budget_call_ref` writes
    /// for this call (it names another schema, namespace, billing scope or call, or
    /// its `source_ids` are not identifiers in ascending order without repeats).
    /// The revocation cleanup does not trust it and does not repair it; the legacy
    /// gates read its `source_ids` as before.
    Unqualified(Vec<String>),
    /// A reference is stored and does not decode: it is neither trusted nor guessed
    /// at. The decoding error is kept for the caller's diagnostic.
    Undecodable(serde_json::Error),
}

/// The source closure registered for the call `call_id` of `billing_scope` in
/// `namespace` when it was reserved: the `source_ids` of its budget call
/// reference, whose id is derived exactly as `ensure_budget_call_ref` derives it,
/// from the call's own namespace, billing scope and call id. The cleanup preserves
/// the reference as a historical accounting fact, so it is still readable after the
/// cleanup completes.
///
/// This is the one reader of the reference for the legacy dispatch and settlement
/// gates (`legacy_call_sources`) and for the revocation cleanup's budget scan and
/// its recheck, so the gates and the cleanup judge a call by the same registered
/// closure, and fall back to the persisted request only when there is no reference.
/// It also tells the cleanup whether the stored reference is the call's own
/// (`RegisteredCallSources::Unqualified`), before the cleanup reads its closure.
pub(crate) async fn registered_call_source_ids(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    billing_scope: &str,
    call_id: &str,
) -> Result<RegisteredCallSources> {
    let digest = fingerprint(&("rsia.budget_call_ref.v1", namespace, billing_scope, call_id))?;
    let body: Option<String> = sqlx::query_scalar(
        "SELECT body FROM objects WHERE namespace=? AND kind='artifact' AND id=?",
    )
    .bind(namespace)
    .bind(format!("budget-ref-{}", &digest[..32]))
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some(body) = body else {
        return Ok(RegisteredCallSources::Missing);
    };
    Ok(match serde_json::from_str::<BudgetCallRef>(&body) {
        Ok(reference)
            if reference.schema_version == "rsia.budget_call_ref.v1"
                && reference.namespace == namespace
                && reference.billing_scope == billing_scope
                && reference.call_id == call_id
                && canonical_source_ids(&reference.source_ids)
                    .is_ok_and(|canonical| canonical == reference.source_ids) =>
        {
            RegisteredCallSources::Named(reference.source_ids)
        }
        Ok(reference) => RegisteredCallSources::Unqualified(reference.source_ids),
        Err(error) => RegisteredCallSources::Undecodable(error),
    })
}

/// The source closure registered for `call` when it was reserved
/// (`registered_call_source_ids`), or `None` when there is no reference
/// (`legacy_call_sources` reads the request of such a call). A reference that does
/// not decode fails the dispatch and the settlement with `Internal`, as it does in
/// `ensure_budget_call_ref`: it is neither trusted nor guessed at.
async fn registered_source_ids(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
) -> Result<Option<Vec<String>>> {
    match registered_call_source_ids(tx, &call.namespace, &call.billing_scope, &call.call_id)
        .await?
    {
        RegisteredCallSources::Missing => Ok(None),
        RegisteredCallSources::Named(source_ids)
        | RegisteredCallSources::Unqualified(source_ids) => Ok(Some(source_ids)),
        RegisteredCallSources::Undecodable(error) => Err(internal(error)),
    }
}

/// The sources a legacy call is judged by, at its dispatch and at its model
/// settlement.
enum LegacyCallSources {
    /// Judged with their complete closure (`source_closure_blocked`).
    Named(Vec<String>),
    /// The reference is missing and the request cannot be read for a closure:
    /// whether the call is free of a revoked source cannot be told.
    Unverifiable,
}

/// The sources of a legacy call: its registered closure when it has a budget call
/// reference; otherwise the closure its persisted request names, read by the one
/// reader the revocation cleanup reads request bodies with
/// (`budget_request_source_ids`), so the gates and the cleanup accept the same
/// schemas and read the same fields.
///
/// Every production reservation registers a closure, so a reference missing there
/// was lost or deleted, and it must not shrink the closure to nothing (plan §7.2.1:
/// a revocation rechecks a queued action before dispatch; §11 and §11.5: a late
/// output over a deleted source forms no usable candidate, its cost is still
/// booked). A request the reader cannot read (a schema it does not read, a closure
/// it cannot read, a redaction) is `Unverifiable`. A request that names no source
/// is an empty closure, and so is a call with no request at all: the ledger holds
/// nothing of it that could name a source, and the cleanup has nothing of it to
/// rescan (such calls are reserved without a closure at the library level only;
/// every production reservation carries its request).
async fn legacy_call_sources(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
) -> Result<LegacyCallSources> {
    if let Some(source_ids) = registered_source_ids(tx, call).await? {
        return Ok(LegacyCallSources::Named(source_ids));
    }
    let Some(request) = &call.request_artifact else {
        return Ok(LegacyCallSources::Named(Vec::new()));
    };
    Ok(
        match budget_request_source_ids(&request.schema_version, &request.body) {
            BudgetRequestSources::Named(source_ids) => LegacyCallSources::Named(source_ids),
            BudgetRequestSources::Unknown(_) => LegacyCallSources::Unverifiable,
        },
    )
}

/// The refusal of the dispatch, and the reason a response is blocked with and a
/// redaction is made for at the settlement, of a legacy call whose closure cannot
/// be verified (`LegacyCallSources::Unverifiable`).
const BUDGET_SOURCES_UNVERIFIABLE: &str = "budget_sources_unverifiable";

/// A determinate refusal kept in the outer result: a new refusal variant would
/// widen the typed pre-dispatch API (`BudgetPreDispatchRefusal`) its callers match
/// exhaustively.
fn sources_unverifiable() -> Error {
    Error::Conflict(BUDGET_SOURCES_UNVERIFIABLE.into())
}

/// The reason a response is blocked with, and a redaction is made for, when a source
/// of its call was revoked.
const SOURCE_REVOKED: &str = "source_revoked";

/// The `rsia.redacted.v1` form of `artifact`, naming the schema and the digest of
/// the content it replaces. An artifact that is already a redaction is left as it
/// is: redacting it again would keep only the digest of the first redaction and
/// lose the digest of the content.
fn redacted_artifact(
    artifact: Option<&BudgetArtifact>,
    reason: &str,
) -> Result<Option<BudgetArtifact>> {
    artifact
        .map(|artifact| {
            if artifact.schema_version == "rsia.redacted.v1" {
                return Ok(artifact.clone());
            }
            BudgetArtifact::from_serializable(
                "rsia.redacted.v1",
                &json!({
                    "schema_version":"rsia.redacted.v1",
                    "state":"source_revoked",
                    "reason":reason,
                    "original_schema":artifact.schema_version,
                    "original_digest":artifact.digest,
                }),
            )
        })
        .transpose()
}

fn validate_reason(value: &str) -> Result<()> {
    text(value, "reason", 512)
}

fn valid_time(value: i64) -> Result<()> {
    if value < 0 {
        return Err(Error::Invalid("time must be nonnegative".into()));
    }
    Ok(())
}

fn require_budget_reader(ctx: &Context) -> Result<()> {
    ctx.require(&[Role::Admin, Role::Host, Role::Evaluator, Role::Worker])
}

fn require_budget_caller(ctx: &Context) -> Result<()> {
    ctx.require(&[Role::Admin, Role::Host, Role::Evaluator, Role::Worker])
}

async fn require_call_access(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    call: &BudgetCallRecord,
) -> Result<()> {
    require_namespace(tx, &call.billing_scope, ctx.namespace()).await?;
    if ctx.role() == Role::Admin || call.namespace == ctx.namespace() {
        Ok(())
    } else {
        Err(Error::NotFound)
    }
}

fn fence_call(call: &BudgetCallRecord, fence: &BudgetCallFence, check_expiry: bool) -> Result<()> {
    if call.actual_input_digest != fence.actual_input_digest
        || call.lease_token != fence.lease_token
        || call.lease_epoch != fence.lease_epoch
        || (check_expiry && fence.now > call.lease_until)
    {
        return Err(Error::Cancelled);
    }
    Ok(())
}

fn same_reservation(
    ctx: &Context,
    call: &BudgetCallRecord,
    request: &BudgetCallReservation,
) -> bool {
    call.namespace == ctx.namespace()
        && call.dispatch_group_id == request.dispatch_group_id
        && call.stage == request.stage
        && call.actual_input_digest == request.actual_input_digest
        && call.request_artifact == request.request_artifact
        && call.reserved_micros == request.max_cost_micros
}

fn ensure_same_charge(call: &BudgetCallRecord, charge: &UsageCharge) -> Result<()> {
    if call.provider_request_id.as_deref() != Some(charge.provider_request_id.as_str())
        || call.usage_record_id.as_deref() != Some(charge.usage_record_id.as_str())
        || call.output_digest.as_deref() != Some(charge.output_digest.as_str())
        || call.actual_cost_micros != Some(charge.amount_micros)
        || call.actual_currency.as_deref() != Some(charge.currency.as_str())
        || call.actual_pricing_version.as_deref() != Some(charge.pricing_version.as_str())
    {
        return Err(Error::Conflict(
            "finalized_call_reused_with_different_charge".into(),
        ));
    }
    Ok(())
}

fn ensure_same_model_settlement(
    call: &BudgetCallRecord,
    evidence: &ModelCallSettlementEvidence,
) -> Result<()> {
    let expected_response = if call.response_usable == Some(true) {
        evidence.usable_response.as_ref().ok_or(Error::Internal)?
    } else {
        &evidence.blocked_response
    };
    // Over a revoked source the transport body is stored as its redaction (by the
    // settlement, and by the cleanup alike), and so it is over a closure that cannot
    // be verified (by the settlement); the redaction names the digest of the body it
    // replaces: the same evidence is still the same evidence.
    let same_transport = call.transport_artifact.as_ref() == Some(&evidence.transport_artifact)
        || call.transport_artifact
            == redacted_artifact(Some(&evidence.transport_artifact), SOURCE_REVOKED)?
        || call.transport_artifact
            == redacted_artifact(
                Some(&evidence.transport_artifact),
                BUDGET_SOURCES_UNVERIFIABLE,
            )?;
    if call.execution_provenance != Some(evidence.provenance)
        || call.actual_model_digest != evidence.actual_model_digest
        || !same_transport
        || call.response_artifact.as_ref() != Some(expected_response)
    {
        return Err(Error::Conflict(
            "model settlement reused with different evidence".into(),
        ));
    }
    Ok(())
}

fn settlement_from_call(
    call: BudgetCallRecord,
    accounting_overflow: bool,
) -> Result<ModelCallSettlement> {
    let response_artifact = call.response_artifact.clone().ok_or(Error::Internal)?;
    let response_usable = call.response_usable.ok_or(Error::Internal)?;
    let response_block_reason = call.response_block_reason.clone();
    Ok(ModelCallSettlement {
        call,
        response_artifact,
        response_usable,
        response_block_reason,
        accounting_overflow,
    })
}

async fn require_namespace(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
    namespace: &str,
) -> Result<()> {
    if !namespace_allowed(tx, billing_scope, namespace).await? {
        return Err(Error::Forbidden);
    }
    Ok(())
}

async fn namespace_allowed(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
    namespace: &str,
) -> Result<bool> {
    let allowed: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM root_budget_namespaces WHERE billing_scope=? AND namespace=?",
    )
    .bind(billing_scope)
    .bind(namespace)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(allowed.is_some())
}

// Only called directly on pure request/source/fence validation. Do not reuse
// this conversion on a database, refence, or general storage result.
fn reservation_validation_refusal<T>(error: Error) -> BudgetPreDispatchResult<T> {
    match error {
        Error::Invalid(message) => Ok(Err(BudgetPreDispatchRefusal::InvalidRequest { message })),
        error => Err(error),
    }
}

async fn load_namespaces(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
) -> Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT namespace FROM root_budget_namespaces WHERE billing_scope=? ORDER BY namespace",
    )
    .bind(billing_scope)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)
}

async fn load_dispatch_group(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
    dispatch_group_id: &str,
) -> Result<Option<DispatchGroupRecord>> {
    sqlx::query(
        "SELECT * FROM root_budget_dispatch_groups
         WHERE billing_scope=? AND dispatch_group_id=?",
    )
    .bind(billing_scope)
    .bind(dispatch_group_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?
    .map(|row| dispatch_group_from_row(&row))
    .transpose()
}

pub(crate) async fn load_call(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
    call_id: &str,
) -> Result<Option<BudgetCallRecord>> {
    sqlx::query("SELECT * FROM root_budget_calls WHERE billing_scope=? AND call_id=?")
        .bind(billing_scope)
        .bind(call_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
        .map(|row| call_from_row(&row))
        .transpose()
}

fn root_from_row(
    row: &sqlx::sqlite::SqliteRow,
    namespaces: Vec<String>,
) -> Result<RootBudgetRecord> {
    Ok(RootBudgetRecord {
        root_budget_id: row.try_get("root_budget_id").map_err(internal)?,
        billing_scope: row.try_get("billing_scope").map_err(internal)?,
        authorizing_namespace: row.try_get("authorizing_namespace").map_err(internal)?,
        allowed_namespaces: namespaces,
        currency: row.try_get("currency").map_err(internal)?,
        pricing_version: row.try_get("pricing_version").map_err(internal)?,
        payment_subject: row.try_get("payment_subject").map_err(internal)?,
        authorization_receipt_digest: row
            .try_get("authorization_receipt_digest")
            .map_err(internal)?,
        per_call_cap_micros: row.try_get("per_call_cap_micros").map_err(internal)?,
        total_limit_micros: row.try_get("total_limit_micros").map_err(internal)?,
        spent_micros: row.try_get("spent_micros").map_err(internal)?,
        reserved_micros: row.try_get("reserved_micros").map_err(internal)?,
        stopped: row.try_get::<i64, _>("stopped").map_err(internal)? != 0,
        stop_reason: row.try_get("stop_reason").map_err(internal)?,
        stop_committed_at: row.try_get("stop_committed_at").map_err(internal)?,
        created_at: row.try_get("created_at").map_err(internal)?,
    })
}

fn call_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<BudgetCallRecord> {
    let stage: String = row.try_get("stage").map_err(internal)?;
    let state: String = row.try_get("state").map_err(internal)?;
    Ok(BudgetCallRecord {
        billing_scope: row.try_get("billing_scope").map_err(internal)?,
        call_id: row.try_get("call_id").map_err(internal)?,
        dispatch_group_id: row.try_get("dispatch_group_id").map_err(internal)?,
        namespace: row.try_get("namespace").map_err(internal)?,
        stage: BudgetStage::parse(&stage)?,
        actual_input_digest: row.try_get("actual_input_digest").map_err(internal)?,
        request_artifact: artifact_from_row(row, "request")?,
        reserved_micros: row.try_get("reserved_micros").map_err(internal)?,
        state: BudgetCallState::parse(&state)?,
        lease_token: row.try_get("lease_token").map_err(internal)?,
        lease_epoch: row.try_get("lease_epoch").map_err(internal)?,
        lease_until: row.try_get("lease_until").map_err(internal)?,
        dispatch_id: row.try_get("dispatch_id").map_err(internal)?,
        provider_request_id: row.try_get("provider_request_id").map_err(internal)?,
        usage_record_id: row.try_get("usage_record_id").map_err(internal)?,
        output_digest: row.try_get("output_digest").map_err(internal)?,
        actual_cost_micros: row.try_get("actual_cost_micros").map_err(internal)?,
        actual_currency: row.try_get("actual_currency").map_err(internal)?,
        actual_pricing_version: row.try_get("actual_pricing_version").map_err(internal)?,
        execution_provenance: row
            .try_get::<Option<String>, _>("execution_provenance")
            .map_err(internal)?
            .map(|value| BudgetExecutionProvenance::parse(&value))
            .transpose()?,
        actual_model_digest: row.try_get("actual_model_digest").map_err(internal)?,
        transport_artifact: artifact_from_row(row, "transport")?,
        response_artifact: artifact_from_row(row, "response")?,
        response_usable: row
            .try_get::<Option<i64>, _>("response_usable")
            .map_err(internal)?
            .map(|value| value != 0),
        response_block_reason: row.try_get("response_block_reason").map_err(internal)?,
        execution_closed: row
            .try_get::<i64, _>("execution_closed")
            .map_err(internal)?
            != 0,
        execution_closed_at: row.try_get("execution_closed_at").map_err(internal)?,
        execution_close_reason: row.try_get("execution_close_reason").map_err(internal)?,
        terminal_reason: row.try_get("terminal_reason").map_err(internal)?,
        created_at: row.try_get("created_at").map_err(internal)?,
        dispatched_at: row.try_get("dispatched_at").map_err(internal)?,
        finalized_at: row.try_get("finalized_at").map_err(internal)?,
    })
}

fn artifact_from_row(
    row: &sqlx::sqlite::SqliteRow,
    prefix: &str,
) -> Result<Option<BudgetArtifact>> {
    let schema: Option<String> = row
        .try_get(format!("{prefix}_artifact_schema").as_str())
        .map_err(internal)?;
    let digest: Option<String> = row
        .try_get(format!("{prefix}_artifact_digest").as_str())
        .map_err(internal)?;
    let body: Option<String> = row
        .try_get(format!("{prefix}_artifact_body").as_str())
        .map_err(internal)?;
    match (schema, digest, body) {
        (Some(schema_version), Some(digest), Some(body)) => {
            let artifact = BudgetArtifact {
                schema_version,
                digest,
                body,
            };
            artifact.validate()?;
            Ok(Some(artifact))
        }
        (None, None, None) => Ok(None),
        _ => Err(Error::Internal),
    }
}

fn dispatch_group_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<DispatchGroupRecord> {
    Ok(DispatchGroupRecord {
        billing_scope: row.try_get("billing_scope").map_err(internal)?,
        dispatch_group_id: row.try_get("dispatch_group_id").map_err(internal)?,
        owner_namespace: row.try_get("owner_namespace").map_err(internal)?,
        stopped: row.try_get::<i64, _>("stopped").map_err(internal)? != 0,
        stop_reason: row.try_get("stop_reason").map_err(internal)?,
        stop_committed_at: row.try_get("stop_committed_at").map_err(internal)?,
        created_at: row.try_get("created_at").map_err(internal)?,
    })
}

async fn insert_event(
    tx: &mut Transaction<'_, Sqlite>,
    billing_scope: &str,
    call_id: Option<&str>,
    event_kind: &str,
    event_at: i64,
    details: serde_json::Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO root_budget_events(billing_scope,call_id,event_kind,event_at,details)
         VALUES(?,?,?,?,?)",
    )
    .bind(billing_scope)
    .bind(call_id)
    .bind(event_kind)
    .bind(event_at)
    .bind(details.to_string())
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(())
}
