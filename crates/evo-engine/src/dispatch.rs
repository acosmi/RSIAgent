//! Role-gated model and persistent management dispatch.
mod backup;
use crate::exploration::ExplorationWorldV1;
use crate::revocation_gate;
use crate::streaming_evaluator::{
    IndependentEvaluationControl, IssueTicketRequest, ProtectedHoldoutRecord,
    ProtectedHoldoutRecordV2, RegisteredEvaluationControl, RegisteredEvaluationControlV3,
    StreamingEvaluationStatus,
};
use evo_core::contract::{MODEL_TOOLS, admin_ops, reject_admin_as_model_tool};
#[cfg(test)]
use evo_core::curriculum::ProbeTerminal;
use evo_core::replay::{ReplaySimulationProfile, WorldPartition};
use evo_core::strategy::{ElasticPolicyV1, ExplorationCapsV1};
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier, now};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

const JOB_SCHEMA: &str = "rsia.management_job.v2";
const INPUT_SCHEMA: &str = "rsia.management_private_input.v1";
/// Schema of the tombstone the revocation cleanup leaves in place of a record
/// whose source was revoked (plan §11.5, E08).
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";
/// `ManagementJob::error_code` of a job that could not run because the cleanup
/// redacted its private input after a source of the request was revoked.
const SOURCE_REVOKED_CODE: &str = "source_revoked";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    ModelTool,
    Admin,
}

pub fn classify(name: &str) -> Result<Surface> {
    if MODEL_TOOLS.contains(&name) {
        return Ok(Surface::ModelTool);
    }
    if admin_ops().contains(&name) {
        return Ok(Surface::Admin);
    }
    Err(Error::Invalid("unknown operation".into()))
}

pub fn authorize(ctx: &Context, name: &str) -> Result<Surface> {
    match classify(name)? {
        Surface::ModelTool => {
            reject_admin_as_model_tool(name)?;
            ctx.require(&[Role::Agent, Role::Host])?;
            Ok(Surface::ModelTool)
        }
        Surface::Admin => {
            required_management_role(ctx, name)?;
            Ok(Surface::Admin)
        }
    }
}

fn required_management_role(ctx: &Context, operation: &str) -> Result<()> {
    match operation {
        "experiment.register" | "evaluation.start" | "evaluation.status" => {
            ctx.require(&[Role::Evaluator])
        }
        "exploration.start"
        | "replay.run"
        | "curriculum.step"
        | "meta.start"
        | "optimization.step"
        | "trace.export"
        | "backup.create"
        | "import.register"
        | "import.execute"
        | "asset.stage"
        | "asset.abort"
        | "asset.handoff"
        | "export.prepare"
        | "export.complete"
        | "export.abort"
        | "budget.root.authorize"
        | "budget.stage_plan.bind"
        | "budget.calls.list_open"
        | "budget.call.reconcile" => ctx.require(&[Role::Admin]),
        _ => Err(Error::Invalid("management_operation_not_wired".into())),
    }
}

pub use evo_storage::{
    ManagementAssetExportResultV2, ManagementAssetExportState, ManagementBackupResultV2,
    ManagementBudgetResultV2, ManagementCancelArrival, ManagementDiagnostic,
    ManagementImportResultState, ManagementImportResultV2, ManagementImportStatusCounts,
    ManagementJob, ManagementJobState, ManagementJobV2, ManagementOptimizationCallProducer,
    ManagementOptimizationCallRef, ManagementOptimizationProgressV2,
    ManagementOptimizationResultV2, ManagementOptimizationStageRef, ManagementPartialResultV2,
    ManagementPendingTerminalState, ManagementResult, ManagementResultV2,
    ManagementTraceExportResultV2, VersionedManagementJob,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateManagementInput {
    id: String,
    schema_version: String,
    operation: String,
    payload_digest: String,
    payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentRegisterRequest {
    pub schema_version: String,
    pub request_key: String,
    pub control: RegisteredEvaluationControl,
    pub holdout: ProtectedHoldoutRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentRegisterRequestV2 {
    pub schema_version: String,
    pub request_key: String,
    pub control: RegisteredEvaluationControlV3,
    pub holdout: ProtectedHoldoutRecordV2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationStartRequest {
    pub schema_version: String,
    pub request_key: String,
    pub ticket: IssueTicketRequestWire,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueTicketRequestWire {
    pub ticket_id: String,
    pub registration_id: String,
    pub holdout_id: String,
    pub issued_at_unix_seconds: i64,
}

impl IssueTicketRequestWire {
    fn engine_request(&self) -> IssueTicketRequest {
        IssueTicketRequest {
            ticket_id: self.ticket_id.clone(),
            registration_id: self.registration_id.clone(),
            holdout_id: self.holdout_id.clone(),
            issued_at_unix_seconds: self.issued_at_unix_seconds,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationStatusRequest {
    pub schema_version: String,
    pub request_key: String,
    pub ticket_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayRunRequest {
    pub schema_version: String,
    pub request_key: String,
    pub pool_digest: String,
    pub partition: WorldPartition,
    pub policy: ElasticPolicyV1,
    pub profile: ReplaySimulationProfile,
    pub caps: ExplorationCapsV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurriculumStepRequest {
    pub schema_version: String,
    pub request_key: String,
    pub profile_id: String,
    pub state_id: String,
    pub root_budget_limit_micros: u64,
}

/// `exploration.start`: register an [`ExplorationWorldV1`] and take its
/// first pure decision. Identity comes from the authenticated caller; the
/// payload carries no actor, role or namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorationStartRequest {
    pub schema_version: String,
    pub request_key: String,
    pub world: ExplorationWorldV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockedManagementRequest {
    pub schema_version: String,
    pub request_key: String,
}

/// `trace.export` prepare: the strict Admin request that freezes the exact
/// candidate. It carries no events, roles, exporter identity, coverage,
/// booleans or absolute paths — only single-segment identifiers of sources
/// the real authenticated loader already captured.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceExportPrepareRequest {
    pub schema_version: String,
    pub request_key: String,
    pub export_id: String,
    pub source_ids: Vec<String>,
}

/// `trace.export` publish: a new request key referencing the same export and
/// the exact frozen candidate digest. The candidate bytes are never sent, and
/// no regenerated candidate is ever accepted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceExportPublishRequest {
    pub schema_version: String,
    pub request_key: String,
    pub export_id: String,
    pub candidate_digest: String,
}

/// Ordinary data and original references; never a decoded claim or Context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizationStepManagementRequest {
    pub schema_version: String,
    pub request_key: String,
    pub compiler: crate::formal_authority::CompilerInputRequest,
    pub source_selection_ref: String,
    pub edit_template_ref: String,
    pub improver_patch_ref: String,
    pub development_control_id: String,
}

const BUDGET_ROOT_AUTHORIZE_SCHEMA: &str = "rsia.management.budget_root_authorize.v1";
const BUDGET_STAGE_PLAN_BIND_SCHEMA: &str = "rsia.management.budget_stage_plan_bind.v1";
const BUDGET_CALLS_LIST_OPEN_SCHEMA: &str = "rsia.management.budget_calls_list_open.v1";
const BUDGET_CALL_RECONCILE_SCHEMA: &str = "rsia.management.budget_call_reconcile.v1";
const BUDGET_LIST_OPEN_DEFAULT_PAGE_SIZE: u16 = 50;

fn default_budget_list_open_page_size() -> u16 {
    BUDGET_LIST_OPEN_DEFAULT_PAGE_SIZE
}

/// The complete trusted root authorization minus its timestamp: `created_at`
/// is always sampled by the service in the writing Session. The body cannot
/// select an actor, role, namespace, time or lease.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetRootAuthorizeRequest {
    schema_version: String,
    request_key: String,
    root_budget_id: String,
    billing_scope: String,
    allowed_namespaces: Vec<String>,
    currency: String,
    pricing_version: String,
    payment_subject: String,
    authorization_receipt_digest: String,
    per_call_cap_micros: i64,
    total_limit_micros: i64,
}

/// One AG-141 root stage plan version bind. The plan type is the real
/// `RootStageBudgetPlanV1`; no second plan family is introduced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetStagePlanBindManagementRequest {
    schema_version: String,
    request_key: String,
    billing_scope: String,
    expected_seq: i64,
    plan: evo_core::evaluation::RootStageBudgetPlanV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetCallsListOpenRequest {
    schema_version: String,
    request_key: String,
    billing_scope: String,
    #[serde(default = "default_budget_list_open_page_size")]
    page_size: u16,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetCallReconcileRequest {
    schema_version: String,
    request_key: String,
    billing_scope: String,
    call_id: String,
    provider_request_id: String,
    usage_record_id: String,
    output_digest: String,
    amount_micros: i64,
    currency: String,
    pricing_version: String,
}

const IMPORT_REGISTER_SCHEMA: &str = "rsia.management.import_register.v1";
const IMPORT_EXECUTE_SCHEMA: &str = "rsia.management.import_execute.v1";

/// One strict `import.register` source: a logical id, the digest of an
/// existing namespace-scoped registered blob and the pinned reader. Roots,
/// paths, purpose, retention, outbound flags, roles and owners are not part of
/// the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRegisterSource {
    source_id: String,
    blob_digest: String,
    reader: crate::import::SourceFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRegisterManagementRequest {
    schema_version: String,
    request_key: String,
    sources: Vec<ImportRegisterSource>,
}

/// `import.execute`: one existing selection of the caller's own namespace.
/// The result identity is derived from the selection, never sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportExecuteManagementRequest {
    schema_version: String,
    request_key: String,
    selection_id: String,
}

const ASSET_STAGE_SCHEMA: &str = "rsia.management.asset_stage.v1";
const ASSET_ABORT_SCHEMA: &str = "rsia.management.asset_abort.v1";
const ASSET_HANDOFF_SCHEMA: &str = "rsia.management.asset_handoff.v1";
const EXPORT_PREPARE_SCHEMA: &str = "rsia.management.export_prepare.v1";
const EXPORT_COMPLETE_SCHEMA: &str = "rsia.management.export_complete.v1";
const EXPORT_ABORT_SCHEMA: &str = "rsia.management.export_abort.v1";

/// `asset.stage`: the digest of one existing namespace-scoped controlled blob
/// holding the strict `rsia.management.asset_stage_input.v1` projection. No
/// physical root, destination, role, owner or authority boolean is accepted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetStageManagementRequest {
    schema_version: String,
    request_key: String,
    input_blob_digest: String,
}

/// `asset.abort`: one existing staged asset of the caller's namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetAbortManagementRequest {
    schema_version: String,
    request_key: String,
    staged_asset_id: String,
}

/// `asset.handoff`: one staged asset and one existing Current candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetHandoffManagementRequest {
    schema_version: String,
    request_key: String,
    staged_asset_id: String,
    candidate_id: String,
}

/// `export.prepare`: the staged asset whose local projection is frozen. The
/// destination is always the server's fixed `local_namespace`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportPrepareManagementRequest {
    schema_version: String,
    request_key: String,
    staged_asset_id: String,
}

/// `export.complete` and `export.abort`: one existing v2 export attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportAttemptManagementRequest {
    schema_version: String,
    request_key: String,
    export_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "operation")]
enum ParsedRequest {
    ExperimentRegister(Box<ExperimentRegisterRequest>),
    #[serde(rename = "experiment_register")]
    ExperimentRegisterCurrent(Box<ExperimentRegisterRequestV2>),
    EvaluationStart(EvaluationStartRequest),
    EvaluationStatus(EvaluationStatusRequest),
    ReplayRun(Box<ReplayRunRequest>),
    CurriculumStep(CurriculumStepRequest),
    ExplorationStart(Box<ExplorationStartRequest>),
    Blocked(BlockedManagementRequest),
    OptimizationStep(Box<OptimizationStepManagementRequest>),
    TraceExportPrepare(Box<TraceExportPrepareRequest>),
    TraceExportPublish(Box<TraceExportPublishRequest>),
    BackupCreate(backup::BackupCreateRequest),
    BudgetRootAuthorize(Box<BudgetRootAuthorizeRequest>),
    BudgetStagePlanBind(Box<BudgetStagePlanBindManagementRequest>),
    BudgetCallsListOpen(Box<BudgetCallsListOpenRequest>),
    BudgetCallReconcile(Box<BudgetCallReconcileRequest>),
    ImportRegister(Box<ImportRegisterManagementRequest>),
    ImportExecute(Box<ImportExecuteManagementRequest>),
    AssetStage(Box<AssetStageManagementRequest>),
    AssetAbort(Box<AssetAbortManagementRequest>),
    AssetHandoff(Box<AssetHandoffManagementRequest>),
    ExportPrepare(Box<ExportPrepareManagementRequest>),
    ExportComplete(Box<ExportAttemptManagementRequest>),
    ExportAbort(Box<ExportAttemptManagementRequest>),
}

impl<'de> Deserialize<'de> for ParsedRequest {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let mut value = Value::deserialize(deserializer)?;
        if value["operation"].as_str() == Some("experiment_register") {
            value
                .as_object_mut()
                .ok_or_else(|| serde::de::Error::custom("invalid management payload"))?
                .remove("operation");
            return parse_request("experiment.register", value).map_err(serde::de::Error::custom);
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case", tag = "operation")]
        enum Other {
            EvaluationStart(EvaluationStartRequest),
            EvaluationStatus(EvaluationStatusRequest),
            ReplayRun(Box<ReplayRunRequest>),
            CurriculumStep(CurriculumStepRequest),
            ExplorationStart(Box<ExplorationStartRequest>),
            Blocked(BlockedManagementRequest),
            OptimizationStep(Box<OptimizationStepManagementRequest>),
            TraceExportPrepare(Box<TraceExportPrepareRequest>),
            TraceExportPublish(Box<TraceExportPublishRequest>),
            BackupCreate(backup::BackupCreateRequest),
            BudgetRootAuthorize(Box<BudgetRootAuthorizeRequest>),
            BudgetStagePlanBind(Box<BudgetStagePlanBindManagementRequest>),
            BudgetCallsListOpen(Box<BudgetCallsListOpenRequest>),
            BudgetCallReconcile(Box<BudgetCallReconcileRequest>),
            ImportRegister(Box<ImportRegisterManagementRequest>),
            ImportExecute(Box<ImportExecuteManagementRequest>),
            AssetStage(Box<AssetStageManagementRequest>),
            AssetAbort(Box<AssetAbortManagementRequest>),
            AssetHandoff(Box<AssetHandoffManagementRequest>),
            ExportPrepare(Box<ExportPrepareManagementRequest>),
            ExportComplete(Box<ExportAttemptManagementRequest>),
            ExportAbort(Box<ExportAttemptManagementRequest>),
        }
        Ok(
            match serde_json::from_value::<Other>(value).map_err(serde::de::Error::custom)? {
                Other::EvaluationStart(value) => Self::EvaluationStart(value),
                Other::EvaluationStatus(value) => Self::EvaluationStatus(value),
                Other::ReplayRun(value) => Self::ReplayRun(value),
                Other::CurriculumStep(value) => Self::CurriculumStep(value),
                Other::ExplorationStart(value) => Self::ExplorationStart(value),
                Other::Blocked(value) => Self::Blocked(value),
                Other::OptimizationStep(value) => Self::OptimizationStep(value),
                Other::TraceExportPrepare(value) => Self::TraceExportPrepare(value),
                Other::TraceExportPublish(value) => Self::TraceExportPublish(value),
                Other::BackupCreate(value) => Self::BackupCreate(value),
                Other::BudgetRootAuthorize(value) => Self::BudgetRootAuthorize(value),
                Other::BudgetStagePlanBind(value) => Self::BudgetStagePlanBind(value),
                Other::BudgetCallsListOpen(value) => Self::BudgetCallsListOpen(value),
                Other::BudgetCallReconcile(value) => Self::BudgetCallReconcile(value),
                Other::ImportRegister(value) => Self::ImportRegister(value),
                Other::ImportExecute(value) => Self::ImportExecute(value),
                Other::AssetStage(value) => Self::AssetStage(value),
                Other::AssetAbort(value) => Self::AssetAbort(value),
                Other::AssetHandoff(value) => Self::AssetHandoff(value),
                Other::ExportPrepare(value) => Self::ExportPrepare(value),
                Other::ExportComplete(value) => Self::ExportComplete(value),
                Other::ExportAbort(value) => Self::ExportAbort(value),
            },
        )
    }
}

#[derive(Clone)]
pub struct ManagementDispatcher {
    store: Store,
    trusted_contexts: Arc<Vec<Context>>,
    recovery_started: Arc<AtomicBool>,
    active_workers: Arc<Mutex<BTreeSet<String>>>,
    optimization_runtime: Option<Arc<crate::optimization::ManagementOptimizationRuntime>>,
}

impl ManagementDispatcher {
    pub fn new(store: Store, trusted_contexts: Vec<Context>) -> Result<Self> {
        Self::with_optimization_runtime(store, trusted_contexts, None)
    }

    /// Fixed by trusted startup, never selected by the public payload.
    pub fn with_optimization_runtime(
        store: Store,
        trusted_contexts: Vec<Context>,
        runtime: Option<crate::optimization::ManagementOptimizationRuntime>,
    ) -> Result<Self> {
        let mut seen = BTreeSet::new();
        let mut unique = Vec::new();
        for context in trusted_contexts {
            if !matches!(context.role(), Role::Admin | Role::Evaluator) {
                return Err(Error::Forbidden);
            }
            let key = format!(
                "{}\0{}\0{:?}",
                context.namespace(),
                context.actor(),
                context.role()
            );
            if seen.insert(key) {
                unique.push(context);
            }
        }
        Ok(Self {
            store,
            trusted_contexts: Arc::new(unique),
            recovery_started: Arc::new(AtomicBool::new(false)),
            active_workers: Arc::new(Mutex::new(BTreeSet::new())),
            optimization_runtime: runtime.map(Arc::new),
        })
    }

    /// Called by the trusted startup path after its data-directory writer lock.
    /// Running/Draining records are reconciled, never passed to execute_request.
    pub async fn recover_pending(&self) -> Result<usize> {
        if self.recovery_started.swap(true, Ordering::AcqRel) {
            return Ok(0);
        }
        let result = self.recover_pending_once().await;
        if result.is_err() {
            self.recovery_started.store(false, Ordering::Release);
        }
        result
    }

    async fn recover_pending_once(&self) -> Result<usize> {
        let mut pending = Vec::new();
        for context in self.trusted_contexts.iter() {
            let mut session = self.store.session().await?;
            let mut cursor = None;
            let mut seen = 0_usize;
            loop {
                let rows = session.object_rows_page(context, "job", cursor).await?;
                if rows.is_empty() {
                    break;
                }
                seen += rows.len();
                if seen > 10000 {
                    return Err(Error::Conflict("scope collection exceeds bounded scan; archive or add paginated query before continuing".into()));
                }
                for row in &rows {
                    let Some(record) =
                        evo_storage::decode_management_job_for_recovery(&row.id, &row.body)?
                    else {
                        continue;
                    };
                    let identity = record.identity();
                    if identity.owner_actor != context.actor()
                        || identity.owner_role != context.role()
                    {
                        continue;
                    }
                    if row.owner != identity.owner_actor || row.id != identity.id {
                        return Err(Error::Conflict(
                            "management job physical binding differs".into(),
                        ));
                    }
                    if is_terminal(identity.state) {
                        continue;
                    }
                    if self
                        .active_workers
                        .lock()
                        .map_err(|_| Error::Internal)?
                        .contains(identity.id)
                    {
                        continue;
                    }
                    pending.push((context.clone(), record));
                }
                cursor = rows.last().map(|row| row.rowid);
            }
            session.commit().await?;
        }
        let count = pending.len();
        for (ctx, record) in pending {
            let identity = record.identity();
            if identity.state == ManagementJobState::Queued
                && record.generation() == 0
                && identity.lease_token.is_none()
                && record.result().is_none()
            {
                self.spawn_job(identity.id.to_owned())?;
            } else {
                // This fixed startup entry, not lease expiry or an empty call
                // query, establishes that the previous process has ended.
                let old = record.into_unfinished_v2()?;
                self.spawn_reconciliation(ctx, old, ObservedWorkerExit::Restarted)?;
            }
        }
        Ok(count)
    }

    pub async fn submit(
        &self,
        ctx: &Context,
        operation: &str,
        payload: Value,
    ) -> Result<VersionedManagementJob> {
        self.require_trusted(ctx)?;
        let job = persist_management(ctx, &self.store, operation, payload).await?;
        if !is_terminal(job.identity().state) {
            self.spawn_job(job.identity().id.to_owned())?;
        }
        Ok(job)
    }

    pub async fn status(&self, ctx: &Context, job_id: &str) -> Result<VersionedManagementJob> {
        self.require_trusted(ctx)?;
        let mut job = load_job_record(ctx, &self.store, job_id).await?;
        if job.identity().state == ManagementJobState::Succeeded {
            // A Succeeded job serves a result computed from its private input. If
            // the revocation cleanup redacted that input, a source of the request
            // was revoked (plan §11.5): the result is not served as live, whatever
            // the operation's own read-side check would make of the redacted
            // records behind it. The job itself is preserved as the record of the
            // action; only this read is refused.
            ensure_private_input_not_redacted_record(ctx, &self.store, &job).await?;
        }
        if job.identity().operation == "evaluation.status"
            && job.identity().state == ManagementJobState::Succeeded
        {
            let request = load_private_request_record(ctx, &self.store, &job)
                .await
                .map_err(InputFault::into_error)?;
            if let ParsedRequest::EvaluationStatus(request) = request {
                let status =
                    IndependentEvaluationControl::status(ctx, &self.store, &request.ticket_id)
                        .await?;
                // Only a formal terminal has report-read metadata to disclose;
                // without one the original public result is served unchanged.
                replace_read_result(
                    &mut job,
                    if status.formal.is_some() {
                        let report_read = crate::streaming_evaluator::formal_report_read_metadata(
                            ctx,
                            &self.store,
                            &status.ticket.id,
                        )
                        .await?;
                        public_evaluation_status_read(&status, report_read)?
                    } else {
                        public_evaluation_status(&status)
                    },
                );
            }
        }
        if job.identity().operation == "replay.run"
            && job.identity().state == ManagementJobState::Succeeded
        {
            if let Some(ManagementResult::ReplayStored {
                report_id,
                pool_digest,
                semantic_reports_digest,
            }) = job.original_result()
            {
                // A completed job is an audit fact. Legacy selection is refused
                // by new consumers, while status still returns this exact result.
                let read = crate::replay::query_replay_report_record(ctx, &self.store, report_id)
                    .await
                    .map_err(crate::replay::replay_read_isolation_for_existing_api)?;
                let binding = if let Some(old) = read.record.report() {
                    (
                        &old.report_id,
                        &old.pool_digest,
                        &old.semantic_reports_digest,
                    )
                } else if let Some(current) = read.record.current_report() {
                    (
                        &current.report_id,
                        &current.pool_digest,
                        &current.semantic_reports_digest,
                    )
                } else {
                    return Err(Error::Invalid("invalid replay audit record".into()));
                };
                if binding != (report_id, pool_digest, semantic_reports_digest) {
                    return Err(Error::Conflict("replay report view digest mismatch".into()));
                }
            } else {
                return Err(Error::Conflict(
                    "replay job succeeded without valid ReplayStored result".into(),
                ));
            }
        }
        if job.identity().operation == "curriculum.step"
            && job.identity().state == ManagementJobState::Succeeded
        {
            if let Some(ManagementResult::ProbeScheduled {
                probe_job_id,
                trigger_digest,
                terminal,
            }) = job.original_result()
            {
                let view =
                    crate::curriculum::verified_probe_job_view(ctx, &self.store, probe_job_id)
                        .await?;
                if &view.id != probe_job_id
                    || &view.trigger_digest != trigger_digest
                    || view.terminal != *terminal
                {
                    return Err(Error::Conflict(
                        "probe job view differs from the stored result".into(),
                    ));
                }
            } else {
                return Err(Error::Conflict(
                    "curriculum job succeeded without valid ProbeScheduled result".into(),
                ));
            }
        }
        if job.identity().operation == "exploration.start"
            && job.identity().state == ManagementJobState::Succeeded
        {
            if let Some(ManagementResult::ExplorationStarted {
                world_id,
                context_signature,
                prefix_digest,
                legal_actions_digest,
                policy_digest,
                caps_digest,
                action,
            }) = job.original_result()
            {
                // What a dispatch never changes is always compared: the world's
                // id, context and the digests of its frozen policy and caps,
                // read through the live source closure (which also re-checks
                // revocation).
                let registration = crate::exploration::verified_world_registration_view(
                    ctx,
                    &self.store,
                    world_id,
                )
                .await?;
                // The result is the first decision, a pure function over the
                // empty prefix of the world as registered (plan §7.1). While no
                // dispatch has started that is the stored world's own. Once the
                // stored world moved on (nodes, spent budget, waits), the live
                // decision is another decision, so the first one is recomputed
                // from the world this job requested, which its private input
                // still holds.
                let requested = if registration.started {
                    let request = load_private_request_record(ctx, &self.store, &job)
                        .await
                        .map_err(InputFault::into_error)?;
                    let ParsedRequest::ExplorationStart(request) = request else {
                        return Err(Error::Conflict(
                            "exploration job input is not an exploration.start request".into(),
                        ));
                    };
                    if &request.world.id != world_id {
                        return Err(Error::Conflict(
                            "exploration job input names a different world".into(),
                        ));
                    }
                    Some(request)
                } else {
                    None
                };
                let first = match &requested {
                    Some(request) => crate::exploration::first_decision(&request.world)?,
                    None => crate::exploration::first_decision(&registration.registered_world)?,
                };
                if &registration.registered_world.id != world_id
                    || &registration.context_signature != context_signature
                    || &registration.policy_digest != policy_digest
                    || &registration.caps_digest != caps_digest
                    || &first.world_id != world_id
                    || &first.prefix_digest != prefix_digest
                    || &first.legal_actions_digest != legal_actions_digest
                    || &first.policy_digest != policy_digest
                    || &first.caps_digest != caps_digest
                    || fingerprint(&first.action)? != fingerprint(action)?
                {
                    return Err(Error::Conflict(
                        "exploration decision view differs from the stored result".into(),
                    ));
                }
                // A started world's counters, nodes and waits have moved, but
                // what it was registered as cannot have: re-check it against the
                // requested world the way registration compares it (the
                // fingerprint of its immutable facts, and the budget it was
                // registered with, rebuilt from its dispatch facts).
                if let Some(request) = &requested {
                    crate::exploration::ensure_world_registered_as(
                        ctx,
                        &self.store,
                        &request.world,
                    )
                    .await?;
                }
            } else {
                return Err(Error::Conflict(
                    "exploration job succeeded without valid ExplorationStarted result".into(),
                ));
            }
        }
        if job.identity().operation == "trace.export"
            && job.identity().state == ManagementJobState::Succeeded
        {
            // A completed trace export job serves references only: the record
            // it names is re-read live with its source closure, and a delivered
            // result must still name the exact published tree. Private
            // material, tokens and paths never enter this view.
            let Some(ManagementResultV2::Trace(result)) = job.result() else {
                return Err(Error::Conflict(
                    "trace export job succeeded without valid trace result".into(),
                ));
            };
            crate::trace_export::verify_result_view(ctx, &self.store, &result).await?;
        }
        Ok(job)
    }

    pub async fn cancel(&self, ctx: &Context, job_id: &str) -> Result<VersionedManagementJob> {
        self.require_trusted(ctx)?;
        let record = cancel_job_atomic(ctx, &self.store, job_id).await?;
        if record.identity().operation == "optimization.step"
            && record.identity().state == ManagementJobState::Draining
        {
            self.optimization_budget_actions(ctx, job_id).await?;
        }
        Ok(record)
    }

    fn spawn_job(&self, job_id: String) -> Result<()> {
        if !self
            .active_workers
            .lock()
            .map_err(|_| Error::Internal)?
            .insert(job_id.clone())
        {
            return Ok(());
        }
        let dispatcher = self.clone();
        tokio::spawn(async move {
            loop {
                match dispatcher.run_job(&job_id).await {
                    Ok(()) => break,
                    Err(error @ (Error::ClockRegressed | Error::Internal)) => {
                        // A failed pre-claim storage/clock read has not proved
                        // a corrupt private input or a stopped worker. Keep the
                        // same supervisor alive and retry without a fake finish.
                        tracing::error!(job_id = %job_id, error = %error, "management supervisor awaits storage or clock recovery");
                        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                    }
                    Err(Error::Capacity(_)) => {
                        dispatcher.retry_claim_later(job_id.clone());
                        break;
                    }
                    Err(error) => {
                        tracing::error!(job_id = %job_id, error = %error, "management background supervisor failed");
                        if !matches!(&error, Error::Conflict(reason) if reason == "management job lease fence changed")
                            && let Err(record_error) =
                                dispatcher.record_background_error(&job_id, &error).await
                        {
                            tracing::error!(job_id = %job_id, error = %record_error, "management supervisor diagnostic unavailable");
                        }
                        break;
                    }
                }
            }
            if let Ok(mut active) = dispatcher.active_workers.lock() {
                active.remove(&job_id);
            }
        });
        Ok(())
    }

    fn spawn_reconciliation(
        &self,
        ctx: Context,
        job: ManagementJobV2,
        observed: ObservedWorkerExit,
    ) -> Result<()> {
        if !self
            .active_workers
            .lock()
            .map_err(|_| Error::Internal)?
            .insert(job.id.clone())
        {
            return Ok(());
        }
        let dispatcher = self.clone();
        tokio::spawn(async move {
            loop {
                match dispatcher
                    .finish_observed_worker(&ctx, &job, &observed)
                    .await
                {
                    Ok(state) if is_terminal(state) => break,
                    Ok(_) => {}
                    Err(Error::ClockRegressed | Error::Internal) => {}
                    Err(error) => {
                        tracing::error!(job_id = %job.id, error = %error, "management reconciliation refused");
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            }
            if let Ok(mut active) = dispatcher.active_workers.lock() {
                active.remove(&job.id);
            }
        });
        Ok(())
    }

    async fn run_job(&self, job_id: &str) -> Result<()> {
        let (ctx, claimed) = self.claim(job_id).await?;
        let Some(job) = claimed else { return Ok(()) };
        let store = self.store.clone();
        let worker_ctx = ctx.clone();
        let worker_job = job.clone();
        let control = WorkerControl::new();
        let worker_control = control.clone();
        let optimization_runtime = self.optimization_runtime.clone();
        let mut handle = tokio::spawn(async move {
            let request = match load_private_request(&worker_ctx, &store, &worker_job).await {
                Ok(request) => request,
                Err(fault) => {
                    let step = match &fault {
                        InputFault::Redacted { .. } => "private_input_redacted",
                        InputFault::Other(_) => "private_input_invalid",
                    };
                    return ObservedWorkerExit::BeforeConsumerFailed(JobOutcome {
                        state: ManagementJobState::Failed,
                        step: step.into(),
                        result: None,
                        error_code: Some(fault.error_code().into()),
                    });
                }
            };
            match execute_request(
                &worker_ctx,
                &store,
                &worker_job,
                request,
                &worker_control,
                optimization_runtime.as_deref(),
            )
            .await
            .or_else(|error| {
                execution_outcome(worker_job, Err(error)).map(ObservedWorkerExit::Completed)
            }) {
                Ok(observed) => observed,
                Err(error) => ObservedWorkerExit::Failed(error_code(&error).into()),
            }
        });
        let mut ticks = tokio::time::interval(std::time::Duration::from_secs(20));
        ticks.tick().await;
        let observed = loop {
            tokio::select! {
                biased;
                result = &mut handle => {
                    break match result {
                        Ok(observed) => observed,
                        Err(_) => ObservedWorkerExit::Failed("management_worker_exited".into()),
                    };
                }
                _ = ticks.tick() => {
                    if let Err(error) = renew_claim(&ctx, &self.store, &job).await {
                        // The same actual handle remains owned and awaited.
                        // Time/storage failure stops future steps; it does not
                        // prove network completion, refund money or abort work.
                        tracing::error!(job_id = %job.id, error = %error, "management lease renewal refused; stopping new steps");
                        control.stop_with_error(error);
                    }
                }
            }
        };
        control.stop();
        loop {
            match self.finish_observed_worker(&ctx, &job, &observed).await {
                Ok(state) if is_terminal(state) => return Ok(()),
                Ok(_) => tokio::time::sleep(std::time::Duration::from_secs(20)).await,
                Err(error @ (Error::ClockRegressed | Error::Internal)) => {
                    tracing::error!(job_id = %job.id, error = %error, "observed management exit awaiting healthy clock/storage");
                    tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Legacy step name retained for reading previously capacity-deferred jobs.
    /// New capacity refusals leave the job, diagnostics and audit unchanged.
    pub const CAPACITY_WAIT_STEP: &'static str = "capacity_wait";
    /// Delay before a capacity-deferred claim is retried.
    pub const CAPACITY_WAIT_RETRY: std::time::Duration = std::time::Duration::from_millis(200);

    async fn claim(&self, job_id: &str) -> Result<(Context, Option<ManagementJobV2>)> {
        identifier(job_id)?;
        for trusted in self.trusted_contexts.iter() {
            let mut session = self.store.session().await?;
            let Some(row) = session.object_row(trusted, "job", job_id).await? else {
                session.commit().await?;
                continue;
            };
            let record = evo_storage::decode_management_job(&row.body)?;
            let identity = record.identity();
            if identity.owner_actor != trusted.actor() || identity.owner_role != trusted.role() {
                session.commit().await?;
                continue;
            }
            if row.owner != identity.owner_actor || identity.id != job_id {
                return Err(Error::Conflict(
                    "management job physical binding differs".into(),
                ));
            }
            if is_terminal(identity.state) {
                session.commit().await?;
                return Ok((trusted.clone(), None));
            }
            if identity.state != ManagementJobState::Queued {
                let draining = identity.state == ManagementJobState::Draining;
                session.commit().await?;
                if draining {
                    consumer_progress_or_unknown(trusted, &self.store, &record).await?;
                }
                // Without an observed exit/startup witness this caller may read
                // provenance, but it cannot assume the old worker has stopped.
                return Ok((trusted.clone(), None));
            }
            // Full stable physical/private identity is checked before any
            // capacity-wait, v1 continuation or fresh claim write.
            let record = load_job_record_in_session(trusted, &mut session, job_id).await?;
            let identity = record.identity();
            // Both cancellation and a new consumer need this same untouched
            // origin, before claim overwrites the original phase and fence.
            let never_started = record.generation() == 0
                && identity.lease_token.is_none()
                && identity.lease_until == 0
                && record.result().is_none()
                && record.pending_terminal_state().is_none()
                && matches!(record.step(), "accepted" | Self::CAPACITY_WAIT_STEP)
                && !matches!(&record, VersionedManagementJob::V2(job) if job.optimization.is_some());
            if record.cancel_requested() && never_started {
                // Proven never-started cancellation is closure, not new use of
                // private content; revoked input cannot prevent this close.
                let job = record.into_unfinished_v2()?;
                session.commit().await?;
                self.finish_observed_worker(trusted, &job, &ObservedWorkerExit::NeverStarted)
                    .await?;
                return Ok((trusted.clone(), None));
            }
            // The physical read's Internal/Clock error propagates untouched.
            // Only an actually read missing/redacted/invalid private body is
            // the original private-input fault, not an inferred storage fault.
            let private = match private_input_body_in_session(
                trusted,
                &mut session,
                identity.private_input_ref,
            )
            .await
            {
                Err(error @ (Error::ClockRegressed | Error::Internal)) => return Err(error),
                Err(error) => Err(InputFault::Other(error)),
                Ok(None) => Err(InputFault::Other(Error::NotFound)),
                Ok(Some(body)) if is_redacted(&body) => Err(InputFault::Redacted {
                    id: identity.private_input_ref.into(),
                }),
                Ok(Some(body)) => decode_private_request_fields(
                    body,
                    identity.private_input_ref,
                    identity.operation,
                    identity.payload_digest,
                )
                .map_err(InputFault::Other),
            };
            if !never_started {
                // Only an untouched accepted/capacity-wait origin may start a
                // new consumer. Historical phases, leases and saved work must
                // retain the existing exit/startup reconciliation path.
                let mut job = record.into_unfinished_v2()?;
                now()?;
                job.state = ManagementJobState::Draining;
                session
                    .put(trusted, "job", &job.id, trusted.actor(), &job)
                    .await?;
                session
                    .audit(trusted, "management.job.unknown_continuation", &job.id)
                    .await?;
                session.commit().await?;
                return Ok((trusted.clone(), None));
            }
            let request = match private {
                Ok(request) => request,
                Err(fault) => {
                    let code = fault.error_code();
                    let phase = match fault {
                        InputFault::Redacted { .. } => "private_input_redacted",
                        InputFault::Other(_) => "private_input_invalid",
                    };
                    let mut job = record.into_unfinished_v2()?;
                    now()?;
                    job.state = ManagementJobState::Failed;
                    job.step = phase.into();
                    job.error_code = Some(code.into());
                    session
                        .put(trusted, "job", &job.id, trusted.actor(), &job)
                        .await?;
                    session
                        .audit(trusted, "management.job.private_input_failed", &job.id)
                        .await?;
                    session.commit().await?;
                    return Ok((trusted.clone(), None));
                }
            };
            let admission = match validate_request_authority(trusted, &request) {
                Err(error) => Err(error),
                Ok(()) => {
                    ensure_dependencies_live(
                        trusted,
                        &mut session,
                        &private_dependencies(&request)?,
                    )
                    .await
                }
            };
            if let Err(error) = admission {
                if matches!(error, Error::ClockRegressed | Error::Internal) {
                    return Err(error);
                }
                let mut job = record.into_unfinished_v2()?;
                now()?;
                job.state = ManagementJobState::Failed;
                job.step = "failed".into();
                job.error_code = Some(error_code(&error).into());
                session
                    .put(trusted, "job", &job.id, trusted.actor(), &job)
                    .await?;
                session
                    .audit(trusted, "management.job.admission_failed", &job.id)
                    .await?;
                session.commit().await?;
                return Ok((trusted.clone(), None));
            }
            let mut job = record.into_unfinished_v2()?;
            // One trusted clock observation precedes every new timing write.
            let at = now()?;
            let capacity_at = u64::try_from(at).map_err(|_| Error::ClockRegressed)?;
            let usage = session.capacity_usage_v41(capacity_at).await?;
            evo_core::capacity::CapacityItem::ConcurrentManagementDispatch
                .admit(usage.concurrent_dispatches, 1)?;
            evo_core::capacity::CapacityItem::ActiveLease.admit(usage.active_leases, 1)?;
            let claim_at = now()?;
            job.generation = job
                .generation
                .checked_add(1)
                .ok_or_else(|| Error::Conflict("management job generation overflow".into()))?;
            job.lease_token = Some(hash(
                format!("{}\0{}\0{}", job.id, job.generation, claim_at).as_bytes(),
            ));
            job.lease_until = claim_at
                .checked_add(60)
                .ok_or_else(|| Error::Invalid("management lease clock exceeds i64".into()))?;
            job.state = ManagementJobState::Running;
            job.step = "claimed".into();
            session
                .put(trusted, "job", &job.id, trusted.actor(), &job)
                .await?;
            session
                .audit(trusted, "management.job.claim", &job.id)
                .await?;
            session.commit().await?;
            return Ok((trusted.clone(), Some(job)));
        }
        Err(Error::Forbidden)
    }

    /// Re-run a capacity-deferred claim after `CAPACITY_WAIT_RETRY`. The loop
    /// ends when the job is claimed, cancelled, or otherwise terminal; a lost
    /// process simply leaves the job Queued for `recover_pending`.
    fn retry_claim_later(&self, job_id: String) {
        let dispatcher = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Self::CAPACITY_WAIT_RETRY).await;
            if let Err(error) = dispatcher.spawn_job(job_id.clone()) {
                tracing::error!(job_id = %job_id, error = %error, "management retry spawn failed");
            }
        });
    }

    // A diagnostic is not worker-exit evidence. Running jobs are closed only
    // by the owned JoinHandle path above; this preserves the old fault API.
    async fn record_background_error(&self, job_id: &str, error: &Error) -> Result<()> {
        match error {
            Error::ClockRegressed => return Err(Error::ClockRegressed),
            Error::Internal => return Err(Error::Internal),
            _ => {}
        }
        for context in self.trusted_contexts.iter() {
            let mut session = self.store.session().await?;
            let Some(row) = session.object_row(context, "job", job_id).await? else {
                session.commit().await?;
                continue;
            };
            let record = evo_storage::decode_management_job(&row.body)?;
            let identity = record.identity();
            if identity.owner_actor != context.actor() || identity.owner_role != context.role() {
                session.commit().await?;
                continue;
            }
            if is_terminal(identity.state) {
                session.commit().await?;
                return Ok(());
            }
            if row.owner != identity.owner_actor || row.id != identity.id {
                return Err(Error::Conflict(
                    "management job physical binding differs".into(),
                ));
            }
            // Error handling is a writer too: a rejected stable/private ID
            // must not escape through a diagnostic or a fresh Queued failure.
            let record = load_job_record_in_session(context, &mut session, job_id).await?;
            let mut job = record.into_unfinished_v2()?;
            let code = match error {
                Error::Conflict(reason)
                    if reason
                        == &format!(
                            "management private input {} was redacted because its source was revoked",
                            job.private_input_ref
                        ) =>
                {
                    SOURCE_REVOKED_CODE
                }
                _ => error_code(error),
            };
            let at = now()?;
            if job.diagnostics.len() >= 32 {
                job.diagnostics.remove(0);
            }
            job.diagnostics.push(ManagementDiagnostic {
                code: code.into(),
                observed_generation: job.generation,
                recorded_at: at,
            });
            if job.state == ManagementJobState::Queued {
                if job.generation == 0 && job.lease_token.is_none() && job.result.is_none() {
                    job.state = ManagementJobState::Failed;
                    job.step = "background_failed".into();
                    job.error_code = Some(code.into());
                    job.lease_until = 0;
                } else {
                    job.state = ManagementJobState::Draining;
                }
            }
            session
                .put(context, "job", &job.id, context.actor(), &job)
                .await?;
            session
                .audit(context, "management.job.background_failed", &job.id)
                .await?;
            session.commit().await?;
            return Ok(());
        }
        Err(Error::Forbidden)
    }

    fn require_trusted(&self, ctx: &Context) -> Result<()> {
        if self.trusted_contexts.iter().any(|trusted| {
            trusted.namespace() == ctx.namespace()
                && trusted.actor() == ctx.actor()
                && trusted.role() == ctx.role()
        }) {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }
}

impl ParsedRequest {
    fn request_key(&self) -> &str {
        match self {
            Self::ExperimentRegister(request) => &request.request_key,
            Self::ExperimentRegisterCurrent(request) => &request.request_key,
            Self::EvaluationStart(request) => &request.request_key,
            Self::EvaluationStatus(request) => &request.request_key,
            Self::ReplayRun(request) => &request.request_key,
            Self::CurriculumStep(request) => &request.request_key,
            Self::ExplorationStart(request) => &request.request_key,
            Self::Blocked(request) => &request.request_key,
            Self::OptimizationStep(request) => &request.request_key,
            Self::TraceExportPrepare(request) => &request.request_key,
            Self::TraceExportPublish(request) => &request.request_key,
            Self::BackupCreate(request) => &request.request_key,
            Self::BudgetRootAuthorize(request) => &request.request_key,
            Self::BudgetStagePlanBind(request) => &request.request_key,
            Self::BudgetCallsListOpen(request) => &request.request_key,
            Self::BudgetCallReconcile(request) => &request.request_key,
            Self::ImportRegister(request) => &request.request_key,
            Self::ImportExecute(request) => &request.request_key,
            Self::AssetStage(request) => &request.request_key,
            Self::AssetAbort(request) => &request.request_key,
            Self::AssetHandoff(request) => &request.request_key,
            Self::ExportPrepare(request) => &request.request_key,
            Self::ExportComplete(request) => &request.request_key,
            Self::ExportAbort(request) => &request.request_key,
        }
    }
}

async fn persist_management(
    ctx: &Context,
    store: &Store,
    operation: &str,
    payload: Value,
) -> Result<VersionedManagementJob> {
    let entry = evo_core::management_registry::lookup(operation)
        .ok_or_else(|| Error::Invalid("unknown_management_operation".into()))?;
    if entry.plane != evo_core::management_registry::Plane::Dispatch {
        return Err(Error::Invalid("management_operation_plane_mismatch".into()));
    }
    if entry.execution != evo_core::management_registry::ExecutionKind::AsyncJob {
        return Err(Error::Invalid(
            "management operation requires dedicated route".into(),
        ));
    }
    required_management_role(ctx, operation)?;
    let request = parse_request(operation, payload)?;
    validate_request_authority(ctx, &request)?;
    identifier(request.request_key())?;
    let payload_digest = fingerprint(&request)?;
    let job_id = stable_id("management-job", ctx, operation, request.request_key());
    let private_id = stable_id("management-input", ctx, operation, request.request_key());
    let mut session = store.session().await?;
    if let Some(cached) = session
        .cached_management_job(ctx, operation, request.request_key(), &request)
        .await?
    {
        let old = cached.identity();
        if old.id != job_id
            || old.private_input_ref != private_id
            || old.payload_digest != payload_digest
        {
            return Err(Error::Conflict(
                "cached management job differs from its request".into(),
            ));
        }
        let current = load_job_record_in_session(ctx, &mut session, &job_id).await?;
        let actual = current.identity();
        if actual.operation != operation
            || actual.request_key != request.request_key()
            || actual.payload_digest != payload_digest
            || actual.private_input_ref != private_id
        {
            return Err(Error::Conflict(
                "persisted management payload digest changed".into(),
            ));
        }
        // Never rewrite an old terminal, its cached v1 response or its input.
        session.commit().await?;
        return Ok(current);
    }
    let dependencies = private_dependencies(&request)?;
    ensure_dependencies_live(ctx, &mut session, &dependencies).await?;
    session
        .admit_capacity(&[(evo_core::capacity::CapacityItem::ManagementJobRecord, 1)])
        .await?;
    let private = PrivateManagementInput {
        id: private_id.clone(),
        schema_version: INPUT_SCHEMA.into(),
        operation: operation.into(),
        payload_digest: payload_digest.clone(),
        payload: serde_json::to_value(&request).map_err(|_| Error::Internal)?,
    };
    let job = ManagementJobV2 {
        id: job_id.clone(),
        schema_version: JOB_SCHEMA.into(),
        operation: operation.into(),
        request_key: request.request_key().into(),
        payload_digest,
        owner_actor: ctx.actor().into(),
        owner_role: ctx.role(),
        state: ManagementJobState::Queued,
        step: "accepted".into(),
        private_input_ref: private_id.clone(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: Vec::new(),
        created_at: now()?,
        cancel_arrival: None,
        pending_terminal_state: None,
        optimization: None,
        backup: None,
    };
    session
        .put(ctx, "artifact", &private_id, ctx.actor(), &private)
        .await?;
    for (kind, id) in &dependencies {
        session
            .put_edge(ctx, "artifact", &private_id, kind, id)
            .await?;
    }
    session.put(ctx, "job", &job_id, ctx.actor(), &job).await?;
    session
        .put_edge(ctx, "job", &job_id, "artifact", &private_id)
        .await?;
    session
        .cache(
            ctx,
            operation,
            request.request_key(),
            &request,
            &job_id,
            &job,
        )
        .await?;
    session.audit(ctx, "management.accept", &job_id).await?;
    session.commit().await?;
    Ok(VersionedManagementJob::V2(job))
}

/// What the submit check of a new request calls the thing it refuses (the words of
/// its `Conflict`).
const SUBMIT_SUBJECT: &str = "management request";

/// `Conflict` naming the first thing a new request depends on whose source was
/// revoked: the dependencies themselves, then everything above them. The judgement
/// is shared with the other writers of a new dependency (`revocation_gate`); the
/// request answers in its own words. It reads in the session that goes on to write,
/// before anything is written, so a refusal leaves no private input, job, edge,
/// idempotency row or audit record behind.
async fn ensure_dependencies_live(
    ctx: &Context,
    session: &mut Session,
    dependencies: &[(String, String)],
) -> Result<()> {
    revocation_gate::ensure_dependencies_live(ctx, session, dependencies, SUBMIT_SUBJECT).await
}

async fn load_job_record_in_session(
    ctx: &Context,
    session: &mut Session,
    job_id: &str,
) -> Result<VersionedManagementJob> {
    identifier(job_id)?;
    let row = session
        .object_row(ctx, "job", job_id)
        .await?
        .ok_or(Error::NotFound)?;
    let record = evo_storage::decode_management_job(&row.body)?;
    let job = record.identity();
    if row.owner != ctx.actor() {
        return Err(Error::NotFound);
    }
    match &record {
        VersionedManagementJob::V1(original) => ensure_owner(ctx, original)?,
        VersionedManagementJob::V2(current) => {
            if current.owner_actor != ctx.actor() || current.owner_role != ctx.role() {
                return Err(Error::NotFound);
            }
        }
    }
    if job.id != job_id
        || job.id != stable_id("management-job", ctx, job.operation, job.request_key)
        || job.private_input_ref
            != stable_id("management-input", ctx, job.operation, job.request_key)
    {
        return Err(Error::Conflict(
            "management job physical binding differs".into(),
        ));
    }
    Ok(record)
}

async fn load_job_record(
    ctx: &Context,
    store: &Store,
    job_id: &str,
) -> Result<VersionedManagementJob> {
    let mut session = store.session().await?;
    let record = load_job_record_in_session(ctx, &mut session, job_id).await?;
    session.commit().await?;
    Ok(record)
}

fn replace_read_result(record: &mut VersionedManagementJob, result: ManagementResult) {
    match record {
        VersionedManagementJob::V1(job) => job.result = Some(result),
        VersionedManagementJob::V2(job) => job.result = Some(ManagementResultV2::Original(result)),
    }
}

#[derive(Debug, Clone)]
struct JobOutcome {
    state: ManagementJobState,
    step: String,
    result: Option<ManagementResultV2>,
    error_code: Option<String>,
}

impl JobOutcome {
    fn has_committed_side_effect(&self) -> bool {
        if self.step == "experiment_already_registered" {
            return false;
        }
        match self.result.as_ref() {
            Some(ManagementResultV2::Partial(ManagementPartialResultV2::ControlRegistered {
                ..
            })) => true,
            Some(ManagementResultV2::Original(
                ManagementResult::ExperimentRegistered { .. }
                | ManagementResult::EvaluationTicketBlocked { .. }
                | ManagementResult::ReplayStored { .. }
                | ManagementResult::ProbeScheduled { .. }
                | ManagementResult::ExplorationStarted { .. },
            )) => true,
            // This is an audit result; the operation's joined reader proves
            // actual compiled or dispatched effects independently.
            Some(ManagementResultV2::Optimization(_)) => false,
            Some(ManagementResultV2::Trace(_)) => true,
            Some(ManagementResultV2::Backup(_)) => true,
            // A listing writes only its own job/audit row; the other three
            // budget results are real authorization/accounting commits.
            Some(ManagementResultV2::Budget(ManagementBudgetResultV2::OpenCallsListed {
                ..
            })) => false,
            Some(ManagementResultV2::Budget(_)) => true,
            // Import registration commits real selection/source rows; import
            // execution commits the derived result and its edges.
            Some(ManagementResultV2::Import(_)) => true,
            // Staged/aborted/handed-off assets and prepared/completed/aborted
            // local exports all commit real business rows.
            Some(ManagementResultV2::AssetExport(_)) => true,
            Some(ManagementResultV2::Original(
                ManagementResult::EvaluationStatus { .. }
                | ManagementResult::EvaluationStatusReadV2 { .. },
            ))
            | None => false,
        }
    }
}

/// Private carrier minted only after this dispatcher's real successful claim.
/// Its Context comes from authentication, never from the persisted request.
#[derive(Clone)]
pub(crate) struct ManagementOptimizationClaim {
    caller: Context,
    job: ManagementJobV2,
    model_context: evo_core::optimization::ModelRequestContext,
    billing_scope: String,
    root_budget_id: String,
    control: WorkerControl,
}

pub(crate) enum ManagementOptimizationCallSource<'a> {
    Model(&'a crate::model::ReadBoundModelRequest),
    Development(&'a crate::optimization::DevelopmentRunRequest),
}

impl ManagementOptimizationClaim {
    pub(crate) async fn admit_new(&self, session: &mut Session) -> Result<ManagementJobV2> {
        self.caller.require(&[Role::Admin])?;
        let current =
            begin_management_step(&self.caller, session, &self.job, &self.control).await?;
        if current.operation != "optimization.step" {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        }
        let root = session
            .root_budget(&self.caller, &self.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        if root.root_budget_id != self.root_budget_id {
            return Err(Error::Conflict(
                "management_optimization_root_changed".into(),
            ));
        }
        // Original reserve/begin still decides stopped/root monetary refusals.
        // This guard adds job provenance, never a replacement budget decision.
        Ok(current)
    }

    fn check_stage_scope(&self, fact: &crate::optimization::StageFact) -> Result<()> {
        fact.validate()?;
        if fact.namespace != self.caller.namespace()
            || fact.episode_id != self.model_context.episode_id
            || fact.step != self.model_context.step
            || fact.attempt != self.model_context.attempt
        {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn require_stage(
        &self,
        session: &mut Session,
        fact: &crate::optimization::StageFact,
    ) -> Result<()> {
        self.check_stage_scope(fact)?;
        let record = load_job_record_in_session(&self.caller, session, &self.job.id).await?;
        let VersionedManagementJob::V2(job) = record else {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        };
        let fact_digest = optimization_stage_record_digest(fact)?;
        let owned = job.optimization.as_ref().is_some_and(|progress| {
            progress.stages.iter().any(|reference| {
                reference.artifact_id == fact.artifact_id
                    && reference.input_digest == fact.input_digest
                    && reference.generation == self.job.generation
                    && reference.record_digest == fact_digest
            })
        });
        if !owned {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn require_observation_origin(
        &self,
        session: &mut Session,
        fact: &crate::optimization::StageFact,
    ) -> Result<()> {
        self.check_stage_scope(fact)?;
        let record = load_job_record_in_session(&self.caller, session, &self.job.id).await?;
        let VersionedManagementJob::V2(job) = record else {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        };
        let progress = job
            .optimization
            .as_ref()
            .ok_or_else(|| Error::Conflict("management_optimization_origin_unknown".into()))?;
        for reference in &progress.stages {
            if reference.generation != self.job.generation {
                continue;
            }
            let row = session
                .object_row(&self.caller, "artifact", &reference.artifact_id)
                .await?
                .ok_or(Error::NotFound)?;
            if row.owner != progress.journal_actor
                || !optimization_stage_reference_matches(
                    &row.body,
                    reference,
                    self.caller.namespace(),
                    &self.model_context.episode_id,
                )?
            {
                continue;
            }
            let owns = match fact.kind {
                crate::optimization::StageFactKind::DispatchObserved => {
                    reference.kind == "dispatch_prepared"
                        && reference.request_id == fact.request_id
                        && reference.input_digest == fact.input_digest
                }
                crate::optimization::StageFactKind::StepCompleted => {
                    reference.kind == "step_prepared"
                        && reference.request_id == fact.request_id
                        && reference.input_digest == fact.input_digest
                }
                _ => reference.kind == "step_prepared",
            };
            if owns {
                return Ok(());
            }
        }
        Err(Error::Conflict(
            "management_optimization_origin_unknown".into(),
        ))
    }

    async fn initialize_progress(
        &self,
        session: &mut Session,
        job: &mut ManagementJobV2,
        journal_actor: &str,
    ) -> Result<()> {
        let row = session
            .object_row(&self.caller, "artifact", &job.private_input_ref)
            .await?
            .ok_or(Error::NotFound)?;
        if row.owner != self.caller.actor() {
            return Err(Error::NotFound);
        }
        let parsed = decode_private_request_fields(
            evo_storage::lifecycle::parse_strict_json(&row.body)?,
            &job.private_input_ref,
            &job.operation,
            &job.payload_digest,
        )?;
        let ParsedRequest::OptimizationStep(request) = parsed else {
            return Err(Error::Internal);
        };
        let control = crate::development::management_control_in_session(
            session,
            &self.caller,
            &request.development_control_id,
        )
        .await?;
        if control.proposer_actor != journal_actor
            || control.root_budget_id != self.root_budget_id
            || control.billing_scope != self.billing_scope
        {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        }
        let digest = hash(row.body.as_bytes());
        match &job.optimization {
            Some(progress)
                if progress.private_input_digest != digest
                    || progress.journal_actor != journal_actor =>
            {
                Err(Error::Conflict(
                    "management_optimization_origin_unknown".into(),
                ))
            }
            Some(_) => Ok(()),
            None => {
                job.optimization = Some(ManagementOptimizationProgressV2 {
                    private_input_digest: digest,
                    journal_actor: journal_actor.into(),
                    stages: Vec::new(),
                    calls: Vec::new(),
                });
                Ok(())
            }
        }
    }

    pub(crate) async fn record_prepared_stage(
        &self,
        session: &mut Session,
        fact: &crate::optimization::StageFact,
        journal_actor: &str,
    ) -> Result<()> {
        self.check_stage_scope(fact)?;
        let mut job = self.admit_new(session).await?;
        self.initialize_progress(session, &mut job, journal_actor)
            .await?;
        let progress = job.optimization.as_mut().ok_or(Error::Internal)?;
        if let Some(old) = progress
            .stages
            .iter()
            .find(|old| old.artifact_id == fact.artifact_id)
        {
            if old.input_digest != fact.input_digest
                || old.generation != self.job.generation
                || old.record_digest != optimization_stage_record_digest(fact)?
                || old.kind != optimization_stage_kind(fact.kind)?
                || old.request_id != fact.request_id
            {
                return Err(Error::Conflict(
                    "management_optimization_origin_unknown".into(),
                ));
            }
            return Ok(());
        }
        if session
            .object_row(&self.caller, "artifact", &fact.artifact_id)
            .await?
            .is_some()
        {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        }
        progress.stages.push(ManagementOptimizationStageRef {
            artifact_id: fact.artifact_id.clone(),
            kind: optimization_stage_kind(fact.kind)?,
            request_id: fact.request_id.clone(),
            record_digest: optimization_stage_record_digest(fact)?,
            dispatch_not_performed: optimization_stage_not_dispatched(fact),
            input_digest: fact.input_digest.clone(),
            generation: self.job.generation,
        });
        session
            .put(&self.caller, "job", &job.id, self.caller.actor(), &job)
            .await?;
        session
            .put_edge(&self.caller, "job", &job.id, "artifact", &fact.artifact_id)
            .await?;
        session
            .audit(&self.caller, "management.optimization.stage", &job.id)
            .await
    }

    pub(crate) async fn record_observed_stage(
        &self,
        session: &mut Session,
        fact: &crate::optimization::StageFact,
        journal_actor: &str,
    ) -> Result<()> {
        self.require_observation_origin(session, fact).await?;
        let record = load_job_record_in_session(&self.caller, session, &self.job.id).await?;
        if is_terminal(record.identity().state) {
            return Ok(());
        }
        let mut job = record.into_unfinished_v2()?;
        // A late observation is not a new use. It must still belong to the
        // original physical generation and the real journal producer.
        if !same_claim_fields(&job, &self.job) {
            return Err(Error::Conflict("management job lease fence changed".into()));
        }
        let progress = job.optimization.as_mut().ok_or(Error::Internal)?;
        if progress.journal_actor != journal_actor {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        }
        let digest = optimization_stage_record_digest(fact)?;
        if let Some(old) = progress
            .stages
            .iter()
            .find(|old| old.artifact_id == fact.artifact_id)
        {
            if old.record_digest != digest || old.generation != self.job.generation {
                return Err(Error::Conflict(
                    "management_optimization_origin_unknown".into(),
                ));
            }
        } else {
            progress.stages.push(ManagementOptimizationStageRef {
                artifact_id: fact.artifact_id.clone(),
                kind: optimization_stage_kind(fact.kind)?,
                request_id: fact.request_id.clone(),
                record_digest: digest,
                dispatch_not_performed: optimization_stage_not_dispatched(fact),
                input_digest: fact.input_digest.clone(),
                generation: self.job.generation,
            });
        }
        if fact.kind == crate::optimization::StageFactKind::StepCompleted {
            let has_compiled = progress
                .stages
                .iter()
                .any(|reference| reference.kind == "edit_compiled");
            let outcome = optimization_terminal_outcome_with_compiled(fact, has_compiled)?;
            if !committed_result_is_preserved(job.result.as_ref(), outcome.result.as_ref())? {
                return Err(Error::Conflict(
                    "management_optimization_committed_result_changed".into(),
                ));
            }
            job.pending_terminal_state = pending_terminal(outcome.state);
            job.state = ManagementJobState::Draining;
            job.step = outcome.step;
            job.result = outcome.result;
            job.error_code = outcome.error_code;
        }
        session
            .put(&self.caller, "job", &job.id, self.caller.actor(), &job)
            .await?;
        session
            .put_edge(&self.caller, "job", &job.id, "artifact", &fact.artifact_id)
            .await?;
        session
            .audit(&self.caller, "management.optimization.observed", &job.id)
            .await
    }

    async fn prepared_stage(
        &self,
        session: &mut Session,
        job: &ManagementJobV2,
        request_id: &str,
        input_digest: &str,
    ) -> Result<String> {
        let progress = job
            .optimization
            .as_ref()
            .ok_or_else(|| Error::Conflict("management_optimization_origin_unknown".into()))?;
        for reference in &progress.stages {
            if reference.generation != self.job.generation || reference.input_digest != input_digest
            {
                continue;
            }
            let fact: crate::optimization::StageFact = session
                .need(&self.caller, "artifact", &reference.artifact_id)
                .await?;
            self.check_stage_scope(&fact)?;
            if fact.artifact_id == reference.artifact_id
                && fact.input_digest == reference.input_digest
                && fact.request_id == request_id
                && fact.kind == crate::optimization::StageFactKind::DispatchPrepared
            {
                return Ok(fact.artifact_id);
            }
        }
        Err(Error::Conflict(
            "management_optimization_origin_unknown".into(),
        ))
    }

    pub(crate) async fn compiled_stages(
        &self,
        session: &mut Session,
    ) -> Result<Vec<crate::optimization::StageFact>> {
        let record = load_job_record_in_session(&self.caller, session, &self.job.id).await?;
        let VersionedManagementJob::V2(job) = record else {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        };
        let progress = job
            .optimization
            .as_ref()
            .ok_or_else(|| Error::Conflict("management_optimization_origin_unknown".into()))?;
        let mut compiled = Vec::new();
        for reference in &progress.stages {
            if reference.generation != self.job.generation {
                continue;
            }
            let fact: crate::optimization::StageFact = session
                .need(&self.caller, "artifact", &reference.artifact_id)
                .await?;
            self.check_stage_scope(&fact)?;
            if fact.artifact_id != reference.artifact_id
                || fact.input_digest != reference.input_digest
            {
                return Err(Error::Conflict(
                    "management_optimization_origin_unknown".into(),
                ));
            }
            if fact.kind == crate::optimization::StageFactKind::EditCompiled {
                compiled.push(fact);
            }
        }
        Ok(compiled)
    }

    pub(crate) async fn step_prepared(
        &self,
        session: &mut Session,
    ) -> Result<crate::optimization::StageFact> {
        let record = load_job_record_in_session(&self.caller, session, &self.job.id).await?;
        let VersionedManagementJob::V2(job) = record else {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        };
        let progress = job
            .optimization
            .as_ref()
            .ok_or_else(|| Error::Conflict("management_optimization_origin_unknown".into()))?;
        for reference in &progress.stages {
            if reference.generation != self.job.generation {
                continue;
            }
            let fact: crate::optimization::StageFact = session
                .need(&self.caller, "artifact", &reference.artifact_id)
                .await?;
            self.check_stage_scope(&fact)?;
            if fact.artifact_id == reference.artifact_id
                && fact.input_digest == reference.input_digest
                && fact.kind == crate::optimization::StageFactKind::StepPrepared
                && fact.request_id == self.model_context.request_id
            {
                return Ok(fact);
            }
        }
        Err(Error::Conflict(
            "management_optimization_origin_unknown".into(),
        ))
    }

    pub(crate) async fn require_call(
        &self,
        session: &mut Session,
        call: &evo_storage::budget::BudgetCallRecord,
        artifact: &evo_storage::budget::BudgetArtifact,
    ) -> Result<()> {
        let record = load_job_record_in_session(&self.caller, session, &self.job.id).await?;
        let VersionedManagementJob::V2(job) = record else {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        };
        let reference = job
            .optimization
            .as_ref()
            .and_then(|progress| {
                progress.calls.iter().find(|reference| {
                    reference.billing_scope == call.billing_scope
                        && reference.call_id == call.call_id
                })
            })
            .ok_or_else(|| Error::Conflict("management_optimization_origin_unknown".into()))?;
        if call.namespace != self.caller.namespace()
            || reference.generation != self.job.generation
            || reference.root_budget_id != self.root_budget_id
            || reference.billing_scope != self.billing_scope
            || reference.actual_input_digest != call.actual_input_digest
            || reference.request_schema != artifact.schema_version
            || reference.request_digest != artifact.digest
            || hash(artifact.body.as_bytes()) != artifact.digest
        {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        }
        let fact: crate::optimization::StageFact = session
            .need(&self.caller, "artifact", &reference.prepared_stage_id)
            .await?;
        self.require_stage(session, &fact).await?;
        Ok(())
    }

    pub(crate) async fn record_call(
        &self,
        session: &mut Session,
        producer_actor: &str,
        source: ManagementOptimizationCallSource<'_>,
        call: &evo_storage::budget::BudgetCallRecord,
        root: &evo_storage::budget::RootBudgetRecord,
        artifact: &evo_storage::budget::BudgetArtifact,
    ) -> Result<()> {
        let (producer, prepared_request_id, prepared_input_digest) = match source {
            ManagementOptimizationCallSource::Model(bound) => {
                if call.call_id != bound.request.request_id
                    || call.actual_input_digest != bound.request.cache_key_digest
                {
                    return Err(Error::Conflict(
                        "management_optimization_origin_unknown".into(),
                    ));
                }
                (
                    ManagementOptimizationCallProducer::Model,
                    bound.request.request_id.as_str(),
                    fingerprint(bound)?,
                )
            }
            ManagementOptimizationCallSource::Development(request) => (
                ManagementOptimizationCallProducer::Development,
                request.request_id.as_str(),
                fingerprint(request)?,
            ),
        };
        let mut job = self.admit_new(session).await?;
        if call.namespace != self.caller.namespace()
            || call.billing_scope != self.billing_scope
            || root.root_budget_id != self.root_budget_id
            || root.billing_scope != self.billing_scope
            || call.dispatch_group_id != self.model_context.episode_id
            || hash(artifact.body.as_bytes()) != artifact.digest
            || call.request_artifact.as_ref() != Some(artifact)
        {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        }
        let prepared_stage_id = self
            .prepared_stage(session, &job, prepared_request_id, &prepared_input_digest)
            .await?;
        if job.optimization.as_ref().is_some_and(|progress| {
            progress
                .calls
                .iter()
                .any(|old| old.billing_scope == call.billing_scope && old.call_id == call.call_id)
        }) {
            self.require_call(session, call, artifact).await?;
            let old = job
                .optimization
                .as_mut()
                .and_then(|progress| {
                    progress.calls.iter_mut().find(|old| {
                        old.billing_scope == call.billing_scope && old.call_id == call.call_id
                    })
                })
                .ok_or(Error::Internal)?;
            if old.lease_token != call.lease_token || old.lease_epoch != call.lease_epoch {
                old.lease_token = call.lease_token.clone();
                old.lease_epoch = call.lease_epoch;
                session
                    .put(&self.caller, "job", &job.id, self.caller.actor(), &job)
                    .await?;
                session
                    .audit(
                        &self.caller,
                        "management.optimization.call_refence",
                        &job.id,
                    )
                    .await?;
            }
            return Ok(());
        }
        let budget_ref_id = optimization_budget_ref_id(
            self.caller.namespace(),
            &call.billing_scope,
            &call.call_id,
        )?;
        let budget_ref = session
            .object_row(&self.caller, "artifact", &budget_ref_id)
            .await?
            .ok_or(Error::NotFound)?;
        if budget_ref.owner != producer_actor {
            return Err(Error::Conflict(
                "management_optimization_origin_unknown".into(),
            ));
        }
        validate_optimization_budget_ref(
            &budget_ref.body,
            &budget_ref_id,
            self.caller.namespace(),
            &call.billing_scope,
            &call.call_id,
        )?;
        let reference = ManagementOptimizationCallRef {
            producer_actor: producer_actor.into(),
            budget_ref_digest: hash(budget_ref.body.as_bytes()),
            producer,
            prepared_stage_id,
            generation: self.job.generation,
            billing_scope: call.billing_scope.clone(),
            root_budget_id: root.root_budget_id.clone(),
            call_id: call.call_id.clone(),
            actual_input_digest: call.actual_input_digest.clone(),
            request_schema: artifact.schema_version.clone(),
            request_digest: artifact.digest.clone(),
            lease_token: call.lease_token.clone(),
            lease_epoch: call.lease_epoch,
        };
        job.optimization
            .as_mut()
            .ok_or(Error::Internal)?
            .calls
            .push(reference);
        session
            .put(&self.caller, "job", &job.id, self.caller.actor(), &job)
            .await?;
        session
            .audit(&self.caller, "management.optimization.call", &job.id)
            .await
    }
}

#[derive(Clone)]
struct WorkerControl {
    stop_new_steps: Arc<AtomicBool>,
    stop_error: Arc<Mutex<Option<Error>>>,
    #[cfg(test)]
    backup_pause: Arc<Mutex<Option<backup::TestPause>>>,
}

impl WorkerControl {
    fn new() -> Self {
        Self {
            stop_new_steps: Arc::new(AtomicBool::new(false)),
            stop_error: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            backup_pause: Arc::new(Mutex::new(None)),
        }
    }
    fn stop(&self) {
        self.stop_new_steps.store(true, Ordering::Release);
    }
    fn stop_with_error(&self, error: Error) {
        if let Ok(mut reason) = self.stop_error.lock()
            && reason.is_none()
        {
            *reason = Some(error);
        }
        self.stop();
    }
    fn admit_step(&self) -> Result<()> {
        if self.stop_new_steps.load(Ordering::Acquire) {
            if let Some(error) = self.stop_error.lock().map_err(|_| Error::Internal)?.take() {
                return Err(error);
            }
            return Err(Error::Cancelled);
        }
        Ok(())
    }
}

// This evidence is private to an actually awaited handle or this startup's
// post-lock recovery entry. It cannot be supplied by request JSON or caller bool.
#[derive(Debug, Clone)]
enum ObservedWorkerExit {
    Completed(JobOutcome),
    BeforeConsumerFailed(JobOutcome),
    // An actual read-only refusal or business refusal from an atomic
    // non-dispatching writer, never an operation-name or empty-call guess.
    BeforeCommitFailed(JobOutcome),
    NeverStarted,
    Failed(String),
    Restarted,
}

async fn execute_request(
    ctx: &Context,
    store: &Store,
    claimed: &ManagementJobV2,
    request: ParsedRequest,
    control: &WorkerControl,
    optimization_runtime: Option<&crate::optimization::ManagementOptimizationRuntime>,
) -> Result<ObservedWorkerExit> {
    let mut job = claimed.clone();
    let result = match request {
        ParsedRequest::ExperimentRegister(request) => {
            return run_experiment_register(ctx, store, job, *request, control).await;
        }
        ParsedRequest::ExperimentRegisterCurrent(request) => {
            run_experiment_register_current(ctx, store, &mut job, *request, control).await
        }
        ParsedRequest::EvaluationStart(request) => {
            return run_evaluation_start(ctx, store, job, request, control).await;
        }
        ParsedRequest::EvaluationStatus(request) => {
            return run_evaluation_status(ctx, store, job, request, control).await;
        }
        ParsedRequest::ReplayRun(request) => {
            run_replay(ctx, store, &mut job, *request, control).await
        }
        ParsedRequest::CurriculumStep(request) => {
            run_curriculum_step(ctx, store, &mut job, request, control).await
        }
        ParsedRequest::ExplorationStart(request) => {
            return run_exploration_start(ctx, store, job, *request, control).await;
        }
        ParsedRequest::OptimizationStep(request) => {
            run_optimization_management(
                ctx,
                store,
                &mut job,
                *request,
                control,
                optimization_runtime,
            )
            .await
        }
        ParsedRequest::TraceExportPrepare(request) => {
            return run_trace_export_prepare(ctx, store, job, *request, control).await;
        }
        ParsedRequest::TraceExportPublish(request) => {
            return run_trace_export_publish(ctx, store, job, *request, control).await;
        }
        ParsedRequest::BackupCreate(request) => {
            return backup::execute(ctx, store, claimed, request, control).await;
        }
        ParsedRequest::BudgetRootAuthorize(request) => {
            return run_budget_root_authorize(ctx, store, claimed.clone(), *request, control).await;
        }
        ParsedRequest::BudgetStagePlanBind(request) => {
            return run_budget_stage_plan_bind(ctx, store, claimed.clone(), *request, control)
                .await;
        }
        ParsedRequest::BudgetCallsListOpen(request) => {
            return run_budget_calls_list_open(ctx, store, claimed.clone(), *request, control)
                .await;
        }
        ParsedRequest::BudgetCallReconcile(request) => {
            return run_budget_call_reconcile(ctx, store, claimed.clone(), *request, control).await;
        }
        ParsedRequest::ImportRegister(request) => {
            return run_import_register(ctx, store, claimed.clone(), *request, control).await;
        }
        ParsedRequest::ImportExecute(request) => {
            return run_import_execute(ctx, store, claimed.clone(), *request, control).await;
        }
        ParsedRequest::AssetStage(request) => {
            return run_asset_stage(ctx, store, claimed.clone(), *request, control).await;
        }
        ParsedRequest::AssetAbort(request) => {
            return run_asset_abort(ctx, store, claimed.clone(), *request, control).await;
        }
        ParsedRequest::AssetHandoff(request) => {
            return run_asset_handoff(ctx, store, claimed.clone(), *request, control).await;
        }
        ParsedRequest::ExportPrepare(request) => {
            return run_export_prepare(ctx, store, claimed.clone(), *request, control).await;
        }
        ParsedRequest::ExportComplete(request) => {
            return run_export_complete(ctx, store, claimed.clone(), *request, control).await;
        }
        ParsedRequest::ExportAbort(request) => {
            return run_export_abort(ctx, store, claimed.clone(), *request, control).await;
        }
        ParsedRequest::Blocked(_) => {
            control.admit_step()?;
            job.state = ManagementJobState::Blocked;
            job.step = "blocked_feature".into();
            job.error_code = Some(format!("{}_consumer_unavailable", job.operation));
            Ok(())
        }
    };
    execution_outcome(job, result).map(ObservedWorkerExit::Completed)
}

async fn run_optimization_management(
    ctx: &Context,
    store: &Store,
    job: &mut ManagementJobV2,
    request: OptimizationStepManagementRequest,
    control: &WorkerControl,
    runtime: Option<&crate::optimization::ManagementOptimizationRuntime>,
) -> Result<()> {
    checkpoint_claim(ctx, store, job, "before_optimization_resolve", control).await?;
    let Some(runtime) = runtime else {
        job.state = ManagementJobState::Blocked;
        job.step = "optimization_backend_unconfigured".into();
        job.error_code = Some("optimization_backend_unconfigured".into());
        return Ok(());
    };
    let (inputs, budget) = runtime
        .resolve_inputs(ctx, store, &job.id, &request)
        .await?;
    let claim = Arc::new(ManagementOptimizationClaim {
        caller: ctx.clone(),
        job: job.clone(),
        model_context: inputs.model_context().clone(),
        billing_scope: budget.billing_scope,
        root_budget_id: budget.root_budget_id,
        control: control.clone(),
    });
    let (completed, compiled) = runtime.run_claimed(store, claim, &inputs).await?;
    let outcome = optimization_terminal_outcome(&completed, &compiled)?;
    job.state = outcome.state;
    job.step = outcome.step;
    job.result = outcome.result;
    job.error_code = outcome.error_code;

    Ok(())
}

fn execution_outcome(mut job: ManagementJobV2, result: Result<()>) -> Result<JobOutcome> {
    let result = match result {
        Err(error @ Error::ClockRegressed) => return Err(error),
        result => result,
    };
    if let Err(error) = result {
        job.state = ManagementJobState::Failed;
        job.step = "failed".into();
        job.error_code = Some(error_code(&error).into());
    }
    Ok(JobOutcome {
        state: job.state,
        step: job.step,
        result: job.result,
        error_code: job.error_code,
    })
}

async fn run_experiment_register(
    ctx: &Context,
    store: &Store,
    mut job: ManagementJobV2,
    request: ExperimentRegisterRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    if request.control.evaluator_actor != ctx.actor()
        || request.holdout.registration_id != request.control.id
    {
        return Err(Error::Forbidden);
    }
    let expected_control = request.control.digest()?;
    let expected_holdout = request.holdout.digest(&request.control)?;
    let mut progress = IndependentEvaluationControl::registration_progress(
        ctx,
        store,
        &request.control.id,
        &request.holdout.id,
    )
    .await?;
    if progress
        .control_digest
        .as_deref()
        .is_some_and(|digest| digest != expected_control)
        || progress
            .holdout_digest
            .as_deref()
            .is_some_and(|digest| digest != expected_holdout)
    {
        return Err(Error::Conflict(
            "registered experiment content differs".into(),
        ));
    }
    if progress.control_digest.is_some() {
        // This owned worker passed claim's never-started origin gate. The
        // matching C was only read, before this consumer's first writer.
        return execution_outcome(
            job,
            Err(Error::Conflict(
                "legacy registered control is not Current input".into(),
            )),
        )
        .map(ObservedWorkerExit::BeforeCommitFailed);
    }
    if progress.control_digest.is_none() {
        IndependentEvaluationControl::register_control(ctx, store, request.control.clone()).await?;
        job.step = "control_registered".into();
        checkpoint_claim(ctx, store, &job, "control_registered", control).await?;
        progress = IndependentEvaluationControl::registration_progress(
            ctx,
            store,
            &request.control.id,
            &request.holdout.id,
        )
        .await?;
    }
    if progress.holdout_digest.is_none() {
        checkpoint_claim(ctx, store, &job, "before_holdout_registration", control).await?;
        IndependentEvaluationControl::register_holdout(ctx, store, request.holdout.clone()).await?;
    }
    let final_progress = IndependentEvaluationControl::registration_progress(
        ctx,
        store,
        &request.control.id,
        &request.holdout.id,
    )
    .await?;
    if final_progress.control_digest.as_deref() != Some(expected_control.as_str())
        || final_progress.holdout_digest.as_deref() != Some(expected_holdout.as_str())
    {
        return Err(Error::Conflict(
            "experiment registration did not converge".into(),
        ));
    }
    job.state = ManagementJobState::Succeeded;
    job.step = "experiment_registered".into();
    job.result = Some(ManagementResultV2::Original(
        ManagementResult::ExperimentRegistered {
            control_id: request.control.id,
            control_digest: expected_control,
            holdout_id: request.holdout.id,
            holdout_digest: expected_holdout,
        },
    ));
    execution_outcome(job, Ok(())).map(ObservedWorkerExit::Completed)
}

async fn run_experiment_register_current(
    ctx: &Context,
    store: &Store,
    job: &mut ManagementJobV2,
    request: ExperimentRegisterRequestV2,
    control: &WorkerControl,
) -> Result<()> {
    if request.control.evaluator_actor != ctx.actor()
        || request.holdout.registration_id != request.control.id
    {
        return Err(Error::Forbidden);
    }
    let expected_control = request.control.digest()?;
    let expected_holdout = request.holdout.digest(&request.control)?;
    let mut session = store.session().await?;
    let mut current = begin_management_step(ctx, &mut session, job, control).await?;
    let progress = IndependentEvaluationControl::registration_progress_in_session(
        ctx,
        &mut session,
        &request.control.id,
        &request.holdout.id,
    )
    .await?;
    if progress
        .control_digest
        .as_deref()
        .is_some_and(|digest| digest != expected_control)
        || progress
            .holdout_digest
            .as_deref()
            .is_some_and(|digest| digest != expected_holdout)
    {
        return Err(Error::Conflict(
            "registered experiment content differs".into(),
        ));
    }
    if progress.control_digest.is_none() {
        return Err(Error::Conflict(
            "current control must be registered before candidate binding".into(),
        ));
    }
    let already_registered = progress.holdout_digest.is_some();
    if !already_registered {
        let digest = IndependentEvaluationControl::register_current_holdout_in_session(
            ctx,
            &mut session,
            request.holdout.clone(),
        )
        .await?;
        if digest != expected_holdout {
            return Err(Error::Conflict(
                "experiment registration did not converge".into(),
            ));
        }
        // A cancellation written by the transaction's actual producer hook
        // must retain the H and its complete same-job result.
        current = reload_same_claim_after_write(ctx, &mut session, job).await?;
    }
    current.step = if already_registered {
        "experiment_already_registered"
    } else {
        "current_holdout_registered"
    }
    .into();
    current.result = Some(ManagementResultV2::Original(
        ManagementResult::ExperimentRegistered {
            control_id: request.control.id.clone(),
            control_digest: expected_control.clone(),
            holdout_id: request.holdout.id.clone(),
            holdout_digest: expected_holdout.clone(),
        },
    ));
    if current.cancel_requested && !already_registered {
        current.cancel_arrival = Some(ManagementCancelArrival::AfterSideEffect);
    }
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.job.checkpoint", &current.id)
        .await?;
    session.commit().await?;
    *job = current;
    job.state = ManagementJobState::Succeeded;
    Ok(())
}

/// Preserve the existing wire's real physical-ID lookup. Only a logical ID
/// that has no physical row enters the owner's canonical identity reader.
async fn evaluation_start_existing_status(
    ctx: &Context,
    store: &Store,
    request: &EvaluationStartRequest,
    stored_ticket_id: Option<&str>,
) -> Result<Option<StreamingEvaluationStatus>> {
    ctx.require(&[Role::Evaluator])?;
    let mut session = store.session().await?;
    let selected = if let Some(id) = stored_ticket_id {
        if session
            .object_row(
                ctx,
                "artifact",
                &e05_storage_id("evaluation_ticket_v2", id)?,
            )
            .await?
            .is_none()
        {
            return Err(Error::NotFound);
        }
        id.to_owned()
    } else if session
        .object_row(
            ctx,
            "artifact",
            &e05_storage_id("evaluation_ticket_v2", &request.ticket.ticket_id)?,
        )
        .await?
        .is_some()
    {
        request.ticket.ticket_id.clone()
    } else {
        let id = IndependentEvaluationControl::management_expected_ticket_id_in_session(
            ctx,
            &mut session,
            &request.ticket.engine_request(),
        )
        .await?;
        if session
            .object_row(
                ctx,
                "artifact",
                &e05_storage_id("evaluation_ticket_v2", &id)?,
            )
            .await?
            .is_none()
        {
            session.commit().await?;
            return Ok(None);
        }
        id
    };
    session.commit().await?;
    let existing = IndependentEvaluationControl::status(ctx, store, &selected).await?;
    if existing.ticket.id != selected
        || existing.ticket.registration_id != request.ticket.registration_id
        || existing.ticket.holdout_id != request.ticket.holdout_id
        || existing.ticket.issued_at_unix_seconds != request.ticket.issued_at_unix_seconds
        || existing.ticket.evaluator_actor != ctx.actor()
        || (selected != request.ticket.ticket_id
            && !existing
                .ticket
                .source_plan_binding
                .as_ref()
                .is_some_and(|binding| binding.logical_ticket_id == request.ticket.ticket_id))
    {
        return Err(Error::Conflict(
            "existing ticket differs from request".into(),
        ));
    }
    Ok(Some(existing))
}

async fn run_evaluation_start(
    ctx: &Context,
    store: &Store,
    mut job: ManagementJobV2,
    request: EvaluationStartRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    checkpoint_claim(ctx, store, &job, "before_ticket_issue", control).await?;
    let issued = match IndependentEvaluationControl::issue_ticket(
        ctx,
        store,
        request.ticket.engine_request(),
    )
    .await
    {
        Err(Error::Conflict(_)) => evaluation_start_existing_status(ctx, store, &request, None)
            .await
            .and_then(|existing| existing.map(|status| status.ticket).ok_or(Error::NotFound)),
        result => result,
    };
    let ticket = match issued {
        Ok(ticket) => ticket,
        Err(error @ (Error::ClockRegressed | Error::Internal)) => return Err(error),
        Err(error) => {
            // issue_ticket commits its business writes only in its final
            // transaction; this business refusal rolled them back. The
            // existing-ticket fallback above only reads, never dispatches.
            return execution_outcome(job, Err(error)).map(ObservedWorkerExit::BeforeCommitFailed);
        }
    };
    job.state = ManagementJobState::Blocked;
    job.step = "ticket_issued_execution_blocked".into();
    job.error_code = Some("execution_provider_unconfigured".into());
    job.result = Some(ManagementResultV2::Original(
        ManagementResult::EvaluationTicketBlocked {
            ticket_id: ticket.id,
        },
    ));
    execution_outcome(job, Ok(())).map(ObservedWorkerExit::Completed)
}

async fn run_evaluation_status(
    ctx: &Context,
    store: &Store,
    mut job: ManagementJobV2,
    request: EvaluationStatusRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    checkpoint_claim(ctx, store, &job, "before_status_read", control).await?;
    let status = match IndependentEvaluationControl::status(ctx, store, &request.ticket_id).await {
        Ok(status) => status,
        Err(error @ (Error::ClockRegressed | Error::Internal)) => return Err(error),
        Err(error) => {
            // The executed strict reader has no business writer or outbound
            // work and cannot own the historical ticket's provider calls.
            return execution_outcome(job, Err(error)).map(ObservedWorkerExit::BeforeCommitFailed);
        }
    };
    job.state = ManagementJobState::Succeeded;
    job.step = "status_read".into();
    job.result = Some(ManagementResultV2::Original(public_evaluation_status(
        &status,
    )));
    execution_outcome(job, Ok(())).map(ObservedWorkerExit::Completed)
}

fn public_evaluation_status(status: &StreamingEvaluationStatus) -> ManagementResult {
    let lifecycle = serde_json::to_value(status.ticket.lifecycle)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    ManagementResult::EvaluationStatus {
        ticket_id: status.ticket.id.clone(),
        lifecycle,
        planned_targets: status.ticket.targets.len(),
        has_formal_terminal: status.formal.is_some(),
    }
}

/// `trace.export` prepare, one Session and one transaction from the live fence
/// to the record write and the terminal job state: the claim and cancel fence,
/// the authenticated source loader and the storage put's own quota and
/// classification gates all commit together, and a refusal (including a
/// cancellation observed at the fence) rolls all of it back — no record, no
/// download surface, zero export side effect.
async fn run_trace_export_prepare(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: TraceExportPrepareRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    let mut session = store.session().await?;
    let finished =
        match trace_export_prepare_in_session(ctx, &mut session, &claimed, &request, control).await
        {
            Ok(finished) => finished,
            Err(error @ (Error::ClockRegressed | Error::Internal)) => return Err(error),
            Err(error) => {
                return execution_outcome(claimed, Err(error))
                    .map(ObservedWorkerExit::BeforeCommitFailed);
            }
        };
    session.commit().await?;
    Ok(ObservedWorkerExit::Completed(JobOutcome {
        state: ManagementJobState::Succeeded,
        step: finished.step,
        result: finished.result,
        error_code: None,
    }))
}

async fn trace_export_prepare_in_session(
    ctx: &Context,
    session: &mut Session,
    claimed: &ManagementJobV2,
    request: &TraceExportPrepareRequest,
    control: &WorkerControl,
) -> Result<ManagementJobV2> {
    let mut current = begin_management_step(ctx, session, claimed, control).await?;
    let started = now()?;
    let members = crate::trace_export::native_members(ctx, session, &request.source_ids).await?;
    let record = crate::trace_export::write_prepared_record(
        ctx,
        session,
        &request.export_id,
        &current.id,
        &members,
        started,
    )
    .await?;
    current.state = ManagementJobState::Succeeded;
    current.step = "trace_export_prepared".into();
    current.lease_token = None;
    current.lease_until = 0;
    current.result = Some(ManagementResultV2::Trace(
        ManagementTraceExportResultV2::TraceExportPrepared {
            export_id: record.export_id,
            record_id: record.id,
            candidate_digest: record.candidate_digest,
        },
    ));
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.job.finish", &current.id)
        .await?;
    Ok(current)
}

/// `trace.export` publish in three steps. TX1, on the job's claim fence, is the
/// publish decision: the frozen digest, the record's one publish job and the
/// frozen members re-derived once through the authenticated loader (source
/// closure, license, recipient scope), moving the record to `reviewed`. The
/// candidate is then staged privately and renamed into its formal directory
/// under the storage publication fence, which re-checks the reviewed record
/// and its source closure — not the namespace watermark — with the database
/// write lock held across the rename. TX2 commits the delivery fact and the
/// terminal job state together on the same claim. A refusal before the rename
/// commits `aborted`; a failure after it is returned as an error, so recovery
/// completes `delivered` from the exact published tree instead of claiming a
/// zero-effect failure.
async fn run_trace_export_publish(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: TraceExportPublishRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    let started = now()?;
    let mut session = store.session().await?;
    let (record, files) = match trace_export_publish_decision(
        ctx,
        &mut session,
        &claimed,
        &request,
        control,
        started,
    )
    .await
    {
        Ok(decided) => decided,
        Err(error @ (Error::ClockRegressed | Error::Internal)) => return Err(error),
        Err(error) => {
            return execution_outcome(claimed, Err(error))
                .map(ObservedWorkerExit::BeforeCommitFailed);
        }
    };
    session.commit().await?;
    let published = match store.stage_trace_export(ctx, &record.id, &files).await {
        Ok(staged) => store.publish_staged_trace_export(ctx, staged).await,
        Err(error) => Err(error),
    };
    let receipt = match published {
        Ok(receipt) => receipt,
        Err(error @ (Error::ClockRegressed | Error::Internal)) => return Err(error),
        Err(error) => {
            return commit_trace_export_abort(ctx, store, claimed, &record, started, error).await;
        }
    };
    let mut session = store.session().await?;
    let mut current = reload_same_claim_after_write(ctx, &mut session, &claimed).await?;
    let delivered = crate::trace_export::finish_publication(
        ctx,
        &mut session,
        &record,
        Some(&receipt),
        started,
    )
    .await?;
    current.state = ManagementJobState::Succeeded;
    current.step = "trace_export_delivered".into();
    current.lease_token = None;
    current.lease_until = 0;
    if current.cancel_requested {
        // The bytes already left through the fenced publication; the
        // completion and the cancel arrival are both recorded honestly.
        current.cancel_arrival = Some(ManagementCancelArrival::AfterSideEffect);
    }
    current.result = Some(ManagementResultV2::Trace(
        ManagementTraceExportResultV2::TraceExportDelivered {
            export_id: delivered.export_id,
            record_id: delivered.id,
            candidate_digest: delivered.candidate_digest,
            receipt_digest: receipt.completion_receipt_digest,
        },
    ));
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.job.finish", &current.id)
        .await?;
    session.commit().await?;
    Ok(ObservedWorkerExit::Completed(JobOutcome {
        state: current.state,
        step: current.step,
        result: current.result,
        error_code: None,
    }))
}

/// TX1 of the publish job: the claim fence, the visible `publishing` step and
/// the reviewed decision with the re-derived frozen bytes. No filesystem
/// effect happens here.
async fn trace_export_publish_decision(
    ctx: &Context,
    session: &mut Session,
    claimed: &ManagementJobV2,
    request: &TraceExportPublishRequest,
    control: &WorkerControl,
    started: i64,
) -> Result<(
    crate::trace_export::TraceExportRecord,
    Vec<evo_storage::LocalExportFile>,
)> {
    let mut current = begin_management_step(ctx, session, claimed, control).await?;
    let decided = crate::trace_export::review_frozen_candidate(
        ctx,
        session,
        &request.export_id,
        &current.id,
        &request.candidate_digest,
        started,
    )
    .await?;
    current.step = "trace_export_publishing".into();
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.job.checkpoint", &current.id)
        .await?;
    Ok(decided)
}

/// A publication refused or failed before its formal rename: on the same
/// claim, the record's `aborted` phase and the job's terminal failure commit
/// together. The private staging was already released by its guard and
/// nothing was delivered. When the claim moved on or the record can no longer
/// be finished (a source revocation already redacted it), only the refusal is
/// reported.
async fn commit_trace_export_abort(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    record: &crate::trace_export::TraceExportRecord,
    started: i64,
    error: Error,
) -> Result<ObservedWorkerExit> {
    let outcome = execution_outcome(claimed.clone(), Err(error))?;
    let mut session = store.session().await?;
    match trace_export_abort_in_session(ctx, &mut session, &claimed, record, started, &outcome)
        .await
    {
        Ok(aborted) => {
            session.commit().await?;
            Ok(ObservedWorkerExit::BeforeCommitFailed(aborted))
        }
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Err(_) => Ok(ObservedWorkerExit::BeforeCommitFailed(outcome)),
    }
}

async fn trace_export_abort_in_session(
    ctx: &Context,
    session: &mut Session,
    claimed: &ManagementJobV2,
    record: &crate::trace_export::TraceExportRecord,
    started: i64,
    outcome: &JobOutcome,
) -> Result<JobOutcome> {
    let mut current = reload_same_claim_after_write(ctx, session, claimed).await?;
    crate::trace_export::finish_publication(ctx, session, record, None, started).await?;
    current.state = ManagementJobState::Failed;
    current.step = "trace_export_aborted".into();
    current.error_code = outcome.error_code.clone();
    current.lease_token = None;
    current.lease_until = 0;
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.job.finish", &current.id)
        .await?;
    Ok(JobOutcome {
        state: current.state,
        step: current.step,
        result: None,
        error_code: current.error_code,
    })
}

/// Four budget management operations: each handler owns one Session from the
/// live claim fence through the real storage write, the terminal job state and
/// the audit row, committed together exactly once. A business refusal rolls all
/// of it back; only the already-terminal job reconciliation records the
/// refusal.
async fn run_budget_root_authorize(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: BudgetRootAuthorizeRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    let mut session = store.session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        // The persisted authorization time is the service's trusted clock,
        // sampled before the first business write; the request never carries it.
        let created_at = now()?;
        let authorization = evo_storage::budget::RootBudgetAuthorization {
            root_budget_id: request.root_budget_id,
            billing_scope: request.billing_scope,
            allowed_namespaces: request.allowed_namespaces,
            currency: request.currency,
            pricing_version: request.pricing_version,
            payment_subject: request.payment_subject,
            authorization_receipt_digest: request.authorization_receipt_digest,
            per_call_cap_micros: request.per_call_cap_micros,
            total_limit_micros: request.total_limit_micros,
            created_at,
        };
        let root = session.authorize_root_budget(ctx, &authorization).await?;
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "root_authorized".into(),
            result: Some(ManagementResultV2::Budget(
                ManagementBudgetResultV2::RootAuthorized {
                    root: Box::new(root),
                },
            )),
            error_code: None,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    match finished {
        Ok(outcome) => {
            session.commit().await?;
            Ok(ObservedWorkerExit::Completed(outcome))
        }
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Err(error) => {
            execution_outcome(claimed, Err(error)).map(ObservedWorkerExit::BeforeCommitFailed)
        }
    }
}

async fn run_budget_stage_plan_bind(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: BudgetStagePlanBindManagementRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    let mut session = store.session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        let binding = session
            .bind_root_stage_plan(
                ctx,
                &evo_storage::budget::RootStagePlanBindRequest {
                    billing_scope: request.billing_scope,
                    expected_seq: request.expected_seq,
                    plan: request.plan,
                },
            )
            .await?;
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "stage_plan_bound".into(),
            result: Some(ManagementResultV2::Budget(
                ManagementBudgetResultV2::StagePlanBound {
                    binding: Box::new(binding),
                },
            )),
            error_code: None,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    match finished {
        Ok(outcome) => {
            session.commit().await?;
            Ok(ObservedWorkerExit::Completed(outcome))
        }
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Err(error) => {
            execution_outcome(claimed, Err(error)).map(ObservedWorkerExit::BeforeCommitFailed)
        }
    }
}

async fn run_budget_calls_list_open(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: BudgetCallsListOpenRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    let mut session = store.session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        // Read-only over budget/call/root/execution facts; the only writes are
        // this management job and its audit row.
        let page = session
            .list_open_budget_calls(
                ctx,
                &request.billing_scope,
                request.page_size,
                request.cursor.as_deref(),
            )
            .await?;
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "open_calls_listed".into(),
            result: Some(ManagementResultV2::Budget(
                ManagementBudgetResultV2::OpenCallsListed {
                    page: Box::new(page),
                },
            )),
            error_code: None,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    match finished {
        Ok(outcome) => {
            session.commit().await?;
            Ok(ObservedWorkerExit::Completed(outcome))
        }
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Err(error) => {
            execution_outcome(claimed, Err(error)).map(ObservedWorkerExit::BeforeCommitFailed)
        }
    }
}

/// A persisted charge identity may only be reconciled with the same actual
/// facts. The finalized case keeps the kernel's own six-field conflict; the
/// dispatched/uncertain case refuses to overwrite a preserved overflow charge.
fn ensure_budget_charge_matches_persisted(
    call: &evo_storage::budget::BudgetCallRecord,
    charge: &evo_storage::budget::UsageCharge,
) -> Result<()> {
    if call.state == evo_storage::budget::BudgetCallState::Finalized {
        return Ok(());
    }
    let same = |stored: Option<&str>, incoming: &str| stored.is_none_or(|value| value == incoming);
    if !same(
        call.provider_request_id.as_deref(),
        &charge.provider_request_id,
    ) || !same(call.usage_record_id.as_deref(), &charge.usage_record_id)
        || !same(call.output_digest.as_deref(), &charge.output_digest)
        || !same(call.actual_currency.as_deref(), &charge.currency)
        || !same(
            call.actual_pricing_version.as_deref(),
            &charge.pricing_version,
        )
        || call
            .actual_cost_micros
            .is_some_and(|amount| amount != charge.amount_micros)
    {
        return Err(Error::Conflict(
            "reconcile_charge_differs_from_persisted_call".into(),
        ));
    }
    Ok(())
}

async fn run_budget_call_reconcile(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: BudgetCallReconcileRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    let mut session = store.session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        let charge = evo_storage::budget::UsageCharge {
            amount_micros: request.amount_micros,
            currency: request.currency,
            pricing_version: request.pricing_version,
            provider_request_id: request.provider_request_id,
            usage_record_id: request.usage_record_id,
            output_digest: request.output_digest,
        };
        let call = session
            .budget_call(ctx, &request.billing_scope, &request.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        ensure_budget_charge_matches_persisted(&call, &charge)?;
        let reconciled_at = now()?;
        let outcome = session
            .reconcile_budget_call_cost(
                ctx,
                &request.billing_scope,
                &request.call_id,
                &charge,
                reconciled_at,
            )
            .await?;
        let root = session
            .root_budget(ctx, &request.billing_scope)
            .await?
            .ok_or(Error::NotFound)?;
        let (state, step, error_code, accounting_overflow, call) = match outcome {
            evo_storage::budget::BudgetReconcileOutcome::Applied(call) => (
                ManagementJobState::Succeeded,
                "call_reconciled",
                None,
                false,
                call,
            ),
            evo_storage::budget::BudgetReconcileOutcome::AlreadyFinalized(call) => (
                ManagementJobState::Succeeded,
                "call_already_reconciled",
                None,
                false,
                call,
            ),
            // The overflow preservation writes are already in this transaction:
            // the terminal state reports the block while the result keeps the
            // real uncertain call, amounts and stopped root as committed facts.
            evo_storage::budget::BudgetReconcileOutcome::PreservedAccountingOverflow(call) => (
                ManagementJobState::Blocked,
                "cost_accounting_overflow",
                Some("cost_accounting_overflow".to_owned()),
                true,
                call,
            ),
        };
        let outcome = JobOutcome {
            state,
            step: step.into(),
            result: Some(ManagementResultV2::Budget(
                ManagementBudgetResultV2::CallReconciled {
                    call: Box::new(evo_storage::budget::BudgetCallSettlementView::new(
                        &call, &root,
                    )),
                    accounting_overflow,
                    provider_source_verified: false,
                },
            )),
            error_code,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    match finished {
        Ok(outcome) => {
            session.commit().await?;
            Ok(ObservedWorkerExit::Completed(outcome))
        }
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Err(error) => {
            execution_outcome(claimed, Err(error)).map(ObservedWorkerExit::BeforeCommitFailed)
        }
    }
}

async fn run_import_register(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: ImportRegisterManagementRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    ctx.require(&[Role::Admin])?;
    let service = crate::import::PersistentImportService::new(store.clone());
    let sources: Vec<(String, String, crate::import::SourceFormat)> = request
        .sources
        .iter()
        .map(|source| {
            (
                source.source_id.clone(),
                source.blob_digest.clone(),
                source.reader,
            )
        })
        .collect();
    // Blob reads and format checks happen outside the job transaction.
    let prepared = service
        .prepare_management_registration(ctx, &request.request_key, &sources)
        .await?;
    let mut session = store.registered_blob_write_session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        // The same transaction re-verifies every pinned blob under the held
        // publication lock and writes the selection, its sources, edges,
        // idempotency row and audit record.
        let (selection, statuses) = service
            .register_in_session(ctx, &mut session, &prepared)
            .await?;
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "import_registered".into(),
            result: Some(ManagementResultV2::Import(
                ManagementImportResultV2::ImportRegistered {
                    selection_id: selection.id.clone(),
                    selection_digest: fingerprint(&selection)?,
                    source_count: selection.payload.import_source_ids.len() as u64,
                    statuses,
                },
            )),
            error_code: None,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    match finished {
        Ok(outcome) => {
            session.commit().await?;
            Ok(ObservedWorkerExit::Completed(outcome))
        }
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Err(error) => {
            execution_outcome(claimed, Err(error)).map(ObservedWorkerExit::BeforeCommitFailed)
        }
    }
}

async fn run_import_execute(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: ImportExecuteManagementRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    ctx.require(&[Role::Admin])?;
    let service = crate::import::PersistentImportService::new(store.clone());
    // The live selection and pinned blobs are read and derived outside the
    // job transaction.
    let prepared = service
        .prepare_management_execution(ctx, &request.selection_id)
        .await?;
    let mut session = store.registered_blob_write_session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        // The same transaction rechecks the live closure and watermark,
        // re-verifies every ready blob and writes the result below the
        // instance event capacity with the terminal job state.
        let result = service
            .execute_in_session(ctx, &mut session, &prepared)
            .await?;
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "import_completed".into(),
            result: Some(ManagementResultV2::Import(
                ManagementImportResultV2::ImportCompleted {
                    selection_id: result.payload.selection_id.clone(),
                    result_id: result.id.clone(),
                    result_digest: fingerprint(&result)?,
                    state: import_management_result_state(result.payload.state),
                    total_events: result.payload.aggregate_summary.total_events as u64,
                    total_sources: result.payload.aggregate_summary.total_sources as u64,
                    locator_count: result.payload.locators.len() as u64,
                    event_record_count: result.payload.event_records.len() as u64,
                    native_trace_count: result.payload.native_traces.len() as u64,
                    coverage: Box::new(result.payload.import_coverage.clone()),
                    reader_version: result.payload.reader_version.clone(),
                    normalization_version: result.payload.normalization_version.clone(),
                    generation_status: result.payload.generation_status.clone(),
                },
            )),
            error_code: None,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    match finished {
        Ok(outcome) => {
            session.commit().await?;
            Ok(ObservedWorkerExit::Completed(outcome))
        }
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Err(error) => {
            execution_outcome(claimed, Err(error)).map(ObservedWorkerExit::BeforeCommitFailed)
        }
    }
}

fn asset_export_result_state(
    state: crate::packages::StagedAssetState,
) -> ManagementAssetExportState {
    match state {
        crate::packages::StagedAssetState::Staged => ManagementAssetExportState::Staged,
        crate::packages::StagedAssetState::Quarantined => ManagementAssetExportState::Quarantined,
        crate::packages::StagedAssetState::Aborted
        | crate::packages::StagedAssetState::Prepared
        | crate::packages::StagedAssetState::Failed => ManagementAssetExportState::Aborted,
    }
}

fn staged_result(
    staged: &crate::packages::E16Envelope<crate::packages::StagedAssetPayload>,
) -> ManagementAssetExportResultV2 {
    ManagementAssetExportResultV2::AssetStaged {
        staged_asset_id: staged.id.clone(),
        manifest_digest: staged.payload.manifest_digest.clone(),
        content_blob_digest: staged.payload.content_blob_digest.clone(),
        content_bytes: staged.payload.content_bytes,
        state: asset_export_result_state(staged.payload.state),
    }
}

fn export_completed_outcome(completion: &crate::packages::ExportCompletion) -> Result<JobOutcome> {
    let delivery_audit_id = completion.audit.id.clone();
    Ok(JobOutcome {
        state: ManagementJobState::Succeeded,
        step: "export_completed".into(),
        result: Some(ManagementResultV2::AssetExport(
            ManagementAssetExportResultV2::ExportCompleted {
                export_id: completion.attempt.id.clone(),
                staged_asset_id: completion.attempt.payload.staged_asset_id.clone(),
                package_tree_digest: completion.receipt.package_tree_digest.clone(),
                delivered_bytes: completion.receipt.delivered_bytes,
                receipt_digest: completion.receipt.completion_receipt_digest.clone(),
                delivery_audit_id,
            },
        )),
        error_code: None,
    })
}

async fn commit_management_outcome(
    session: Session,
    claimed: ManagementJobV2,
    finished: Result<JobOutcome>,
) -> Result<ObservedWorkerExit> {
    match finished {
        Ok(outcome) => {
            session.commit().await?;
            Ok(ObservedWorkerExit::Completed(outcome))
        }
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Err(error) => {
            execution_outcome(claimed, Err(error)).map(ObservedWorkerExit::BeforeCommitFailed)
        }
    }
}

async fn run_asset_stage(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: AssetStageManagementRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    ctx.require(&[Role::Admin])?;
    // The controlled input blob is read and the whole projection is decoded,
    // validated and hashed outside the job transaction.
    let input = store
        .verified_blob_input(
            ctx,
            &request.input_blob_digest,
            crate::packages::MAX_PERSISTENT_PROJECTION,
        )
        .await?;
    let (stage_request, projection) =
        crate::packages::decode_asset_stage_input(&input.bytes, &request.request_key)?;
    let mut session = store.registered_blob_write_session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        // The same transaction re-checks watermark, sources, dependencies,
        // capacity and the existing key image, then publishes the projection
        // blob and writes the finalized staged asset.
        let staged = crate::packages::PersistentPackageStore::stage_package_in_session(
            ctx,
            &mut session,
            &stage_request,
            &projection,
        )
        .await?;
        if !matches!(
            staged.payload.state,
            crate::packages::StagedAssetState::Staged
                | crate::packages::StagedAssetState::Quarantined
        ) {
            return Err(Error::Conflict("staged asset is terminal".into()));
        }
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "asset_staged".into(),
            result: Some(ManagementResultV2::AssetExport(staged_result(&staged))),
            error_code: None,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    commit_management_outcome(session, claimed, finished).await
}

async fn run_asset_abort(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: AssetAbortManagementRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    ctx.require(&[Role::Admin])?;
    let mut session = store.session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        let staged = crate::packages::PersistentPackageStore::abort_staged_in_session(
            ctx,
            &mut session,
            &request.staged_asset_id,
        )
        .await?;
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "asset_aborted".into(),
            result: Some(ManagementResultV2::AssetExport(
                ManagementAssetExportResultV2::AssetAborted {
                    staged_asset_id: staged.id.clone(),
                    state: ManagementAssetExportState::Aborted,
                },
            )),
            error_code: None,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    commit_management_outcome(session, claimed, finished).await
}

async fn run_asset_handoff(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: AssetHandoffManagementRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    ctx.require(&[Role::Admin])?;
    let staged =
        crate::packages::PersistentPackageStore::read_staged(ctx, store, &request.staged_asset_id)
            .await?;
    if staged.owner_actor != ctx.actor()
        || staged.payload.state != crate::packages::StagedAssetState::Staged
    {
        return Err(Error::Forbidden);
    }
    if staged.payload.package_schema_version != crate::packages::PACKAGE_SCHEMA_V1 {
        return Err(Error::Invalid(
            "candidate handoff requires a validated package projection".into(),
        ));
    }
    if staged.payload.candidate_material.is_none() {
        return Err(Error::Invalid(
            "staged asset has no candidate material binding".into(),
        ));
    }
    if staged.payload.package_kind != "skill" {
        return Err(Error::Invalid(
            "candidate handoff only supports skill packages".into(),
        ));
    }
    let projection = crate::packages::load_staged_projection(ctx, store, &staged).await?;
    let mut session = store.session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        // The same transaction re-reads the staged asset, the Current
        // candidate and the whole material/source closure, then writes only
        // the candidate ref, edge and audit record.
        let handed = crate::packages::PersistentPackageStore::handoff_in_session(
            ctx,
            &mut session,
            &staged,
            &request.candidate_id,
            &projection,
        )
        .await?;
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "asset_handed_off".into(),
            result: Some(ManagementResultV2::AssetExport(
                ManagementAssetExportResultV2::AssetHandedOff {
                    staged_asset_id: handed.id.clone(),
                    candidate_id: request.candidate_id.clone(),
                },
            )),
            error_code: None,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    commit_management_outcome(session, claimed, finished).await
}

async fn run_export_prepare(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: ExportPrepareManagementRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    ctx.require(&[Role::Admin])?;
    let staged =
        crate::packages::PersistentPackageStore::read_staged(ctx, store, &request.staged_asset_id)
            .await?;
    if staged.owner_actor != ctx.actor()
        || staged.payload.state != crate::packages::StagedAssetState::Staged
    {
        return Err(Error::Forbidden);
    }
    if staged.payload.package_schema_version != crate::packages::PACKAGE_SCHEMA_V1 {
        return Err(Error::Invalid(
            "only validated package projections can be exported".into(),
        ));
    }
    let projection = crate::packages::load_staged_projection(ctx, store, &staged).await?;
    let files = crate::packages::local_export_files(&projection)?;
    let (package_tree_digest, delivered_bytes) = evo_storage::local_export_tree_digest(&files)?;
    let mut session = store.session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        // The fixed local destination, the staged watermark/source closure
        // and one replayable intent per request key commit with the job.
        let attempt = crate::packages::PersistentPackageStore::prepare_export_in_session(
            ctx,
            &mut session,
            &request.request_key,
            &staged,
            &files,
            &package_tree_digest,
            delivered_bytes,
            None,
        )
        .await?;
        if attempt.payload.state != crate::packages::ExportAttemptState::Prepared {
            return Err(Error::Conflict("export attempt is not prepared".into()));
        }
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "export_prepared".into(),
            result: Some(ManagementResultV2::AssetExport(
                ManagementAssetExportResultV2::ExportPrepared {
                    export_id: attempt.id.clone(),
                    staged_asset_id: attempt.payload.staged_asset_id.clone(),
                    package_tree_digest: attempt.payload.package_tree_digest.clone(),
                    delivered_bytes: attempt.payload.delivered_bytes,
                },
            )),
            error_code: None,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    commit_management_outcome(session, claimed, finished).await
}

async fn run_export_complete(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: ExportAttemptManagementRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    ctx.require(&[Role::Admin])?;
    // All blob and directory reads, the tree derivation and the dependency
    // preflight happen before the job transaction.
    match crate::packages::PersistentPackageStore::prepare_export_completion(
        ctx,
        store,
        &request.export_id,
    )
    .await?
    {
        crate::packages::ExportCompletionPreparation::Completed(_, completion) => {
            let mut session = store.session().await?;
            let finished = async {
                let mut current =
                    begin_management_step(ctx, &mut session, &claimed, control).await?;
                let outcome = export_completed_outcome(&completion)?;
                apply_completed_outcome(&mut current, &outcome)?;
                session
                    .put(ctx, "job", &current.id, ctx.actor(), &current)
                    .await?;
                session
                    .audit(ctx, "management.job.finish", &current.id)
                    .await?;
                Ok::<JobOutcome, Error>(outcome)
            }
            .await;
            commit_management_outcome(session, claimed, finished).await
        }
        crate::packages::ExportCompletionPreparation::Ready(prepared) => {
            let mut session = store.registered_blob_write_session().await?;
            let finished = async {
                let mut current =
                    begin_management_step(ctx, &mut session, &claimed, control).await?;
                let completion = {
                    let lease = current.lease_token.clone().ok_or(Error::Internal)?;
                    let fence = evo_storage::ManagementJobFence {
                        job_id: &current.id,
                        operation: "export.complete",
                        generation: current.generation,
                        lease_token: &lease,
                    };
                    crate::packages::PersistentPackageStore::complete_export_in_session(
                        ctx,
                        store,
                        &mut session,
                        &request.export_id,
                        &prepared,
                        Some(&fence),
                    )
                    .await?
                };
                let outcome = export_completed_outcome(&completion)?;
                apply_completed_outcome(&mut current, &outcome)?;
                session
                    .put(ctx, "job", &current.id, ctx.actor(), &current)
                    .await?;
                session
                    .audit(ctx, "management.job.finish", &current.id)
                    .await?;
                Ok::<JobOutcome, Error>(outcome)
            }
            .await;
            commit_management_outcome(session, claimed, finished).await
        }
    }
}

async fn run_export_abort(
    ctx: &Context,
    store: &Store,
    claimed: ManagementJobV2,
    request: ExportAttemptManagementRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    ctx.require(&[Role::Admin])?;
    let mut session = store.registered_blob_write_session().await?;
    let finished = async {
        let mut current = begin_management_step(ctx, &mut session, &claimed, control).await?;
        {
            let lease = current.lease_token.clone().ok_or(Error::Internal)?;
            let fence = evo_storage::ManagementJobFence {
                job_id: &current.id,
                operation: "export.abort",
                generation: current.generation,
                lease_token: &lease,
            };
            crate::packages::PersistentPackageStore::abort_export_in_session(
                ctx,
                store,
                &mut session,
                &request.export_id,
                Some(&fence),
            )
            .await?;
        }
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "export_aborted".into(),
            result: Some(ManagementResultV2::AssetExport(
                ManagementAssetExportResultV2::ExportAborted {
                    export_id: request.export_id.clone(),
                    state: ManagementAssetExportState::Aborted,
                },
            )),
            error_code: None,
        };
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        Ok::<JobOutcome, Error>(outcome)
    }
    .await;
    commit_management_outcome(session, claimed, finished).await
}

fn import_management_result_state(
    state: crate::import::ImportResultState,
) -> ManagementImportResultState {
    match state {
        crate::import::ImportResultState::Complete => ManagementImportResultState::Complete,
        crate::import::ImportResultState::Partial => ManagementImportResultState::Partial,
        crate::import::ImportResultState::Failed => ManagementImportResultState::Failed,
    }
}

async fn run_replay(
    ctx: &Context,
    store: &Store,
    job: &mut ManagementJobV2,
    request: ReplayRunRequest,
    control: &WorkerControl,
) -> Result<()> {
    checkpoint_claim(ctx, store, job, "before_replay_run", control).await?;
    let stored = crate::replay::run_and_persist_pool_replay(
        ctx,
        store,
        &request.pool_digest,
        request.partition,
        &request.policy,
        &request.profile,
        &request.caps,
    )
    .await?;
    job.state = ManagementJobState::Succeeded;
    job.step = "replay_stored".into();
    job.result = Some(ManagementResultV2::Original(
        ManagementResult::ReplayStored {
            report_id: stored.report_id,
            pool_digest: stored.pool_digest,
            semantic_reports_digest: stored.semantic_reports_digest,
        },
    ));
    Ok(())
}

async fn run_curriculum_step(
    ctx: &Context,
    store: &Store,
    job: &mut ManagementJobV2,
    request: CurriculumStepRequest,
    control: &WorkerControl,
) -> Result<()> {
    checkpoint_claim(ctx, store, job, "before_curriculum_step", control).await?;
    let coordinator = crate::curriculum::PersistentCurriculumCoordinator::new(
        store.clone(),
        ctx.clone(),
        ctx.actor(),
    )?;
    // The management job id is the idempotency key: a crash re-run of the
    // same job reloads the recorded probe job instead of re-triggering.
    let probe = coordinator
        .schedule_probe_idempotent(
            &job.id,
            &request.profile_id,
            &request.state_id,
            request.root_budget_limit_micros,
        )
        .await?;
    job.state = ManagementJobState::Succeeded;
    job.step = "probe_scheduled".into();
    job.result = Some(ManagementResultV2::Original(
        ManagementResult::ProbeScheduled {
            probe_job_id: probe.id,
            trigger_digest: probe.trigger_digest,
            terminal: probe.terminal,
        },
    ));
    Ok(())
}

async fn run_exploration_start(
    ctx: &Context,
    store: &Store,
    mut job: ManagementJobV2,
    request: ExplorationStartRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    checkpoint_claim(ctx, store, &job, "before_exploration_start", control).await?;
    let coordinator =
        crate::exploration::PersistentCoordinator::new(store.clone(), ctx.clone(), ctx.actor())?;
    let world_id = request.world.id.clone();
    let context_signature = request.world.context_signature.clone();
    // Registration is idempotent on the world's immutable registration facts and
    // on the budget it was registered with: a crash re-run of this job, or the
    // same world under another request key, converges on the world it already
    // persisted (plan §6.7.4) instead of failing on its own earlier commit, also
    // after the world dispatched.
    match coordinator
        .register_world_idempotent(request.world.clone())
        .await
    {
        Ok(_) => {}
        // Storage/clock errors do not establish the transaction's outcome.
        Err(error @ (Error::ClockRegressed | Error::Internal)) => return Err(error),
        Err(error) => {
            // Every business write of this non-dispatching producer is in this
            // transaction. A business refusal rolled it back, even if another
            // job's stored world cannot pass a later observation read.
            return execution_outcome(job, Err(error)).map(ObservedWorkerExit::BeforeCommitFailed);
        }
    }
    // The first decision is a pure function over the empty prefix of the world as
    // requested (plan §7.1), not the live decision: once a dispatch moved the
    // stored world on, the live decision is another one. No node is dispatched
    // by the management adapter.
    let decision = crate::exploration::first_decision(&request.world)?;
    if decision.world_id != world_id {
        return Err(Error::Conflict(
            "exploration decision belongs to a different world".into(),
        ));
    }
    job.state = ManagementJobState::Succeeded;
    job.step = "exploration_started".into();
    job.result = Some(ManagementResultV2::Original(
        ManagementResult::ExplorationStarted {
            world_id,
            context_signature,
            prefix_digest: decision.prefix_digest,
            legal_actions_digest: decision.legal_actions_digest,
            policy_digest: decision.policy_digest,
            caps_digest: decision.caps_digest,
            action: decision.action,
        },
    ));
    execution_outcome(job, Ok(())).map(ObservedWorkerExit::Completed)
}

fn validate_request_authority(ctx: &Context, request: &ParsedRequest) -> Result<()> {
    match request {
        ParsedRequest::ExperimentRegister(request) => {
            if request.control.evaluator_actor != ctx.actor()
                || request.holdout.registration_id != request.control.id
            {
                return Err(Error::Forbidden);
            }
        }
        ParsedRequest::ExperimentRegisterCurrent(request) => {
            if request.control.evaluator_actor != ctx.actor()
                || request.holdout.registration_id != request.control.id
            {
                return Err(Error::Forbidden);
            }
        }
        ParsedRequest::OptimizationStep(_)
        | ParsedRequest::BackupCreate(_)
        | ParsedRequest::ReplayRun(_)
        | ParsedRequest::CurriculumStep(_)
        | ParsedRequest::ExplorationStart(_)
        | ParsedRequest::BudgetRootAuthorize(_)
        | ParsedRequest::BudgetStagePlanBind(_)
        | ParsedRequest::BudgetCallsListOpen(_)
        | ParsedRequest::BudgetCallReconcile(_) => {
            ctx.require(&[Role::Admin])?;
        }
        ParsedRequest::ImportRegister(request) => {
            ctx.require(&[Role::Admin])?;
            validate_import_register_request(request)?;
        }
        ParsedRequest::ImportExecute(request) => {
            ctx.require(&[Role::Admin])?;
            identifier(&request.selection_id)?;
        }
        ParsedRequest::AssetStage(request) => {
            ctx.require(&[Role::Admin])?;
            identifier(&request.request_key)?;
            crate::packages::validate_digest(&request.input_blob_digest, "asset stage input blob")?;
        }
        ParsedRequest::AssetAbort(request) => {
            ctx.require(&[Role::Admin])?;
            identifier(&request.staged_asset_id)?;
        }
        ParsedRequest::AssetHandoff(request) => {
            ctx.require(&[Role::Admin])?;
            identifier(&request.staged_asset_id)?;
            identifier(&request.candidate_id)?;
        }
        ParsedRequest::ExportPrepare(request) => {
            ctx.require(&[Role::Admin])?;
            identifier(&request.staged_asset_id)?;
        }
        ParsedRequest::ExportComplete(request) | ParsedRequest::ExportAbort(request) => {
            ctx.require(&[Role::Admin])?;
            identifier(&request.export_id)?;
        }
        ParsedRequest::TraceExportPrepare(request) => {
            ctx.require(&[Role::Admin])?;
            validate_trace_export_prepare_request(request)?;
        }
        ParsedRequest::TraceExportPublish(request) => {
            ctx.require(&[Role::Admin])?;
            validate_trace_export_publish_request(request)?;
        }
        _ => {}
    }
    Ok(())
}

/// Trace export identifiers are exact single-segment identifiers: an `id` of
/// the caller's choosing with no dotted or colon-separated compound inside.
fn trace_export_segment_identifier(value: &str) -> Result<()> {
    identifier(value)?;
    if value.contains(['.', ':']) {
        return Err(Error::Invalid(
            "trace export identifiers are single segment".into(),
        ));
    }
    Ok(())
}

fn validate_trace_export_prepare_request(request: &TraceExportPrepareRequest) -> Result<()> {
    trace_export_segment_identifier(&request.export_id)?;
    if request.source_ids.is_empty() || request.source_ids.len() > crate::packages::MAX_FILES - 2 {
        return Err(Error::Invalid(
            "trace export source count is invalid".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for id in &request.source_ids {
        trace_export_segment_identifier(id)?;
        if !seen.insert(id.as_str()) {
            return Err(Error::Invalid("duplicate trace export source".into()));
        }
    }
    Ok(())
}

fn validate_trace_export_publish_request(request: &TraceExportPublishRequest) -> Result<()> {
    trace_export_segment_identifier(&request.export_id)?;
    crate::packages::validate_digest(&request.candidate_digest, "trace export candidate digest")
}

fn validate_import_register_request(request: &ImportRegisterManagementRequest) -> Result<()> {
    identifier(&request.request_key)?;
    if request.sources.is_empty()
        || request.sources.len() > evo_core::evidence::MAX_DISCOVERED_FILES
    {
        return Err(Error::Invalid("import requires 1..=200 sources".into()));
    }
    let mut seen = BTreeSet::new();
    for source in &request.sources {
        identifier(&source.source_id)?;
        crate::packages::validate_digest(&source.blob_digest, "import blob digest")?;
        if !seen.insert(source.source_id.as_str()) {
            return Err(Error::Conflict("duplicate import source id".into()));
        }
    }
    Ok(())
}

/// Why a job's private input could not be turned into a request.
#[derive(Debug)]
enum InputFault {
    /// The revocation cleanup replaced the input with an `rsia.redacted.v1`
    /// tombstone: a source of the request was revoked (plan §11.5). Expected
    /// after a revocation, not corruption.
    Redacted { id: String },
    /// Anything else: absent, damaged, differing from its job, not decodable.
    Other(Error),
}

impl From<Error> for InputFault {
    fn from(error: Error) -> Self {
        Self::Other(error)
    }
}

impl InputFault {
    /// The error a caller reports. A redacted input is a `Conflict` that names
    /// the input and the revocation, never `Internal`.
    fn into_error(self) -> Error {
        match self {
            Self::Redacted { id } => redacted_input_error(&id),
            Self::Other(error) => error,
        }
    }

    /// The `error_code` a job that hit this fault is recorded with.
    fn error_code(&self) -> &'static str {
        match self {
            Self::Redacted { .. } => SOURCE_REVOKED_CODE,
            Self::Other(error) => error_code(error),
        }
    }
}

fn redacted_input_error(id: &str) -> Error {
    Error::Conflict(format!(
        "management private input {id} was redacted because its source was revoked"
    ))
}

/// The stored body of a job's private input, `None` when it is absent.
///
/// The body is read as plain JSON so that the two expected shapes (the input
/// itself, or the cleanup's tombstone in its place) are told apart before any
/// typed decoding; the tombstone is not an input, and decoding it as one would
/// surface as `Internal` and raise the storage layer's "database operation
/// failed" error log for an expected state.
async fn private_input_body_ref(
    ctx: &Context,
    store: &Store,
    private_input_ref: &str,
) -> Result<Option<Value>> {
    let mut session = store.session().await?;
    let result = private_input_body_in_session(ctx, &mut session, private_input_ref).await?;
    session.commit().await?;
    Ok(result)
}

async fn private_input_body_in_session(
    ctx: &Context,
    session: &mut Session,
    private_input_ref: &str,
) -> Result<Option<Value>> {
    let Some(row) = session
        .object_row(ctx, "artifact", private_input_ref)
        .await?
    else {
        return Ok(None);
    };
    if row.owner != ctx.actor() {
        return Err(Error::NotFound);
    }
    let value = evo_storage::lifecycle::parse_strict_json(&row.body)
        .map_err(|_| Error::Invalid("management_private_input_invalid_json".into()))?;
    Ok(Some(value))
}

fn is_redacted(body: &Value) -> bool {
    body.get("schema_version").and_then(Value::as_str) == Some(REDACTED_SCHEMA)
}

/// `Conflict` when the job's private input was redacted by the cleanup. An
/// absent or ordinary input passes: this gate names one expected state and
/// decides nothing else.
async fn ensure_private_input_not_redacted_record(
    ctx: &Context,
    store: &Store,
    job: &VersionedManagementJob,
) -> Result<()> {
    match private_input_body_ref(ctx, store, job.identity().private_input_ref).await? {
        Some(body) if is_redacted(&body) => {
            Err(redacted_input_error(job.identity().private_input_ref))
        }
        _ => Ok(()),
    }
}

async fn load_private_request_record(
    ctx: &Context,
    store: &Store,
    job: &VersionedManagementJob,
) -> std::result::Result<ParsedRequest, InputFault> {
    let identity = job.identity();
    let body = private_input_body_ref(ctx, store, identity.private_input_ref)
        .await?
        .ok_or(Error::NotFound)?;
    if is_redacted(&body) {
        return Err(InputFault::Redacted {
            id: identity.private_input_ref.into(),
        });
    }
    Ok(decode_private_request_fields(
        body,
        identity.private_input_ref,
        identity.operation,
        identity.payload_digest,
    )?)
}

async fn load_private_request(
    ctx: &Context,
    store: &Store,
    job: &ManagementJobV2,
) -> std::result::Result<ParsedRequest, InputFault> {
    let body = private_input_body_ref(ctx, store, &job.private_input_ref)
        .await?
        .ok_or(Error::NotFound)?;
    if is_redacted(&body) {
        return Err(InputFault::Redacted {
            id: job.private_input_ref.clone(),
        });
    }
    Ok(decode_private_request(body, job)?)
}

/// Decodes and cross-checks a stored (not redacted) private input against its job.
fn decode_private_request(body: Value, job: &ManagementJobV2) -> Result<ParsedRequest> {
    decode_private_request_fields(
        body,
        &job.private_input_ref,
        &job.operation,
        &job.payload_digest,
    )
}

// Both raw versioned reads use the same original private-input/ParsedRequest
// verifier, without converting an admitted job back through Value or inventing
// a v1 job under the new schema.
fn decode_private_request_fields(
    body: Value,
    private_input_ref: &str,
    operation: &str,
    payload_digest: &str,
) -> Result<ParsedRequest> {
    let private: PrivateManagementInput = match serde_json::from_value(body) {
        Ok(private) => private,
        Err(error) => {
            tracing::error!(%error, id = %private_input_ref, "stored management private input does not decode");
            return Err(Error::Internal);
        }
    };
    if private.id != private_input_ref
        || private.schema_version != INPUT_SCHEMA
        || private.operation != operation
        || private.payload_digest != payload_digest
    {
        return Err(Error::Conflict(
            "private management input differs from job".into(),
        ));
    }
    let request: ParsedRequest = serde_json::from_value(private.payload)
        .map_err(|_| Error::Invalid("invalid persisted management input".into()))?;
    if fingerprint(&request)? != payload_digest {
        return Err(Error::Conflict(
            "persisted management payload digest changed".into(),
        ));
    }
    Ok(request)
}

fn same_claim_fields(current: &ManagementJobV2, claimed: &ManagementJobV2) -> bool {
    current.id == claimed.id
        && current.owner_actor == claimed.owner_actor
        && current.owner_role == claimed.owner_role
        && current.operation == claimed.operation
        && current.request_key == claimed.request_key
        && current.payload_digest == claimed.payload_digest
        && current.private_input_ref == claimed.private_input_ref
        && current.generation == claimed.generation
        && current.lease_token == claimed.lease_token
}

async fn begin_management_step(
    ctx: &Context,
    session: &mut Session,
    claimed: &ManagementJobV2,
    control: &WorkerControl,
) -> Result<ManagementJobV2> {
    let record = load_job_record_in_session(ctx, session, &claimed.id).await?;
    if is_terminal(record.identity().state) {
        return Err(Error::Conflict("management job lease fence changed".into()));
    }
    let current = record.into_unfinished_v2()?;
    ensure_fence(&current, claimed)?;
    control.admit_step()?;
    if current.cancel_requested {
        return Err(Error::Cancelled);
    }
    let body = private_input_body_in_session(ctx, session, &current.private_input_ref)
        .await?
        .ok_or(Error::NotFound)?;
    if is_redacted(&body) {
        return Err(redacted_input_error(&current.private_input_ref));
    }
    let request = decode_private_request(body, &current)?;
    validate_request_authority(ctx, &request)?;
    ensure_dependencies_live(ctx, session, &private_dependencies(&request)?).await?;
    Ok(current)
}

async fn reload_same_claim_after_write(
    ctx: &Context,
    session: &mut Session,
    claimed: &ManagementJobV2,
) -> Result<ManagementJobV2> {
    let record = load_job_record_in_session(ctx, session, &claimed.id).await?;
    if is_terminal(record.identity().state) {
        return Err(Error::Conflict("management job lease fence changed".into()));
    }
    let current = record.into_unfinished_v2()?;
    if !same_claim_fields(&current, claimed) {
        return Err(Error::Conflict("management job lease fence changed".into()));
    }
    Ok(current)
}

async fn checkpoint_claim(
    ctx: &Context,
    store: &Store,
    claimed: &ManagementJobV2,
    step: &str,
    control: &WorkerControl,
) -> Result<()> {
    let mut session = store.session().await?;
    let mut current = begin_management_step(ctx, &mut session, claimed, control).await?;
    current.step = step.into();
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.job.checkpoint", &current.id)
        .await?;
    session.commit().await
}

#[derive(Clone)]
struct OwnedOptimizationCall {
    reference: ManagementOptimizationCallRef,
    call: evo_storage::budget::BudgetCallRecord,
}

struct OptimizationAccounting {
    calls: Vec<OwnedOptimizationCall>,
    terminal: Option<crate::optimization::StageFact>,
    compiled: Vec<crate::optimization::StageFact>,
    historical_compiled: Vec<ManagementOptimizationStageRef>,
    historical_terminal: bool,
    known_none: bool,
}

impl OptimizationAccounting {
    fn pending(&self) -> bool {
        self.calls.iter().any(|owned| {
            matches!(
                owned.call.state,
                evo_storage::budget::BudgetCallState::Reserved
                    | evo_storage::budget::BudgetCallState::Dispatched
                    | evo_storage::budget::BudgetCallState::Uncertain
            ) || (owned.call.dispatch_id.is_some() && !owned.call.execution_closed)
        })
    }
    fn dispatched(&self) -> bool {
        self.calls
            .iter()
            .any(|owned| owned.call.dispatch_id.is_some())
    }
    fn has_business_side_effect(&self) -> bool {
        self.dispatched() || !self.compiled.is_empty() || !self.historical_compiled.is_empty()
    }
    fn views(&self) -> Vec<ManagementFeeCallView> {
        self.calls
            .iter()
            .map(|owned| ManagementFeeCallView {
                billing_scope: owned.call.billing_scope.clone(),
                call_id: owned.call.call_id.clone(),
                state: owned.call.state,
                actual_cost_micros: owned.call.actual_cost_micros,
                reserved_micros: owned.call.reserved_micros,
                actual_currency: owned.call.actual_currency.clone(),
                actual_pricing_version: owned.call.actual_pricing_version.clone(),
                execution_closed: owned.call.execution_closed,
            })
            .collect()
    }
    fn pending_views(&self) -> Vec<ManagementFeeCallView> {
        self.calls
            .iter()
            .filter(|owned| {
                matches!(
                    owned.call.state,
                    evo_storage::budget::BudgetCallState::Reserved
                        | evo_storage::budget::BudgetCallState::Dispatched
                        | evo_storage::budget::BudgetCallState::Uncertain
                ) || (owned.call.dispatch_id.is_some() && !owned.call.execution_closed)
            })
            .map(|owned| ManagementFeeCallView {
                billing_scope: owned.call.billing_scope.clone(),
                call_id: owned.call.call_id.clone(),
                state: owned.call.state,
                actual_cost_micros: owned.call.actual_cost_micros,
                reserved_micros: owned.call.reserved_micros,
                actual_currency: owned.call.actual_currency.clone(),
                actual_pricing_version: owned.call.actual_pricing_version.clone(),
                execution_closed: owned.call.execution_closed,
            })
            .collect()
    }
}

/// One read Session joins physical job/private input, original prepared facts,
/// actual root and the full original ledger. No query result grants new work.
async fn optimization_live_accounting_in_session(
    ctx: &Context,
    session: &mut Session,
    record: &VersionedManagementJob,
) -> Result<Option<OptimizationAccounting>> {
    let identity = record.identity();
    if identity.operation != "optimization.step" {
        return Ok(None);
    }
    let body = private_input_body_in_session(ctx, session, identity.private_input_ref)
        .await?
        .ok_or(Error::NotFound)?;
    if is_redacted(&body) {
        return Ok(None);
    }
    let parsed = decode_private_request_fields(
        body,
        identity.private_input_ref,
        identity.operation,
        identity.payload_digest,
    )?;
    let ParsedRequest::OptimizationStep(request) = parsed else {
        return Ok(None);
    };
    let VersionedManagementJob::V2(job) = record else {
        return Ok(None);
    };
    let producer_identity = fingerprint(&(ctx.namespace(), ctx.actor(), &job.id))?;
    let episode = format!("management-episode-{producer_identity}");
    let step_request = format!("management-opt-{producer_identity}");
    let scopes = session.budget_call_scopes_for_group(ctx, &episode).await?;
    if job.optimization.is_none() {
        // This is the real producer's never-entered phase, not an empty-query
        // assertion: every later step/call must append its reference atomically.
        let before = (job.state == ManagementJobState::Queued
            && job.generation == 0
            && job.lease_token.is_none())
            || matches!(job.step.as_str(), "claimed" | "before_optimization_resolve")
            || (job.state == ManagementJobState::Cancelled
                && job.generation == 0
                && job.lease_token.is_none()
                && job.step == "cancelled_before_claim")
            || (job.state == ManagementJobState::Blocked
                && job.step == "optimization_backend_unconfigured"
                && job.error_code.as_deref() == Some("optimization_backend_unconfigured"));
        return Ok(
            (before && scopes.is_empty()).then(|| OptimizationAccounting {
                calls: Vec::new(),
                terminal: None,
                compiled: Vec::new(),
                historical_compiled: Vec::new(),
                historical_terminal: false,
                known_none: true,
            }),
        );
    }
    let progress = job.optimization.as_ref().ok_or(Error::Internal)?;
    let control = crate::development::management_control_in_session(
        session,
        ctx,
        &request.development_control_id,
    )
    .await?;
    let mut facts = std::collections::BTreeMap::new();
    let mut step = None;
    for reference in &progress.stages {
        if reference.generation == 0 || reference.generation > job.generation {
            return Ok(None);
        }
        let row = session
            .object_row(ctx, "artifact", &reference.artifact_id)
            .await?
            .ok_or(Error::NotFound)?;
        if row.owner != control.proposer_actor {
            return Ok(None);
        }
        let fact: crate::optimization::StageFact =
            serde_json::from_value(evo_storage::lifecycle::parse_strict_json(&row.body)?)
                .map_err(|_| Error::Conflict("management_optimization_origin_unknown".into()))?;
        fact.validate()?;
        if fact.artifact_id != reference.artifact_id
            || fact.input_digest != reference.input_digest
            || fact.namespace != ctx.namespace()
            || fact.episode_id != episode
            || fact.step != 0
            || fact.attempt != 0
        {
            return Ok(None);
        }
        if fact.kind == crate::optimization::StageFactKind::StepPrepared {
            if fact.request_id != step_request || step.is_some() {
                return Ok(None);
            }
            let original: crate::formal_authority::CompilerInputRequest = serde_json::from_value(
                fact.payload
                    .get("compiler_request")
                    .cloned()
                    .ok_or(Error::NotFound)?,
            )
            .map_err(|_| Error::Conflict("management_optimization_origin_unknown".into()))?;
            let binding: crate::formal_authority::CurrentFormalCompilerInputBinding =
                serde_json::from_value(
                    fact.payload
                        .get("compiler_binding")
                        .cloned()
                        .ok_or(Error::NotFound)?,
                )
                .map_err(|_| Error::Conflict("management_optimization_origin_unknown".into()))?;
            if fingerprint(&original)? != fingerprint(&request.compiler)? {
                return Ok(None);
            }
            let mut input = fact.payload.clone();
            let object = input.as_object_mut().ok_or(Error::NotFound)?;
            object.remove("compiler_request");
            object.remove("compiler_binding");
            if fact.schema_version == crate::optimization::OPTIMIZATION_STAGE_FACT_SCHEMA_V2 {
                object.remove("job_scope");
            }
            if fact.input_digest != fingerprint(&(fingerprint(&input)?, &binding))? {
                return Ok(None);
            }
            step = Some(fact.clone());
        }
        if facts.insert(fact.artifact_id.clone(), fact).is_some() {
            return Ok(None);
        }
    }
    let Some(step) = step else { return Ok(None) };
    let terminal_id = crate::optimization::management_completed_stage_id(&step)?;
    let terminal = if let Some(row) = session.object_row(ctx, "artifact", &terminal_id).await? {
        if row.owner != control.proposer_actor {
            return Ok(None);
        }
        let fact: crate::optimization::StageFact =
            serde_json::from_value(evo_storage::lifecycle::parse_strict_json(&row.body)?)
                .map_err(|_| Error::Conflict("management_optimization_origin_unknown".into()))?;
        fact.validate()?;
        if fact.artifact_id != terminal_id
            || fact.input_digest != step.input_digest
            || fact.namespace != ctx.namespace()
            || fact.episode_id != episode
            || fact.request_id != step_request
        {
            return Ok(None);
        }
        crate::optimization::management_completed_outcome(&fact)?;
        Some(fact)
    } else {
        None
    };
    let mut owned = Vec::new();
    let mut referenced = BTreeSet::new();
    for reference in &progress.calls {
        if reference.generation == 0
            || reference.generation > job.generation
            || !referenced.insert((reference.billing_scope.clone(), reference.call_id.clone()))
        {
            return Ok(None);
        }
        let root = session
            .root_budget(ctx, &reference.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        if root.root_budget_id != reference.root_budget_id
            || root.root_budget_id != control.root_budget_id
            || root.billing_scope != control.billing_scope
        {
            return Ok(None);
        }
        let call = session
            .budget_call(ctx, &reference.billing_scope, &reference.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        let artifact = call.request_artifact.as_ref().ok_or(Error::NotFound)?;
        let prepared = facts
            .get(&reference.prepared_stage_id)
            .ok_or(Error::NotFound)?;
        let producer = progress
            .stages
            .iter()
            .find(|stage| stage.artifact_id == reference.prepared_stage_id)
            .ok_or(Error::NotFound)?;
        if call.namespace != ctx.namespace()
            || call.dispatch_group_id != episode
            || call.actual_input_digest != reference.actual_input_digest
            || artifact.schema_version != reference.request_schema
            || artifact.digest != reference.request_digest
            || hash(artifact.body.as_bytes()) != artifact.digest
            || producer.generation != reference.generation
            || prepared.kind != crate::optimization::StageFactKind::DispatchPrepared
        {
            return Ok(None);
        }
        match reference.producer {
            ManagementOptimizationCallProducer::Model => {
                let bound: crate::model::ReadBoundModelRequest = serde_json::from_value(
                    evo_storage::lifecycle::parse_strict_json(&artifact.body)?,
                )
                .map_err(|_| Error::Conflict("management_optimization_origin_unknown".into()))?;
                bound.validate()?;
                if bound.schema_version != crate::model::CURRENT_BOUND_MODEL_SCHEMA
                    || !bound.compiler_use.is_current()
                    || bound.request.namespace != ctx.namespace()
                    || bound.request.episode_id != episode
                    || bound.request.request_id != call.call_id
                    || bound.request.cache_key_digest != call.actual_input_digest
                    || prepared.request_id != call.call_id
                    || prepared.input_digest != fingerprint(&bound)?
                    || fingerprint(&bound.compiler_use.request)? != fingerprint(&request.compiler)?
                    || fingerprint(&prepared.payload)?
                        != fingerprint(&serde_json::to_value(&bound).map_err(|_| Error::Internal)?)?
                {
                    return Ok(None);
                }
            }
            ManagementOptimizationCallProducer::Development => {
                let original: crate::optimization::DevelopmentRunRequest =
                    serde_json::from_value(prepared.payload.clone()).map_err(|_| {
                        Error::Conflict("management_optimization_origin_unknown".into())
                    })?;
                if prepared.request_id != original.request_id
                    || prepared.input_digest != fingerprint(&original)?
                {
                    return Ok(None);
                }
                crate::development::verify_management_budget_call(session, ctx, &call, &original)
                    .await?;
            }
        }
        let budget_ref_id = optimization_budget_ref_id(
            ctx.namespace(),
            &reference.billing_scope,
            &reference.call_id,
        )?;
        let budget_ref = session
            .object_row(ctx, "artifact", &budget_ref_id)
            .await?
            .ok_or(Error::NotFound)?;
        if budget_ref.owner != reference.producer_actor
            || hash(budget_ref.body.as_bytes()) != reference.budget_ref_digest
        {
            return Ok(None);
        }
        validate_optimization_budget_ref(
            &budget_ref.body,
            &budget_ref_id,
            ctx.namespace(),
            &reference.billing_scope,
            &reference.call_id,
        )?;
        owned.push(OwnedOptimizationCall {
            reference: reference.clone(),
            call,
        });
    }
    let mut actual = BTreeSet::new();
    for scope in scopes {
        for call in session
            .budget_calls_for_group(ctx, &scope, &episode)
            .await?
        {
            actual.insert((call.billing_scope, call.call_id));
        }
    }
    if actual != referenced {
        return Ok(None);
    }
    // A no-call completion also needs the actual terminal and every attempted
    // model dispatch's original NotDispatched observation. Empty refs are insufficient.
    let mut proved_no_dispatch = terminal.is_some();
    for prepared in facts
        .values()
        .filter(|fact| fact.kind == crate::optimization::StageFactKind::DispatchPrepared)
    {
        if progress
            .calls
            .iter()
            .any(|reference| reference.prepared_stage_id == prepared.artifact_id)
        {
            continue;
        }
        let observed_id = crate::optimization::management_observed_stage_id(prepared)?;
        let Some(row) = session.object_row(ctx, "artifact", &observed_id).await? else {
            proved_no_dispatch = false;
            continue;
        };
        let observed: crate::optimization::StageFact =
            serde_json::from_value(evo_storage::lifecycle::parse_strict_json(&row.body)?)
                .map_err(|_| Error::Conflict("management_optimization_origin_unknown".into()))?;
        observed.validate()?;
        let response: crate::model::ModelResponse =
            match serde_json::from_value(observed.payload.clone()) {
                Ok(response) => response,
                Err(_) => {
                    proved_no_dispatch = false;
                    continue;
                }
            };
        if row.owner != control.proposer_actor
            || observed.artifact_id != observed_id
            || observed.input_digest != prepared.input_digest
            || !matches!(
                response,
                crate::model::ModelResponse::Rejected {
                    dispatch: crate::model::RejectedDispatch::NotDispatched,
                    ..
                }
            )
        {
            proved_no_dispatch = false;
        }
    }
    let known_none = owned.is_empty() && proved_no_dispatch;
    if owned.is_empty() && !known_none {
        return Ok(None);
    }
    let compiled = facts
        .values()
        .filter(|fact| fact.kind == crate::optimization::StageFactKind::EditCompiled)
        .cloned()
        .collect();
    Ok(Some(OptimizationAccounting {
        calls: owned,
        terminal,
        compiled,
        historical_compiled: Vec::new(),
        historical_terminal: false,
        known_none,
    }))
}

fn optimization_terminal_outcome(
    fact: &crate::optimization::StageFact,
    compiled: &[crate::optimization::StageFact],
) -> Result<JobOutcome> {
    optimization_terminal_outcome_with_compiled(fact, !compiled.is_empty())
}

fn optimization_terminal_outcome_with_compiled(
    fact: &crate::optimization::StageFact,
    has_compiled: bool,
) -> Result<JobOutcome> {
    let outcome = crate::optimization::management_completed_outcome(fact)?;
    let stopped_after_compile = has_compiled
        && match &outcome {
            crate::optimization::OptimizationStepOutcome::Uncertain { .. } => true,
            crate::optimization::OptimizationStepOutcome::Rejected { class } => !matches!(
                class,
                crate::optimization::StepTerminalClass::KeepIncumbentNotImproved { .. }
                    | crate::optimization::StepTerminalClass::KeepIncumbentRetentionBroken { .. }
            ),
            _ => false,
        };
    if stopped_after_compile {
        return Ok(optimization_partial_outcome());
    }
    let (state, error_code) = match outcome {
        crate::optimization::OptimizationStepOutcome::Rejected { class }
        | crate::optimization::OptimizationStepOutcome::Uncertain { class } => {
            (ManagementJobState::Blocked, Some(class.code().into()))
        }
        _ => (ManagementJobState::Succeeded, None),
    };
    Ok(JobOutcome {
        state,
        step: "optimization_completed".into(),
        result: Some(ManagementResultV2::Optimization(
            ManagementOptimizationResultV2::OptimizationStepCompleted {
                stage_fact_id: fact.artifact_id.clone(),
                input_digest: fact.input_digest.clone(),
            },
        )),
        error_code,
    })
}

fn optimization_partial_outcome() -> JobOutcome {
    JobOutcome {
        state: ManagementJobState::Blocked,
        step: "optimization_partial_commit".into(),
        result: None,
        error_code: Some("blocked_partial_commit".into()),
    }
}

fn optimization_stage_kind(kind: crate::optimization::StageFactKind) -> Result<String> {
    serde_json::to_value(kind)
        .map_err(|_| Error::Internal)?
        .as_str()
        .map(str::to_owned)
        .ok_or(Error::Internal)
}

fn optimization_stage_not_dispatched(fact: &crate::optimization::StageFact) -> bool {
    matches!(
        fact.kind,
        crate::optimization::StageFactKind::DispatchObserved
    ) && matches!(
        serde_json::from_value::<crate::model::ModelResponse>(fact.payload.clone()),
        Ok(crate::model::ModelResponse::Rejected {
            dispatch: crate::model::RejectedDispatch::NotDispatched,
            ..
        })
    )
}

fn optimization_stage_record_digest(fact: &crate::optimization::StageFact) -> Result<String> {
    // Exactly Session::put and commit_redacted's encoding, in original field order.
    Ok(hash(
        serde_json::to_string(fact)
            .map_err(|_| Error::Internal)?
            .as_bytes(),
    ))
}

fn optimization_budget_ref_id(namespace: &str, scope: &str, call_id: &str) -> Result<String> {
    let digest = fingerprint(&("rsia.budget_call_ref.v1", namespace, scope, call_id))?;
    Ok(format!("budget-ref-{}", &digest[..32]))
}

fn validate_optimization_budget_ref(
    body: &str,
    id: &str,
    namespace: &str,
    scope: &str,
    call_id: &str,
) -> Result<()> {
    let reference: evo_storage::budget::BudgetCallRef =
        serde_json::from_value(evo_storage::lifecycle::parse_strict_json(body)?)
            .map_err(|_| Error::Conflict("management_optimization_origin_unknown".into()))?;
    if reference.id != id
        || reference.schema_version != "rsia.budget_call_ref.v1"
        || reference.namespace != namespace
        || reference.billing_scope != scope
        || reference.call_id != call_id
        || reference
            .source_ids
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(Error::Conflict(
            "management_optimization_origin_unknown".into(),
        ));
    }
    Ok(())
}

// Exactly lifecycle's existing object redaction, with its original raw TEXT
// digest. These checks authorize historical reads only, never a new source use.
fn optimization_object_redaction_matches(
    value: &Value,
    id: &str,
    schema: &str,
    original_digest: &str,
) -> bool {
    value.as_object().is_some_and(|object| object.len() == 7)
        && value["schema_version"].as_str() == Some(REDACTED_SCHEMA)
        && value["state"].as_str() == Some("source_revoked")
        && value["original_kind"].as_str() == Some("artifact")
        && value["id"].as_str() == Some(id)
        && value["original_schema"].as_str() == Some(schema)
        && value["original_digest"].as_str() == Some(original_digest)
        && value["metadata"].is_object()
}

fn optimization_stage_reference_matches(
    body: &str,
    reference: &ManagementOptimizationStageRef,
    namespace: &str,
    episode: &str,
) -> Result<bool> {
    let value = evo_storage::lifecycle::parse_strict_json(body)?;
    if is_redacted(&value) {
        let schema = match value["original_schema"].as_str() {
            Some(crate::optimization::OPTIMIZATION_STAGE_FACT_SCHEMA) => {
                crate::optimization::OPTIMIZATION_STAGE_FACT_SCHEMA
            }
            Some(crate::optimization::OPTIMIZATION_STAGE_FACT_SCHEMA_V2)
                if reference.kind == "step_prepared" =>
            {
                crate::optimization::OPTIMIZATION_STAGE_FACT_SCHEMA_V2
            }
            _ => return Ok(false),
        };
        return Ok(optimization_object_redaction_matches(
            &value,
            &reference.artifact_id,
            schema,
            &reference.record_digest,
        ) && value["metadata"]["artifact_id"].as_str()
            == Some(reference.artifact_id.as_str())
            && value["metadata"]["kind"].as_str() == Some(reference.kind.as_str())
            && value["metadata"]["request_id"].as_str() == Some(reference.request_id.as_str())
            && value["metadata"]["namespace"].as_str() == Some(namespace)
            && value["metadata"]["episode_id"].as_str() == Some(episode)
            && value["metadata"]["step"].as_u64() == Some(0)
            && value["metadata"]["attempt"].as_u64() == Some(0)
            && value["metadata"]["input_digest"].as_str()
                == Some(reference.input_digest.as_str()));
    }
    let fact: crate::optimization::StageFact = serde_json::from_value(value)
        .map_err(|_| Error::Conflict("management_optimization_origin_unknown".into()))?;
    fact.validate()?;
    Ok(hash(body.as_bytes()) == reference.record_digest
        && fact.artifact_id == reference.artifact_id
        && optimization_stage_kind(fact.kind)? == reference.kind
        && fact.request_id == reference.request_id
        && optimization_stage_not_dispatched(&fact) == reference.dispatch_not_performed
        && fact.input_digest == reference.input_digest
        && fact.namespace == namespace
        && fact.episode_id == episode
        && fact.step == 0
        && fact.attempt == 0)
}

fn optimization_request_history_matches(
    artifact: &evo_storage::budget::BudgetArtifact,
    reference: &ManagementOptimizationCallRef,
) -> Result<bool> {
    if hash(artifact.body.as_bytes()) != artifact.digest {
        return Ok(false);
    }
    if artifact.schema_version == reference.request_schema
        && artifact.digest == reference.request_digest
    {
        return Ok(true);
    }
    let value = evo_storage::lifecycle::parse_strict_json(&artifact.body)?;
    Ok(artifact.schema_version == REDACTED_SCHEMA
        && value.as_object().is_some_and(|object| object.len() == 5)
        && value["schema_version"].as_str() == Some(REDACTED_SCHEMA)
        && value["state"].as_str() == Some("source_revoked")
        && matches!(
            value["reason"].as_str(),
            Some("source_revoked" | "budget_sources_unverifiable" | "e16_source_unavailable")
        )
        && value["original_schema"].as_str() == Some(reference.request_schema.as_str())
        && value["original_digest"].as_str() == Some(reference.request_digest.as_str()))
}

/// The same already-produced references after legal redaction. No request body,
/// Current authority or money is reconstructed. A missing or changed witness is
/// unknown; an empty call query cannot prove a no-call history here.
async fn optimization_historical_accounting_in_session(
    ctx: &Context,
    session: &mut Session,
    record: &VersionedManagementJob,
) -> Result<Option<OptimizationAccounting>> {
    let VersionedManagementJob::V2(job) = record else {
        return Ok(None);
    };
    if job.operation != "optimization.step" {
        return Ok(None);
    }
    let Some(progress) = &job.optimization else {
        return Ok(None);
    };
    if progress.private_input_digest.is_empty() || progress.journal_actor.is_empty() {
        return Ok(None);
    }
    let private = session
        .object_row(ctx, "artifact", &job.private_input_ref)
        .await?
        .ok_or(Error::NotFound)?;
    if private.owner != job.owner_actor || private.owner != ctx.actor() {
        return Ok(None);
    }
    let body = evo_storage::lifecycle::parse_strict_json(&private.body)?;
    if is_redacted(&body) {
        if !optimization_object_redaction_matches(
            &body,
            &job.private_input_ref,
            INPUT_SCHEMA,
            &progress.private_input_digest,
        ) {
            return Ok(None);
        }
    } else {
        if hash(private.body.as_bytes()) != progress.private_input_digest {
            return Ok(None);
        }
        let parsed = decode_private_request_fields(
            body,
            &job.private_input_ref,
            &job.operation,
            &job.payload_digest,
        )?;
        if !matches!(parsed, ParsedRequest::OptimizationStep(_)) {
            return Ok(None);
        }
    }
    let producer = fingerprint(&(ctx.namespace(), ctx.actor(), &job.id))?;
    let episode = format!("management-episode-{producer}");
    let step_request = format!("management-opt-{producer}");
    let mut facts = std::collections::BTreeMap::new();
    let mut step = None;
    let mut completed = None;
    let mut compiled_refs = Vec::new();
    for reference in &progress.stages {
        if reference.generation == 0
            || reference.generation > job.generation
            || facts
                .insert(reference.artifact_id.clone(), reference)
                .is_some()
        {
            return Ok(None);
        }
        let row = session
            .object_row(ctx, "artifact", &reference.artifact_id)
            .await?
            .ok_or(Error::NotFound)?;
        if row.owner != progress.journal_actor
            || !optimization_stage_reference_matches(
                &row.body,
                reference,
                ctx.namespace(),
                &episode,
            )?
        {
            return Ok(None);
        }
        if reference.kind == "step_prepared"
            && (reference.request_id != step_request || step.replace(reference).is_some())
        {
            return Ok(None);
        }
        if reference.kind == "step_completed"
            && (reference.request_id != step_request || completed.replace(reference).is_some())
        {
            return Ok(None);
        }
        if reference.kind == "edit_compiled" {
            compiled_refs.push(reference.clone());
        }
    }
    let Some(step) = step else { return Ok(None) };
    if let Some(completed) = completed {
        if completed.generation != step.generation || completed.input_digest != step.input_digest {
            return Ok(None);
        }
        // The original observational writer committed this result/target in
        // the very transaction that retained this terminal's original digest.
        if job.pending_terminal_state.is_none() {
            return Ok(None);
        }
        match job.result.as_ref() {
            Some(ManagementResultV2::Optimization(
                ManagementOptimizationResultV2::OptimizationStepCompleted {
                    stage_fact_id,
                    input_digest,
                },
            )) if stage_fact_id == &completed.artifact_id
                && input_digest == &completed.input_digest => {}
            None if !compiled_refs.is_empty()
                && job.step == "optimization_partial_commit"
                && job.error_code.as_deref() == Some("blocked_partial_commit") => {}
            _ => return Ok(None),
        }
    }
    let mut owned = Vec::new();
    let mut referenced = BTreeSet::new();
    for reference in &progress.calls {
        if reference.generation == 0
            || reference.generation > job.generation
            || !referenced.insert((reference.billing_scope.clone(), reference.call_id.clone()))
        {
            return Ok(None);
        }
        let prepared = facts
            .get(&reference.prepared_stage_id)
            .ok_or(Error::NotFound)?;
        if prepared.kind != "dispatch_prepared"
            || prepared.generation != reference.generation
            || reference.generation != step.generation
        {
            return Ok(None);
        }
        let root = session
            .root_budget(ctx, &reference.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        if root.root_budget_id != reference.root_budget_id
            || root.billing_scope != reference.billing_scope
            || (root.authorizing_namespace != ctx.namespace()
                && !root
                    .allowed_namespaces
                    .iter()
                    .any(|ns| ns == ctx.namespace()))
        {
            return Ok(None);
        }
        let call = session
            .budget_call(ctx, &reference.billing_scope, &reference.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        if call.namespace != ctx.namespace()
            || call.call_id != reference.call_id
            || call.billing_scope != reference.billing_scope
            || call.dispatch_group_id != episode
            || call.actual_input_digest != reference.actual_input_digest
            || call.lease_token != reference.lease_token
            || call.lease_epoch != reference.lease_epoch
        {
            return Ok(None);
        }
        if !optimization_request_history_matches(
            call.request_artifact.as_ref().ok_or(Error::NotFound)?,
            reference,
        )? {
            return Ok(None);
        }
        let budget_ref_id = optimization_budget_ref_id(
            ctx.namespace(),
            &reference.billing_scope,
            &reference.call_id,
        )?;
        let budget_ref = session
            .object_row(ctx, "artifact", &budget_ref_id)
            .await?
            .ok_or(Error::NotFound)?;
        if budget_ref.owner != reference.producer_actor
            || hash(budget_ref.body.as_bytes()) != reference.budget_ref_digest
        {
            return Ok(None);
        }
        validate_optimization_budget_ref(
            &budget_ref.body,
            &budget_ref_id,
            ctx.namespace(),
            &reference.billing_scope,
            &reference.call_id,
        )?;
        owned.push(OwnedOptimizationCall {
            reference: reference.clone(),
            call,
        });
    }
    let mut actual = BTreeSet::new();
    for scope in session.budget_call_scopes_for_group(ctx, &episode).await? {
        for call in session
            .budget_calls_for_group(ctx, &scope, &episode)
            .await?
        {
            actual.insert((call.billing_scope, call.call_id));
        }
    }
    if actual != referenced {
        return Ok(None);
    }
    // Exact immutable observational refs, recorded by the real journal in its
    // producer transaction, prove every attempted dispatch never entered I/O.
    // An empty call query alone never grants this historical fact.
    let no_call_proof = completed.is_some()
        && progress
            .stages
            .iter()
            .filter(|reference| reference.kind == "dispatch_prepared")
            .all(|prepared| {
                progress.stages.iter().any(|observed| {
                    observed.kind == "dispatch_observed"
                        && observed.dispatch_not_performed
                        && observed.generation == prepared.generation
                        && observed.request_id == prepared.request_id
                        && observed.input_digest == prepared.input_digest
                })
            });
    let known_none = owned.is_empty() && no_call_proof;
    if owned.is_empty() && !known_none {
        return Ok(None);
    }
    Ok(Some(OptimizationAccounting {
        calls: owned,
        terminal: None,
        compiled: Vec::new(),
        historical_compiled: compiled_refs,
        historical_terminal: completed.is_some(),
        known_none,
    }))
}

async fn optimization_accounting_in_session(
    ctx: &Context,
    session: &mut Session,
    record: &VersionedManagementJob,
) -> Result<Option<OptimizationAccounting>> {
    match optimization_live_accounting_in_session(ctx, session, record).await {
        Ok(Some(facts)) => Ok(Some(facts)),
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Ok(None) | Err(_) => {
            optimization_historical_accounting_in_session(ctx, session, record).await
        }
    }
}

fn optimization_accounting_progress(
    facts: &OptimizationAccounting,
    record: &VersionedManagementJob,
) -> Result<ConsumerProgress> {
    if let Some(terminal) = &facts.terminal {
        return Ok(ConsumerProgress::Committed(Box::new(
            optimization_terminal_outcome(terminal, &facts.compiled)?,
        )));
    }
    if facts.historical_terminal {
        let VersionedManagementJob::V2(job) = record else {
            return Ok(ConsumerProgress::Unknown);
        };
        let Some(target) = job.pending_terminal_state else {
            return Ok(ConsumerProgress::Unknown);
        };
        return Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
            state: restored_terminal(target),
            step: job.step.clone(),
            result: job.result.clone(),
            error_code: job.error_code.clone(),
        })));
    }
    if !facts.compiled.is_empty() || !facts.historical_compiled.is_empty() {
        return Ok(ConsumerProgress::Committed(Box::new(
            optimization_partial_outcome(),
        )));
    }
    Ok(ConsumerProgress::NotCommitted)
}

async fn optimization_accounting_or_unknown(
    ctx: &Context,
    session: &mut Session,
    record: &VersionedManagementJob,
) -> Result<Option<OptimizationAccounting>> {
    match optimization_accounting_in_session(ctx, session, record).await {
        Ok(facts) => Ok(facts),
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Err(_) => Ok(None),
    }
}

#[derive(Debug)]
enum ConsumerProgress {
    NotCommitted,
    Committed(Box<JobOutcome>),
    Unknown,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagementCurriculumEnvelope<T> {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: T,
}

/// Read the real existing seven adapters, not an operation-name/empty-call
/// shortcut. No producer or method that can dispatch a model is invoked here.
async fn consumer_progress(
    ctx: &Context,
    store: &Store,
    record: &VersionedManagementJob,
) -> Result<ConsumerProgress> {
    if record.identity().operation == "backup.create" {
        return backup::consumer_progress(ctx, store, record).await;
    }
    if matches!(
        record.identity().operation,
        "import.register" | "import.execute"
    ) {
        // Each import handler writes every business row, the terminal job state
        // and the result in one transaction. A non-terminal record therefore
        // proves no import effect; a terminal one is its own committed outcome.
        if is_terminal(record.identity().state)
            && matches!(record.result(), Some(ManagementResultV2::Import(_)))
        {
            return Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                state: record.identity().state,
                step: record.step().into(),
                result: record.result(),
                error_code: record.error_code().map(str::to_owned),
            })));
        }
        return Ok(ConsumerProgress::NotCommitted);
    }
    if matches!(
        record.identity().operation,
        "asset.stage" | "asset.abort" | "asset.handoff" | "export.prepare" | "export.abort"
    ) {
        // Each of these handlers writes every business row and the terminal job
        // state in one transaction. A non-terminal record proves no effect; a
        // terminal one with its bound result is its own committed outcome.
        if is_terminal(record.identity().state)
            && matches!(record.result(), Some(ManagementResultV2::AssetExport(_)))
        {
            return Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                state: record.identity().state,
                step: record.step().into(),
                result: record.result(),
                error_code: record.error_code().map(str::to_owned),
            })));
        }
        return Ok(ConsumerProgress::NotCommitted);
    }
    if record.identity().operation == "export.complete"
        && is_terminal(record.identity().state)
        && matches!(
            record.result(),
            Some(ManagementResultV2::AssetExport(
                ManagementAssetExportResultV2::ExportCompleted { .. }
            ))
        )
    {
        return Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
            state: record.identity().state,
            step: record.step().into(),
            result: record.result(),
            error_code: record.error_code().map(str::to_owned),
        })));
    }
    if let VersionedManagementJob::V2(job) = record
        && job.operation == "experiment.register"
        && job.step == "current_holdout_registered"
    {
        let Some(ManagementResult::ExperimentRegistered {
            control_id,
            control_digest,
            holdout_id,
            holdout_digest,
        }) = record.original_result()
        else {
            return Ok(ConsumerProgress::Unknown);
        };
        // This actual producer checkpoint saved the Full references with its
        // H commit. Historical verification does not reuse the private source.
        let mut session = store.session().await?;
        let progress = IndependentEvaluationControl::registration_progress_in_session(
            ctx,
            &mut session,
            control_id,
            holdout_id,
        )
        .await?;
        session.commit().await?;
        if progress.control_digest.as_ref() != Some(control_digest)
            || progress.holdout_digest.as_ref() != Some(holdout_digest)
        {
            return Ok(ConsumerProgress::Unknown);
        }
        return Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
            state: ManagementJobState::Succeeded,
            step: job.step.clone(),
            result: job.result.clone(),
            error_code: None,
        })));
    }
    if record.identity().operation == "optimization.step" {
        let mut session = store.session().await?;
        let current = load_job_record_in_session(ctx, &mut session, record.identity().id).await?;
        let facts = optimization_accounting_or_unknown(ctx, &mut session, &current).await?;
        let progress = match facts {
            Some(facts) => optimization_accounting_progress(&facts, &current)?,
            None => ConsumerProgress::Unknown,
        };
        session.commit().await?;
        return Ok(progress);
    }
    let request = load_private_request_record(ctx, store, record)
        .await
        .map_err(InputFault::into_error)?;
    validate_request_authority(ctx, &request)?;
    if !matches!(&request, ParsedRequest::ReplayRun(_)) {
        let mut sources = store.session().await?;
        ensure_dependencies_live(ctx, &mut sources, &private_dependencies(&request)?).await?;
        sources.commit().await?;
    }
    match request {
        ParsedRequest::ExperimentRegister(request) => {
            let mut session = store.session().await?;
            let progress = IndependentEvaluationControl::registration_progress_in_session(
                ctx,
                &mut session,
                &request.control.id,
                &request.holdout.id,
            )
            .await?;
            let c = request.control.digest()?;
            let h = request.holdout.digest(&request.control)?;
            if progress.control_digest.as_deref().is_some_and(|v| v != c)
                || progress.holdout_digest.as_deref().is_some_and(|v| v != h)
            {
                return Err(Error::Conflict(
                    "registered experiment content differs".into(),
                ));
            }
            session.commit().await?;
            // In the known legacy writer these phases were reached only after
            // this job's control commit. A bare matching pre-existing C does
            // not prove that this request created it.
            let recorded_control = record.generation() > 0
                && matches!(
                    record.step(),
                    "control_registered" | "before_holdout_registration" | "experiment_registered"
                );
            let saved_control = match record.result() {
                Some(ManagementResultV2::Partial(
                    ManagementPartialResultV2::ControlRegistered {
                        control_id,
                        control_digest,
                    },
                )) => control_id == request.control.id && control_digest == c,
                Some(ManagementResultV2::Original(ManagementResult::ExperimentRegistered {
                    control_id,
                    control_digest,
                    ..
                })) => control_id == request.control.id && control_digest == c,
                _ => false,
            };
            if progress.control_digest.is_some() && !recorded_control && !saved_control {
                return Ok(ConsumerProgress::Unknown);
            }
            match (progress.control_digest, progress.holdout_digest) {
                (None, None) => Ok(ConsumerProgress::NotCommitted),
                (Some(control_digest), None) => {
                    Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                        state: ManagementJobState::Blocked,
                        step: "control_registered".into(),
                        result: Some(ManagementResultV2::Partial(
                            ManagementPartialResultV2::ControlRegistered {
                                control_id: request.control.id,
                                control_digest,
                            },
                        )),
                        error_code: Some("blocked_partial_commit".into()),
                    })))
                }
                (Some(control_digest), Some(holdout_digest)) => {
                    Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                        state: ManagementJobState::Succeeded,
                        step: "experiment_registered".into(),
                        result: Some(ManagementResultV2::Original(
                            ManagementResult::ExperimentRegistered {
                                control_id: request.control.id,
                                control_digest,
                                holdout_id: request.holdout.id,
                                holdout_digest,
                            },
                        )),
                        error_code: None,
                    })))
                }
                _ => Err(Error::Conflict(
                    "holdout exists without its registered control".into(),
                )),
            }
        }
        ParsedRequest::ExperimentRegisterCurrent(request) => {
            let mut session = store.session().await?;
            let progress = IndependentEvaluationControl::registration_progress_in_session(
                ctx,
                &mut session,
                &request.control.id,
                &request.holdout.id,
            )
            .await?;
            let c = request.control.digest()?;
            let h = request.holdout.digest(&request.control)?;
            if progress.control_digest.as_deref().is_some_and(|v| v != c)
                || progress.holdout_digest.as_deref().is_some_and(|v| v != h)
            {
                return Err(Error::Conflict(
                    "registered experiment content differs".into(),
                ));
            }
            session.commit().await?;
            match (progress.control_digest, progress.holdout_digest) {
                (None, None) => Ok(ConsumerProgress::NotCommitted),
                // C is an input, while H and its checkpoint commit together.
                // Only the actual never-started or pre-H claimed phases prove
                // that this job has not committed H.
                (Some(_), None)
                    if (record.generation() == 0
                        && record.identity().lease_token.is_none()
                        && record.result().is_none())
                        || (record.step() == "claimed"
                            && matches!(
                                record.identity().state,
                                ManagementJobState::Running | ManagementJobState::Draining
                            )) =>
                {
                    Ok(ConsumerProgress::NotCommitted)
                }
                (Some(_), None) => Ok(ConsumerProgress::Unknown),
                (Some(control_digest), Some(holdout_digest)) => {
                    let owned_result = matches!(record.original_result(),
                        Some(ManagementResult::ExperimentRegistered {control_id,control_digest: saved_c,holdout_id,holdout_digest: saved_h})
                        if control_id == &request.control.id && saved_c == &control_digest
                            && holdout_id == &request.holdout.id && saved_h == &holdout_digest);
                    if record.identity().schema_version == evo_storage::MANAGEMENT_JOB_V2_SCHEMA
                        && record.identity().state == ManagementJobState::Cancelled
                        && !owned_result
                    {
                        return Ok(ConsumerProgress::NotCommitted);
                    }
                    if !owned_result
                        && !matches!(
                            record.step(),
                            "experiment_registered" | "experiment_already_registered"
                        )
                    {
                        return Ok(ConsumerProgress::Unknown);
                    }
                    Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                        state: ManagementJobState::Succeeded,
                        step: if record.identity().schema_version
                            == evo_storage::MANAGEMENT_JOB_V2_SCHEMA
                            && record.step() == "experiment_already_registered"
                        {
                            "experiment_already_registered"
                        } else {
                            "experiment_registered"
                        }
                        .into(),
                        result: Some(ManagementResultV2::Original(
                            ManagementResult::ExperimentRegistered {
                                control_id: request.control.id,
                                control_digest,
                                holdout_id: request.holdout.id,
                                holdout_digest,
                            },
                        )),
                        error_code: None,
                    })))
                }
                _ => Err(Error::Conflict(
                    "holdout exists without its registered control".into(),
                )),
            }
        }
        ParsedRequest::EvaluationStart(request) => {
            let stored_ticket_id = match record.original_result() {
                Some(ManagementResult::EvaluationTicketBlocked { ticket_id }) => {
                    Some(ticket_id.as_str())
                }
                _ => None,
            };
            let Some(current) =
                evaluation_start_existing_status(ctx, store, &request, stored_ticket_id).await?
            else {
                return Ok(ConsumerProgress::NotCommitted);
            };
            Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                state: ManagementJobState::Blocked,
                step: "ticket_issued_execution_blocked".into(),
                result: Some(ManagementResultV2::Original(
                    ManagementResult::EvaluationTicketBlocked {
                        ticket_id: current.ticket.id,
                    },
                )),
                error_code: Some("execution_provider_unconfigured".into()),
            })))
        }
        ParsedRequest::EvaluationStatus(request) => {
            // This is the real full read-only consumer. It does not claim the
            // historical ticket's own provider calls for this management job.
            match IndependentEvaluationControl::status(ctx, store, &request.ticket_id).await {
                Ok(_) | Err(Error::NotFound) => {}
                Err(error) => return Err(error),
            }
            // This concrete adapter only reads. A missing ticket cannot start
            // or own any of the historical ticket's provider calls.
            Ok(ConsumerProgress::NotCommitted)
        }
        ParsedRequest::ReplayRun(request) => {
            let policy = fingerprint(&request.policy)?;
            let profile = fingerprint(&request.profile)?;
            let caps = fingerprint(&request.caps)?;
            let ids = if let Some(ManagementResult::ReplayStored { report_id, .. }) =
                record.original_result()
            {
                vec![report_id.clone()]
            } else {
                [
                    evo_core::replay::ReplayReportIdentityVersion::LegacyV2,
                    evo_core::replay::ReplayReportIdentityVersion::NextV3,
                ]
                .into_iter()
                .map(|version| {
                    evo_core::replay::replay_report_id_for_version(
                        version,
                        ctx.namespace(),
                        request.partition,
                        &request.pool_digest,
                        &policy,
                        &profile,
                        &caps,
                    )
                })
                .collect::<Result<Vec<_>>>()?
            };
            for id in ids {
                let mut session = store.session().await?;
                let present = match evo_storage::replay::replay_report_version_in_session(
                    &mut session,
                    ctx,
                    &id,
                )
                .await
                {
                    Ok(Some(_)) => true,
                    Err(evo_core::replay::ReplayReadIsolation::Source(Error::NotFound)) => false,
                    Ok(None) => return Ok(ConsumerProgress::Unknown),
                    Err(error) => {
                        return Err(crate::replay::replay_read_isolation_for_existing_api(error));
                    }
                };
                session.commit().await?;
                if !present {
                    continue;
                }
                let read = crate::replay::query_replay_report_record(ctx, store, &id)
                    .await
                    .map_err(crate::replay::replay_read_isolation_for_existing_api)?;
                let (actual_id, pool, semantic, actual_policy, actual_profile, actual_caps) =
                    if let Some(old) = read.record.report() {
                        (
                            &old.report_id,
                            &old.pool_digest,
                            &old.semantic_reports_digest,
                            &old.policy_digest,
                            &old.profile_digest,
                            &old.caps_digest,
                        )
                    } else if let Some(current) = read.record.current_report() {
                        (
                            &current.report_id,
                            &current.pool_digest,
                            &current.semantic_reports_digest,
                            &current.policy_digest,
                            &current.profile_digest,
                            &current.caps_digest,
                        )
                    } else {
                        return Err(Error::Invalid("invalid replay audit record".into()));
                    };
                if actual_id != &id
                    || pool != &request.pool_digest
                    || actual_policy != &policy
                    || actual_profile != &profile
                    || actual_caps != &caps
                {
                    return Err(Error::Conflict("replay report view digest mismatch".into()));
                }
                return Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                    state: ManagementJobState::Succeeded,
                    step: "replay_stored".into(),
                    result: Some(ManagementResultV2::Original(
                        ManagementResult::ReplayStored {
                            report_id: id,
                            pool_digest: pool.clone(),
                            semantic_reports_digest: semantic.clone(),
                        },
                    )),
                    error_code: None,
                })));
            }
            // The bound private request and both exact producer report reads
            // prove no report commit. This adapter only recomputes locally and
            // writes that report in one final transaction; it has no provider
            // execution to drain. Its new-use pool/source gates remain in the
            // producer, not in this historical effect lookup.
            Ok(ConsumerProgress::NotCommitted)
        }
        ParsedRequest::CurriculumStep(request) => {
            let logical = crate::curriculum::probe_schedule_receipt_id(record.identity().id)?;
            let physical = format!(
                "e12-{}",
                fingerprint(&("probe_schedule_receipt_v1", &logical))?
            );
            let mut session = store.session().await?;
            let Some(row) = session.object_row(ctx, "artifact", &physical).await? else {
                session.commit().await?;
                return Ok(ConsumerProgress::NotCommitted);
            };
            if row.owner != ctx.actor() {
                return Err(Error::Forbidden);
            }
            let value = evo_storage::lifecycle::parse_strict_json(&row.body)?;
            let envelope: ManagementCurriculumEnvelope<crate::curriculum::ProbeScheduleReceiptV1> =
                serde_json::from_value(value).map_err(|_| {
                    Error::Conflict("probe schedule receipt differs from the request".into())
                })?;
            let receipt = envelope.payload;
            if envelope.schema_version != "rsia.curriculum_artifact_envelope.v1"
                || envelope.id != physical
                || envelope.record_kind != "probe_schedule_receipt_v1"
                || receipt.schema_version != crate::curriculum::ProbeScheduleReceiptV1::SCHEMA
                || receipt.id != logical
                || receipt.idempotency_key != record.identity().id
                || receipt.profile_id != request.profile_id
                || receipt.state_id != request.state_id
                || receipt.root_budget_limit_micros != request.root_budget_limit_micros
            {
                return Err(Error::Conflict(
                    "probe schedule receipt differs from the request".into(),
                ));
            }
            let profile_id = crate::curriculum::curriculum_profile_storage_id(&receipt.profile_id)?;
            let profile_row = session
                .object_row(ctx, "artifact", &profile_id)
                .await?
                .ok_or(Error::NotFound)?;
            let profile_value = evo_storage::lifecycle::parse_strict_json(&profile_row.body)?;
            let profile_envelope: ManagementCurriculumEnvelope<
                evo_core::curriculum::CurriculumControlProfileV1,
            > = serde_json::from_value(profile_value)
                .map_err(|_| Error::Conflict("curriculum artifact envelope mismatch".into()))?;
            if profile_envelope.schema_version != "rsia.curriculum_artifact_envelope.v1"
                || profile_envelope.id != profile_id
                || profile_envelope.record_kind != "curriculum_profile_v1"
            {
                return Err(Error::Conflict(
                    "curriculum artifact envelope mismatch".into(),
                ));
            }
            if profile_envelope.payload.profile_id != receipt.profile_id {
                return Err(Error::Conflict(
                    "curriculum artifact envelope mismatch".into(),
                ));
            }
            profile_envelope.payload.validate()?;
            let expected_edges: BTreeSet<_> = [
                (
                    "artifact".into(),
                    crate::curriculum::curriculum_probe_job_storage_id(&receipt.probe_job_id)?,
                ),
                (
                    "artifact".into(),
                    crate::curriculum::curriculum_state_storage_id(&receipt.state_id)?,
                ),
                ("artifact".into(), profile_id),
            ]
            .into_iter()
            .collect();
            if session
                .dependency_edges(ctx, "artifact", &physical)
                .await?
                .into_iter()
                .collect::<BTreeSet<_>>()
                != expected_edges
            {
                return Err(Error::Conflict(
                    "recorded probe job differs from its schedule receipt".into(),
                ));
            }
            session.commit().await?;
            let probe =
                crate::curriculum::verified_probe_job_view(ctx, store, &receipt.probe_job_id)
                    .await?;
            if probe.profile_id != receipt.profile_id
                || probe.state_id != receipt.state_id
                || probe.trigger_digest != receipt.trigger_digest
                || probe.root_budget_limit_micros != receipt.root_budget_limit_micros
                || probe.effective_monetary_limit_micros != 0
                || probe.provider_dispatch_count != 0
                || !probe.attempts.is_empty()
                || profile_envelope.payload.runtime_status
                    != evo_core::curriculum::CurriculumRuntimeStatus::Disabled
            {
                return Ok(ConsumerProgress::Unknown);
            }
            Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                state: ManagementJobState::Succeeded,
                step: "probe_scheduled".into(),
                result: Some(ManagementResultV2::Original(
                    ManagementResult::ProbeScheduled {
                        probe_job_id: probe.id,
                        trigger_digest: probe.trigger_digest,
                        terminal: probe.terminal,
                    },
                )),
                error_code: None,
            })))
        }
        ParsedRequest::ExplorationStart(request) => {
            let physical = crate::exploration::exploration_world_storage_id(&request.world.id)?;
            let mut session = store.session().await?;
            let present = session
                .object_row(ctx, "artifact", &physical)
                .await?
                .is_some();
            session.commit().await?;
            if !present {
                return Ok(ConsumerProgress::NotCommitted);
            }
            let registration =
                crate::exploration::verified_world_registration_view(ctx, store, &request.world.id)
                    .await?;
            crate::exploration::ensure_world_registered_as(ctx, store, &request.world).await?;
            let first = crate::exploration::first_decision(&request.world)?;
            if registration.context_signature != request.world.context_signature
                || registration.policy_digest != first.policy_digest
                || registration.caps_digest != first.caps_digest
            {
                return Err(Error::Conflict(
                    "exploration decision view differs from the stored result".into(),
                ));
            }
            Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                state: ManagementJobState::Succeeded,
                step: "exploration_started".into(),
                result: Some(ManagementResultV2::Original(
                    ManagementResult::ExplorationStarted {
                        world_id: request.world.id,
                        context_signature: request.world.context_signature,
                        prefix_digest: first.prefix_digest,
                        legal_actions_digest: first.legal_actions_digest,
                        policy_digest: first.policy_digest,
                        caps_digest: first.caps_digest,
                        action: first.action,
                    },
                )),
                error_code: None,
            })))
        }
        ParsedRequest::TraceExportPrepare(request) => {
            // Crash reconciliation: the record and the terminal job state were
            // written by one transaction, so this job's own record is the
            // proof its prepare committed.
            let job_id = record.identity().id;
            let mut session = store.session().await?;
            let record =
                crate::trace_export::load_live_record(ctx, &mut session, &request.export_id)
                    .await?;
            session.commit().await?;
            match record {
                Some(record) if record.prepare_job_id == job_id => {
                    Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                        state: ManagementJobState::Succeeded,
                        step: "trace_export_prepared".into(),
                        result: Some(ManagementResultV2::Trace(
                            ManagementTraceExportResultV2::TraceExportPrepared {
                                export_id: record.export_id,
                                record_id: record.id,
                                candidate_digest: record.candidate_digest,
                            },
                        )),
                        error_code: None,
                    })))
                }
                _ => Ok(ConsumerProgress::NotCommitted),
            }
        }
        ParsedRequest::TraceExportPublish(request) => {
            // Crash reconciliation anchored on the record: only this job's
            // reviewed record whose formal tree is exactly the frozen members
            // completes `delivered`; a missing or differing tree proves no
            // publication, and nothing is republished here.
            match crate::trace_export::reconcile_publication(
                ctx,
                store,
                &request.export_id,
                record.identity().id,
            )
            .await?
            {
                Some(delivered) => Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                    state: ManagementJobState::Succeeded,
                    step: "trace_export_delivered".into(),
                    result: Some(ManagementResultV2::Trace(
                        ManagementTraceExportResultV2::TraceExportDelivered {
                            receipt_digest: delivered
                                .delivery
                                .ok_or(Error::Internal)?
                                .receipt_digest,
                            export_id: delivered.export_id,
                            record_id: delivered.id,
                            candidate_digest: delivered.candidate_digest,
                        },
                    )),
                    error_code: None,
                }))),
                None => Ok(ConsumerProgress::NotCommitted),
            }
        }
        ParsedRequest::BudgetRootAuthorize(_)
        | ParsedRequest::BudgetStagePlanBind(_)
        | ParsedRequest::BudgetCallsListOpen(_)
        | ParsedRequest::BudgetCallReconcile(_) => {
            // These handlers write their business effect and the terminal job
            // state in one transaction. A stored terminal Budget result is
            // therefore the committed outcome; without one nothing was
            // committed by this job.
            let identity = record.identity();
            let result = record.result();
            if matches!(
                identity.state,
                ManagementJobState::Succeeded | ManagementJobState::Blocked
            ) && matches!(result, Some(ManagementResultV2::Budget(_)))
            {
                Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                    state: identity.state,
                    step: record.step().into(),
                    result,
                    error_code: record.error_code().map(str::to_owned),
                })))
            } else {
                Ok(ConsumerProgress::NotCommitted)
            }
        }
        // Unreachable: the import short-circuit above owns both operations.
        // The arms keep this match total without a wildcard.
        ParsedRequest::ImportRegister(_) | ParsedRequest::ImportExecute(_) => {
            Ok(ConsumerProgress::NotCommitted)
        }
        // Unreachable: the asset/export short-circuits above own these five.
        ParsedRequest::AssetStage(_)
        | ParsedRequest::AssetAbort(_)
        | ParsedRequest::AssetHandoff(_)
        | ParsedRequest::ExportPrepare(_)
        | ParsedRequest::ExportAbort(_) => Ok(ConsumerProgress::NotCommitted),
        ParsedRequest::ExportComplete(request) => {
            // A published tree whose SQL completion did not commit is an
            // exact-effect question, never an empty-failure guess: only the
            // full attempt/input/source/watermark/tree/bytes/dependency and
            // actual-directory agreement completes; a differing tree that
            // exists stays Unknown (Draining); an absent tree proves no
            // delivery.
            match crate::packages::PersistentPackageStore::inspect_export_completion_recovery(
                ctx,
                store,
                &request.export_id,
            )
            .await?
            {
                crate::packages::ExportCompletionInspection::Completed(_, completion) => Ok(
                    ConsumerProgress::Committed(Box::new(export_completed_outcome(&completion)?)),
                ),
                crate::packages::ExportCompletionInspection::Ready(_)
                | crate::packages::ExportCompletionInspection::PublishedUnverified => {
                    Ok(ConsumerProgress::Unknown)
                }
                crate::packages::ExportCompletionInspection::NotPublished => {
                    Ok(ConsumerProgress::NotCommitted)
                }
            }
        }
        ParsedRequest::OptimizationStep(_) | ParsedRequest::BackupCreate(_) => Err(Error::Internal),
        ParsedRequest::Blocked(request) => {
            if record.identity().operation != "meta.start"
                || request.schema_version != "rsia.management.meta_start.v1"
            {
                return Ok(ConsumerProgress::Unknown);
            }
            // This admitted concrete adapter has no business writer or
            // outbound work. Restore its exact terminal without executing it
            // again; result=None does not replace any accounting lookup.
            Ok(ConsumerProgress::Committed(Box::new(JobOutcome {
                state: ManagementJobState::Blocked,
                step: "blocked_feature".into(),
                result: None,
                error_code: Some(format!(
                    "{}_consumer_unavailable",
                    record.identity().operation
                )),
            })))
        }
    }
}

// A physical consumer may currently prove less than this authenticated job
// already committed. That is uncertainty, never permission to erase refs.
fn committed_result_is_preserved(
    stored: Option<&ManagementResultV2>,
    actual: Option<&ManagementResultV2>,
) -> Result<bool> {
    if !stored_result_has_side_effect(stored) {
        return Ok(true);
    }
    let (Some(stored), Some(actual)) = (stored, actual) else {
        return Ok(false);
    };
    if fingerprint(stored)? == fingerprint(actual)? {
        return Ok(true);
    }
    match (stored, actual) {
        (
            ManagementResultV2::Partial(ManagementPartialResultV2::ControlRegistered {
                control_id,
                control_digest,
            }),
            ManagementResultV2::Original(ManagementResult::ExperimentRegistered {
                control_id: actual_id,
                control_digest: actual_digest,
                ..
            }),
        ) => Ok(control_id == actual_id && control_digest == actual_digest),
        _ => Ok(false),
    }
}

fn progress_preserves_committed_result(
    stored: Option<&ManagementResultV2>,
    progress: &ConsumerProgress,
) -> Result<bool> {
    match progress {
        ConsumerProgress::Unknown => Ok(true),
        ConsumerProgress::NotCommitted => Ok(!stored_result_has_side_effect(stored)),
        ConsumerProgress::Committed(outcome) => {
            committed_result_is_preserved(stored, outcome.result.as_ref())
        }
    }
}

async fn consumer_progress_or_unknown(
    ctx: &Context,
    store: &Store,
    record: &VersionedManagementJob,
) -> Result<ConsumerProgress> {
    match consumer_progress(ctx, store, record).await {
        Ok(progress) => {
            if progress_preserves_committed_result(record.result().as_ref(), &progress)? {
                Ok(progress)
            } else {
                Ok(ConsumerProgress::Unknown)
            }
        }
        Err(error @ (Error::ClockRegressed | Error::Internal)) => Err(error),
        Err(
            Error::Invalid(_)
            | Error::Forbidden
            | Error::NotFound
            | Error::Conflict(_)
            | Error::Capacity(_)
            | Error::Budget
            | Error::Cancelled,
        ) => Ok(ConsumerProgress::Unknown),
    }
}

fn pending_terminal(state: ManagementJobState) -> Option<ManagementPendingTerminalState> {
    match state {
        ManagementJobState::Succeeded => Some(ManagementPendingTerminalState::Succeeded),
        ManagementJobState::Failed => Some(ManagementPendingTerminalState::Failed),
        ManagementJobState::Cancelled => Some(ManagementPendingTerminalState::Cancelled),
        ManagementJobState::Blocked => Some(ManagementPendingTerminalState::Blocked),
        _ => None,
    }
}

fn restored_terminal(state: ManagementPendingTerminalState) -> ManagementJobState {
    match state {
        ManagementPendingTerminalState::Succeeded => ManagementJobState::Succeeded,
        ManagementPendingTerminalState::Failed => ManagementJobState::Failed,
        ManagementPendingTerminalState::Cancelled => ManagementJobState::Cancelled,
        ManagementPendingTerminalState::Blocked => ManagementJobState::Blocked,
    }
}

fn apply_completed_outcome(current: &mut ManagementJobV2, outcome: &JobOutcome) -> Result<()> {
    if pending_terminal(outcome.state).is_none() {
        return Err(Error::Conflict(
            "management worker did not return a terminal outcome".into(),
        ));
    }
    if !committed_result_is_preserved(current.result.as_ref(), outcome.result.as_ref())? {
        current.state = ManagementJobState::Draining;
        return Ok(());
    }
    let partial_control = matches!(
        outcome.result.as_ref(),
        Some(ManagementResultV2::Partial(_))
    );
    let partial =
        partial_control || outcome.error_code.as_deref() == Some("blocked_partial_commit");
    let committed = outcome.has_committed_side_effect()
        || partial
        || (current.operation == "optimization.step"
            && current.cancel_arrival == Some(ManagementCancelArrival::AfterSideEffect));
    if partial {
        current.state = ManagementJobState::Blocked;
        current.step = if partial_control {
            "control_registered".into()
        } else {
            outcome.step.clone()
        };
        current.result = outcome.result.clone();
        current.error_code = Some("blocked_partial_commit".into());
    } else if current.cancel_requested && !committed {
        current.state = ManagementJobState::Cancelled;
        current.step = "cancelled".into();
        current.result = if current.operation == "optimization.step" {
            outcome.result.clone()
        } else {
            None
        };
        current.error_code = Some("cancelled".into());
    } else {
        current.state = outcome.state;
        current.step = outcome
            .step
            .strip_suffix(":cancel_after_side_effect")
            .unwrap_or(&outcome.step)
            .to_owned();
        current.result = outcome.result.clone();
        current.error_code = outcome.error_code.clone();
    }
    if current.cancel_requested {
        current.cancel_arrival = Some(
            if committed || current.cancel_arrival == Some(ManagementCancelArrival::AfterSideEffect)
            {
                ManagementCancelArrival::AfterSideEffect
            } else {
                ManagementCancelArrival::BeforeSideEffect
            },
        );
    }
    current.pending_terminal_state = None;
    current.lease_token = None;
    current.lease_until = 0;
    Ok(())
}

/// The original live-fence writer is still strict, including terminal/stale
/// early-return. Its sole production caller has actually awaited the worker.
async fn finish_claim(
    ctx: &Context,
    store: &Store,
    claimed: &ManagementJobV2,
    outcome: JobOutcome,
) -> Result<()> {
    let mut session = store.session().await?;
    let record = load_job_record_in_session(ctx, &mut session, &claimed.id).await?;
    if is_terminal(record.identity().state) {
        session.commit().await?;
        return Ok(());
    }
    let mut current = record.into_unfinished_v2()?;
    ensure_fence(&current, claimed)?;
    apply_completed_outcome(&mut current, &outcome)?;
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.job.finish", &current.id)
        .await?;
    session.commit().await
}

impl ManagementDispatcher {
    /// Invoked after the job's cancel commit or after an actual worker exit.
    /// The reader transaction is released before the original Store wrappers.
    async fn optimization_budget_actions(&self, ctx: &Context, job_id: &str) -> Result<()> {
        let mut session = self.store.session().await?;
        let record = load_job_record_in_session(ctx, &mut session, job_id).await?;
        let requested = record.cancel_requested();
        let facts = optimization_accounting_or_unknown(ctx, &mut session, &record).await?;
        session.commit().await?;
        let Some(facts) = facts else { return Ok(()) };
        for owned in facts.calls {
            let call = &owned.call;
            let cancel = match call.state {
                evo_storage::budget::BudgetCallState::Reserved => {
                    // A newer Reserved fence is not ours to release.
                    call.lease_token == owned.reference.lease_token
                        && call.lease_epoch == owned.reference.lease_epoch
                }
                evo_storage::budget::BudgetCallState::Dispatched
                | evo_storage::budget::BudgetCallState::Uncertain => requested,
                _ => false,
            };
            if !cancel {
                continue;
            }
            let fence = evo_storage::budget::BudgetCallFence {
                billing_scope: call.billing_scope.clone(),
                call_id: call.call_id.clone(),
                actual_input_digest: call.actual_input_digest.clone(),
                lease_token: call.lease_token.clone(),
                lease_epoch: call.lease_epoch,
                now: now()?,
            };
            // Only original budget semantics: proven Reserved release;
            // Dispatched -> Uncertain; no refund/re-dispatch/automatic charge.
            self.store
                .cancel_budget_call(ctx, &fence, "management_job_stopped")
                .await?;
        }
        Ok(())
    }
}

impl ManagementDispatcher {
    /// Own the `export.complete` crash reconciliation. `Some` means this path
    /// decided the job (recovered, already completed, or an unverified tree
    /// kept Draining); `None` means the formal tree is absent and the generic
    /// NotCommitted path answers honestly.
    async fn finish_export_complete(
        &self,
        ctx: &Context,
        claimed: &ManagementJobV2,
        _observed: &ObservedWorkerExit,
    ) -> Result<Option<ManagementJobState>> {
        let request = match load_private_request(ctx, &self.store, claimed).await {
            Ok(ParsedRequest::ExportComplete(request)) => request,
            Ok(_) => {
                return Err(Error::Conflict(
                    "export completion job input differs".into(),
                ));
            }
            // A revocation tombstone hides the export id but does not prove
            // that no tree was published. Keep this uncertainty Draining
            // instead of letting the generic path record Failed.
            Err(InputFault::Redacted { .. }) => {
                return self
                    .mark_export_complete_draining(ctx, claimed)
                    .await
                    .map(Some);
            }
            Err(InputFault::Other(error @ (Error::ClockRegressed | Error::Internal))) => {
                return Err(error);
            }
            // Absent/damaged input without a recoverable export id: the
            // generic NotCommitted path answers honestly.
            Err(InputFault::Other(_)) => return Ok(None),
        };
        let inspection =
            crate::packages::PersistentPackageStore::inspect_export_completion_recovery(
                ctx,
                &self.store,
                &request.export_id,
            )
            .await;
        let inspection = match inspection {
            Ok(inspection) => inspection,
            Err(error @ (Error::ClockRegressed | Error::Internal)) => return Err(error),
            // The stored attempt or its closure cannot be read as the exact
            // preimage. Without a proven absent tree this is uncertainty, not
            // a zero-effect failure.
            Err(_) => {
                return self
                    .mark_export_complete_draining(ctx, claimed)
                    .await
                    .map(Some);
            }
        };
        match inspection {
            crate::packages::ExportCompletionInspection::NotPublished => Ok(None),
            crate::packages::ExportCompletionInspection::PublishedUnverified => self
                .mark_export_complete_draining(ctx, claimed)
                .await
                .map(Some),
            crate::packages::ExportCompletionInspection::Ready(_)
            | crate::packages::ExportCompletionInspection::Completed(_, _) => self
                .recover_export_complete(ctx, claimed, &request.export_id, &inspection)
                .await
                .map(Some),
        }
    }

    async fn mark_export_complete_draining(
        &self,
        ctx: &Context,
        claimed: &ManagementJobV2,
    ) -> Result<ManagementJobState> {
        let mut session = self.store.session().await?;
        let record = load_job_record_in_session(ctx, &mut session, &claimed.id).await?;
        if is_terminal(record.identity().state) {
            let state = record.identity().state;
            session.commit().await?;
            return Ok(state);
        }
        let mut current = record.into_unfinished_v2()?;
        if !same_claim_fields(&current, claimed) {
            return Err(Error::Conflict("management job lease fence changed".into()));
        }
        if current.state != ManagementJobState::Draining {
            current.state = ManagementJobState::Draining;
            now()?;
            session
                .put(ctx, "job", &current.id, ctx.actor(), &current)
                .await?;
            session
                .audit(ctx, "management.job.draining", &current.id)
                .await?;
        }
        session.commit().await?;
        Ok(ManagementJobState::Draining)
    }

    async fn recover_export_complete(
        &self,
        ctx: &Context,
        claimed: &ManagementJobV2,
        export_id: &str,
        inspection: &crate::packages::ExportCompletionInspection,
    ) -> Result<ManagementJobState> {
        let mut session = self.store.registered_blob_write_session().await?;
        let record = load_job_record_in_session(ctx, &mut session, &claimed.id).await?;
        if is_terminal(record.identity().state) {
            let state = record.identity().state;
            session.commit().await?;
            return Ok(state);
        }
        let mut current = record.into_unfinished_v2()?;
        if !same_claim_fields(&current, claimed) {
            return Err(Error::Conflict("management job lease fence changed".into()));
        }
        // Only the SQL completion is written in this transaction; the published
        // tree is verified, never staged, renamed or rewritten.
        let completion =
            crate::packages::PersistentPackageStore::recover_export_completion_in_session(
                ctx,
                &self.store,
                &mut session,
                export_id,
                inspection,
            )
            .await?;
        let outcome = export_completed_outcome(&completion)?;
        apply_completed_outcome(&mut current, &outcome)?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.job.finish", &current.id)
            .await?;
        session.commit().await?;
        Ok(ManagementJobState::Succeeded)
    }
}

impl ManagementDispatcher {
    async fn finish_observed_worker(
        &self,
        ctx: &Context,
        claimed: &ManagementJobV2,
        observed: &ObservedWorkerExit,
    ) -> Result<ManagementJobState> {
        if claimed.operation == "backup.create" {
            return backup::finish_observed(ctx, &self.store, claimed, observed).await;
        }
        if claimed.operation == "optimization.step" {
            let current = load_job_record(ctx, &self.store, &claimed.id).await?;
            if is_terminal(current.identity().state) {
                return Ok(current.identity().state);
            }
            // This entry owns a real exit/startup witness. Budget work happens
            // outside the subsequent management writer transaction.
            self.optimization_budget_actions(ctx, &claimed.id).await?;
        }
        if claimed.operation == "export.complete"
            && !matches!(
                observed,
                ObservedWorkerExit::Completed(outcome)
                    if outcome.state != ManagementJobState::Failed
            )
            && let Some(state) = self.finish_export_complete(ctx, claimed, observed).await?
        {
            return Ok(state);
        }
        if let ObservedWorkerExit::Completed(outcome) = observed
            && outcome.state != ManagementJobState::Failed
            && claimed.operation != "optimization.step"
        {
            match finish_claim(ctx, &self.store, claimed, outcome.clone()).await {
                Ok(()) => {
                    return Ok(load_job_record(ctx, &self.store, &claimed.id)
                        .await?
                        .identity()
                        .state);
                }
                Err(Error::Conflict(reason)) if reason == "management job lease fence changed" => {}
                Err(error) => return Err(error),
            }
        }
        // One actual worker has exited, or startup owns the data directory.
        // Ordinary old workers never reach this expired-lease reconciliation.
        let _at = now()?;
        let record = load_job_record(ctx, &self.store, &claimed.id).await?;
        if is_terminal(record.identity().state) {
            return Ok(record.identity().state);
        }
        let progress = match observed {
            // An owned pre-consumer/never-started exit or an executed reader/
            // rolled-back producer proves no new business commit, not an empty query.
            ObservedWorkerExit::BeforeConsumerFailed(_)
            | ObservedWorkerExit::BeforeCommitFailed(_)
            | ObservedWorkerExit::NeverStarted => ConsumerProgress::NotCommitted,
            ObservedWorkerExit::Completed(outcome)
                if outcome.state != ManagementJobState::Failed =>
            {
                ConsumerProgress::NotCommitted
            }
            // Normal failures retain their outcome, but may still follow a C/H
            // commit or unresolved execution. Reload the actual progress.
            ObservedWorkerExit::Completed(_)
            | ObservedWorkerExit::Failed(_)
            | ObservedWorkerExit::Restarted => {
                consumer_progress_or_unknown(ctx, &self.store, &record).await?
            }
        };
        let mut session = self.store.session().await?;
        let actual = load_job_record_in_session(ctx, &mut session, &claimed.id).await?;
        if is_terminal(actual.identity().state) {
            let state = actual.identity().state;
            session.commit().await?;
            return Ok(state);
        }
        let mut current = actual.into_unfinished_v2()?;
        let before = fingerprint(&current)?;
        if !same_claim_fields(&current, claimed) {
            return Err(Error::Conflict("management job lease fence changed".into()));
        }
        let progress = if progress_preserves_committed_result(current.result.as_ref(), &progress)? {
            progress
        } else {
            ConsumerProgress::Unknown
        };
        if current.operation == "optimization.step" {
            let current_record = VersionedManagementJob::V2(current.clone());
            let accounting =
                optimization_accounting_or_unknown(ctx, &mut session, &current_record).await?;
            if current.cancel_requested
                && accounting
                    .as_ref()
                    .is_some_and(|facts| facts.has_business_side_effect())
            {
                current.cancel_arrival = Some(ManagementCancelArrival::AfterSideEffect);
            }
            if let Some(facts) = &accounting {
                let actual = optimization_accounting_progress(facts, &current_record)?;
                if let ConsumerProgress::Committed(outcome) = actual
                    && progress_preserves_committed_result(
                        current.result.as_ref(),
                        &ConsumerProgress::Committed(outcome.clone()),
                    )?
                    && (facts.terminal.is_some() || facts.historical_terminal)
                {
                    let original_token = current.lease_token.clone();
                    let original_until = current.lease_until;
                    apply_completed_outcome(&mut current, &outcome)?;
                    if facts.pending() {
                        current.pending_terminal_state = pending_terminal(current.state);
                        current.state = ManagementJobState::Draining;
                        current.lease_token = original_token;
                        current.lease_until = original_until;
                    }
                    if fingerprint(&current)? != before {
                        now()?;
                        session
                            .put(ctx, "job", &current.id, ctx.actor(), &current)
                            .await?;
                        session
                            .audit(
                                ctx,
                                if current.state == ManagementJobState::Draining {
                                    "management.job.draining"
                                } else {
                                    "management.job.finish"
                                },
                                &current.id,
                            )
                            .await?;
                    }
                    let state = current.state;
                    session.commit().await?;
                    return Ok(state);
                }
            }
            let pre_consumer_exit = matches!(
                observed,
                ObservedWorkerExit::BeforeConsumerFailed(_) | ObservedWorkerExit::NeverStarted
            ) && current.optimization.is_none();
            let unresolved =
                !pre_consumer_exit && accounting.as_ref().is_none_or(|facts| facts.pending());
            if current.cancel_requested
                && accounting
                    .as_ref()
                    .is_some_and(|facts| facts.has_business_side_effect())
            {
                current.cancel_arrival = Some(ManagementCancelArrival::AfterSideEffect);
            }
            if unresolved {
                let original_token = current.lease_token.clone();
                let original_until = current.lease_until;
                if let ObservedWorkerExit::Completed(outcome)
                | ObservedWorkerExit::BeforeConsumerFailed(outcome) = observed
                {
                    apply_completed_outcome(&mut current, outcome)?;
                    if let Some(target) = pending_terminal(current.state) {
                        current.pending_terminal_state = Some(target);
                    }
                    current.lease_token = original_token;
                    current.lease_until = original_until;
                }
                let failure = match observed {
                    ObservedWorkerExit::Failed(code) => Some(code),
                    ObservedWorkerExit::Completed(outcome)
                        if outcome.state == ManagementJobState::Failed =>
                    {
                        outcome.error_code.as_ref()
                    }
                    _ => None,
                };
                if let Some(code) = failure
                    && let Some(facts) = &accounting
                {
                    if !facts.compiled.is_empty() || !facts.historical_compiled.is_empty() {
                        current.pending_terminal_state =
                            Some(ManagementPendingTerminalState::Blocked);
                        current.step = "optimization_partial_commit".into();
                        current.error_code = Some("blocked_partial_commit".into());
                    } else if facts.terminal.is_none()
                        && !facts.historical_terminal
                        && !stored_result_has_side_effect(current.result.as_ref())
                    {
                        current.pending_terminal_state = Some(if current.cancel_requested {
                            ManagementPendingTerminalState::Cancelled
                        } else {
                            ManagementPendingTerminalState::Failed
                        });
                        current.step = if current.cancel_requested {
                            "cancelled".into()
                        } else {
                            "worker_exited_before_commit".into()
                        };
                        current.error_code = Some(if current.cancel_requested {
                            "cancelled".into()
                        } else {
                            code.clone()
                        });
                    }
                }
                current.state = ManagementJobState::Draining;
                if current.cancel_requested
                    && accounting
                        .as_ref()
                        .is_some_and(|facts| facts.has_business_side_effect())
                {
                    current.cancel_arrival = Some(ManagementCancelArrival::AfterSideEffect);
                }
                if fingerprint(&current)? != before {
                    now()?;
                    session
                        .put(ctx, "job", &current.id, ctx.actor(), &current)
                        .await?;
                    session
                        .audit(ctx, "management.job.draining", &current.id)
                        .await?;
                }
                session.commit().await?;
                return Ok(ManagementJobState::Draining);
            }
        }
        match observed {
            ObservedWorkerExit::Completed(outcome)
                if outcome.state != ManagementJobState::Failed =>
            {
                apply_completed_outcome(&mut current, outcome)?;
            }
            ObservedWorkerExit::BeforeConsumerFailed(outcome)
            | ObservedWorkerExit::BeforeCommitFailed(outcome) => {
                // These private outcomes came from an owned pre-consumer exit
                // or an actual reader/rolled-back producer, not request data.
                apply_completed_outcome(&mut current, outcome)?;
            }
            _ => match progress {
                ConsumerProgress::Unknown => {
                    current.state = ManagementJobState::Draining;
                    // Existing step/result/error/refs stay intact. No guessed
                    // terminal, no new dispatch and no empty-fee completion.
                    let failure = match observed {
                        ObservedWorkerExit::Completed(outcome) => outcome.error_code.as_ref(),
                        ObservedWorkerExit::Failed(code) => Some(code),
                        _ => None,
                    };
                    if let Some(code) = failure
                        && !current.diagnostics.last().is_some_and(|last| {
                            last.code == *code && last.observed_generation == current.generation
                        })
                    {
                        if current.diagnostics.len() >= 32 {
                            current.diagnostics.remove(0);
                        }
                        current.diagnostics.push(ManagementDiagnostic {
                            code: code.clone(),
                            observed_generation: current.generation,
                            recorded_at: _at,
                        });
                    }
                }
                ConsumerProgress::Committed(mut outcome) => {
                    if let Some(state) = current.pending_terminal_state {
                        outcome.state = restored_terminal(state);
                        outcome.step = current.step.clone();
                        outcome.result = current.result.clone();
                        outcome.error_code = current.error_code.clone();
                    }
                    apply_completed_outcome(&mut current, &outcome)?;
                }
                ConsumerProgress::NotCommitted
                    if current.step != "experiment_already_registered"
                        && stored_result_has_side_effect(current.result.as_ref()) =>
                {
                    // Re-read current is authoritative even if it acquired a
                    // committed reference after the earlier progress read.
                    current.state = ManagementJobState::Draining;
                }
                ConsumerProgress::NotCommitted if current.pending_terminal_state.is_some() => {
                    let outcome = JobOutcome {
                        state: restored_terminal(
                            current.pending_terminal_state.ok_or(Error::Internal)?,
                        ),
                        step: current.step.clone(),
                        result: current.result.clone(),
                        error_code: current.error_code.clone(),
                    };
                    apply_completed_outcome(&mut current, &outcome)?;
                }
                ConsumerProgress::NotCommitted => {
                    let outcome = if let ObservedWorkerExit::Completed(outcome) = observed {
                        outcome.clone()
                    } else {
                        let (step, code) = if current.cancel_requested {
                            ("cancelled", "cancelled".to_owned())
                        } else {
                            let code = match observed {
                                ObservedWorkerExit::Failed(code) => code.clone(),
                                _ => "management_worker_exited".into(),
                            };
                            let step = if current.operation == "replay.run"
                                && current.step == "before_replay_run"
                                && matches!(observed, ObservedWorkerExit::Failed(code) if code == "not_found")
                            {
                                // NotCommitted already proved both exact report
                                // identities absent for this offline producer.
                                "failed"
                            } else {
                                "worker_exited_before_commit"
                            };
                            (step, code)
                        };
                        JobOutcome {
                            state: if current.cancel_requested {
                                ManagementJobState::Cancelled
                            } else {
                                ManagementJobState::Failed
                            },
                            step: step.into(),
                            result: None,
                            error_code: Some(code),
                        }
                    };
                    apply_completed_outcome(&mut current, &outcome)?;
                }
            },
        }
        if fingerprint(&current)? == before {
            let state = current.state;
            session.commit().await?;
            return Ok(state);
        }
        now()?;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(
                ctx,
                if current.state == ManagementJobState::Draining {
                    "management.job.draining"
                } else {
                    "management.job.finish"
                },
                &current.id,
            )
            .await?;
        session.commit().await?;
        Ok(current.state)
    }
}

async fn cancel_job_atomic(
    ctx: &Context,
    store: &Store,
    job_id: &str,
) -> Result<VersionedManagementJob> {
    identifier(job_id)?;
    let mut session = store.session().await?;
    let record = load_job_record_in_session(ctx, &mut session, job_id).await?;
    if let VersionedManagementJob::V2(saved) = &record
        && is_terminal(saved.state)
        && !saved.cancel_requested
        && ((saved.operation == "backup.create"
            && matches!(saved.result, Some(ManagementResultV2::Backup(_))))
            || (matches!(
                saved.operation.as_str(),
                "import.register" | "import.execute"
            ) && matches!(saved.result, Some(ManagementResultV2::Import(_))))
            || (matches!(
                saved.operation.as_str(),
                "asset.stage"
                    | "asset.abort"
                    | "asset.handoff"
                    | "export.prepare"
                    | "export.complete"
                    | "export.abort"
            ) && matches!(saved.result, Some(ManagementResultV2::AssetExport(_)))))
    {
        let mut job = saved.clone();
        now()?;
        job.cancel_requested = true;
        job.cancel_arrival = Some(ManagementCancelArrival::AfterSideEffect);
        session.put(ctx, "job", &job.id, ctx.actor(), &job).await?;
        session.audit(ctx, "management.job.cancel", &job.id).await?;
        session.commit().await?;
        return Ok(VersionedManagementJob::V2(job));
    }
    if is_terminal(record.identity().state) || record.cancel_requested() {
        // Includes old terminal v1 and already-requested Running/Draining:
        // logical zero-write, no new audit/revision/arrival and no migration.
        session.commit().await?;
        return Ok(record);
    }
    let optimization_effect = if record.identity().operation == "optimization.step" {
        optimization_accounting_or_unknown(ctx, &mut session, &record)
            .await?
            .map(|facts| facts.has_business_side_effect())
    } else {
        None
    };
    let mut job = record.into_unfinished_v2()?;
    let _at = now()?;
    let never_claimed = job.state == ManagementJobState::Queued
        && job.generation == 0
        && job.lease_token.is_none()
        && job.result.is_none();
    job.cancel_requested = true;
    if never_claimed {
        job.state = ManagementJobState::Cancelled;
        job.step = "cancelled_before_claim".into();
        job.error_code = Some("cancelled".into());
        job.cancel_arrival = Some(ManagementCancelArrival::BeforeSideEffect);
        job.lease_until = 0;
    } else {
        // Cancellation prevents future checkpoints. Execution or fees may
        // still be in flight, so the actual worker exit/reconciler owns closure.
        job.state = if job.operation == "backup.create" {
            ManagementJobState::Running
        } else {
            ManagementJobState::Draining
        };
        if job.operation == "optimization.step" {
            if let Some(after) = optimization_effect {
                job.cancel_arrival = Some(
                    if after || job.cancel_arrival == Some(ManagementCancelArrival::AfterSideEffect)
                    {
                        ManagementCancelArrival::AfterSideEffect
                    } else {
                        ManagementCancelArrival::BeforeSideEffect
                    },
                );
            }
        } else if job.operation == "backup.create" {
            job.cancel_arrival = Some(if stored_result_has_side_effect(job.result.as_ref()) {
                ManagementCancelArrival::AfterSideEffect
            } else {
                ManagementCancelArrival::BeforeSideEffect
            });
        } else if job.step == "experiment_already_registered" {
            job.cancel_arrival = Some(ManagementCancelArrival::BeforeSideEffect);
        } else if stored_result_has_side_effect(job.result.as_ref()) {
            job.cancel_arrival = Some(ManagementCancelArrival::AfterSideEffect);
        }
    }
    session.put(ctx, "job", &job.id, ctx.actor(), &job).await?;
    session.audit(ctx, "management.job.cancel", &job.id).await?;
    session.commit().await?;
    Ok(VersionedManagementJob::V2(job))
}

async fn renew_claim(ctx: &Context, store: &Store, claimed: &ManagementJobV2) -> Result<()> {
    let mut session = store.session().await?;
    let record = load_job_record_in_session(ctx, &mut session, &claimed.id).await?;
    if is_terminal(record.identity().state) {
        session.commit().await?;
        return Ok(());
    }
    let at = now()?;
    let mut current = record.into_unfinished_v2()?;
    if current.state != ManagementJobState::Running
        || current.cancel_requested
        || !same_claim_fields(&current, claimed)
        || current.lease_until <= at
    {
        return Err(Error::Conflict("management job lease fence changed".into()));
    }
    current.lease_until = at
        .checked_add(60)
        .ok_or_else(|| Error::Invalid("management lease clock exceeds i64".into()))?;
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.job.renew", &current.id)
        .await?;
    session.commit().await
}

fn stored_result_has_side_effect(result: Option<&ManagementResultV2>) -> bool {
    match result {
        Some(ManagementResultV2::Partial(_)) => true,
        Some(ManagementResultV2::Optimization(_)) => true,
        Some(ManagementResultV2::Trace(_)) => true,
        Some(ManagementResultV2::Backup(_)) => true,
        Some(ManagementResultV2::Budget(ManagementBudgetResultV2::OpenCallsListed { .. })) => false,
        Some(ManagementResultV2::Budget(_)) => true,
        Some(ManagementResultV2::Import(_)) => true,
        Some(ManagementResultV2::AssetExport(_)) => true,
        Some(ManagementResultV2::Original(
            ManagementResult::EvaluationStatus { .. }
            | ManagementResult::EvaluationStatusReadV2 { .. },
        ))
        | None => false,
        Some(ManagementResultV2::Original(_)) => true,
    }
}

fn ensure_fence(current: &ManagementJobV2, claimed: &ManagementJobV2) -> Result<()> {
    if current.state != ManagementJobState::Running
        || !same_claim_fields(current, claimed)
        || current.lease_until <= now()?
    {
        return Err(Error::Conflict("management job lease fence changed".into()));
    }
    Ok(())
}

fn is_terminal(state: ManagementJobState) -> bool {
    matches!(
        state,
        ManagementJobState::Succeeded
            | ManagementJobState::Failed
            | ManagementJobState::Cancelled
            | ManagementJobState::Blocked
    )
}

fn ensure_owner(ctx: &Context, job: &ManagementJob) -> Result<()> {
    if job.schema_version != evo_storage::MANAGEMENT_JOB_V1_SCHEMA
        || job.owner_actor != ctx.actor()
        || job.owner_role != ctx.role()
    {
        return Err(Error::NotFound);
    }
    Ok(())
}

fn parse_request(operation: &str, payload: Value) -> Result<ParsedRequest> {
    let invalid = || Error::Invalid("invalid management payload".into());
    let parsed = match operation {
        "experiment.register" => match payload["schema_version"].as_str() {
            Some("rsia.management.experiment_register.v1") => ParsedRequest::ExperimentRegister(
                Box::new(serde_json::from_value(payload).map_err(|_| invalid())?),
            ),
            Some("rsia.management.experiment_register.v2") => {
                ParsedRequest::ExperimentRegisterCurrent(Box::new(
                    serde_json::from_value(payload).map_err(|_| invalid())?,
                ))
            }
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "evaluation.start" => {
            ParsedRequest::EvaluationStart(serde_json::from_value(payload).map_err(|_| invalid())?)
        }
        "evaluation.status" => {
            ParsedRequest::EvaluationStatus(serde_json::from_value(payload).map_err(|_| invalid())?)
        }
        "replay.run" => ParsedRequest::ReplayRun(Box::new(
            serde_json::from_value(payload).map_err(|_| invalid())?,
        )),
        "curriculum.step" => {
            ParsedRequest::CurriculumStep(serde_json::from_value(payload).map_err(|_| invalid())?)
        }
        "exploration.start" => ParsedRequest::ExplorationStart(Box::new(
            serde_json::from_value(payload).map_err(|_| invalid())?,
        )),
        "optimization.step" => ParsedRequest::OptimizationStep(Box::new(
            serde_json::from_value(payload).map_err(|_| invalid())?,
        )),
        "trace.export" => match payload["schema_version"].as_str() {
            Some("rsia.management.trace_export.prepare.v1") => ParsedRequest::TraceExportPrepare(
                Box::new(serde_json::from_value(payload).map_err(|_| invalid())?),
            ),
            Some("rsia.management.trace_export.publish.v1") => ParsedRequest::TraceExportPublish(
                Box::new(serde_json::from_value(payload).map_err(|_| invalid())?),
            ),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "backup.create" => {
            ParsedRequest::BackupCreate(serde_json::from_value(payload).map_err(|_| invalid())?)
        }
        "budget.root.authorize" => match payload["schema_version"].as_str() {
            Some(BUDGET_ROOT_AUTHORIZE_SCHEMA) => ParsedRequest::BudgetRootAuthorize(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "budget.stage_plan.bind" => match payload["schema_version"].as_str() {
            Some(BUDGET_STAGE_PLAN_BIND_SCHEMA) => ParsedRequest::BudgetStagePlanBind(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "budget.calls.list_open" => match payload["schema_version"].as_str() {
            Some(BUDGET_CALLS_LIST_OPEN_SCHEMA) => ParsedRequest::BudgetCallsListOpen(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "budget.call.reconcile" => match payload["schema_version"].as_str() {
            Some(BUDGET_CALL_RECONCILE_SCHEMA) => ParsedRequest::BudgetCallReconcile(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "import.register" => match payload["schema_version"].as_str() {
            Some(IMPORT_REGISTER_SCHEMA) => ParsedRequest::ImportRegister(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "import.execute" => match payload["schema_version"].as_str() {
            Some(IMPORT_EXECUTE_SCHEMA) => ParsedRequest::ImportExecute(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "asset.stage" => match payload["schema_version"].as_str() {
            Some(ASSET_STAGE_SCHEMA) => ParsedRequest::AssetStage(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "asset.abort" => match payload["schema_version"].as_str() {
            Some(ASSET_ABORT_SCHEMA) => ParsedRequest::AssetAbort(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "asset.handoff" => match payload["schema_version"].as_str() {
            Some(ASSET_HANDOFF_SCHEMA) => ParsedRequest::AssetHandoff(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "export.prepare" => match payload["schema_version"].as_str() {
            Some(EXPORT_PREPARE_SCHEMA) => ParsedRequest::ExportPrepare(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "export.complete" => match payload["schema_version"].as_str() {
            Some(EXPORT_COMPLETE_SCHEMA) => ParsedRequest::ExportComplete(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "export.abort" => match payload["schema_version"].as_str() {
            Some(EXPORT_ABORT_SCHEMA) => ParsedRequest::ExportAbort(Box::new(
                serde_json::from_value(payload).map_err(|_| invalid())?,
            )),
            _ => {
                return Err(Error::Invalid(
                    "unsupported management payload schema".into(),
                ));
            }
        },
        "meta.start" => {
            ParsedRequest::Blocked(serde_json::from_value(payload).map_err(|_| invalid())?)
        }
        _ => return Err(Error::Invalid("unknown management operation".into())),
    };
    let expected = match &parsed {
        ParsedRequest::ExperimentRegisterCurrent(_) => {
            "rsia.management.experiment_register.v2".into()
        }
        ParsedRequest::TraceExportPrepare(_) => "rsia.management.trace_export.prepare.v1".into(),
        ParsedRequest::TraceExportPublish(_) => "rsia.management.trace_export.publish.v1".into(),
        _ => format!("rsia.management.{}.v1", operation.replace('.', "_")),
    };
    let actual = match &parsed {
        ParsedRequest::ExperimentRegister(request) => &request.schema_version,
        ParsedRequest::ExperimentRegisterCurrent(request) => &request.schema_version,
        ParsedRequest::EvaluationStart(request) => &request.schema_version,
        ParsedRequest::EvaluationStatus(request) => &request.schema_version,
        ParsedRequest::ReplayRun(request) => &request.schema_version,
        ParsedRequest::CurriculumStep(request) => &request.schema_version,
        ParsedRequest::ExplorationStart(request) => &request.schema_version,
        ParsedRequest::Blocked(request) => &request.schema_version,
        ParsedRequest::OptimizationStep(request) => &request.schema_version,
        ParsedRequest::TraceExportPrepare(request) => &request.schema_version,
        ParsedRequest::TraceExportPublish(request) => &request.schema_version,
        ParsedRequest::BackupCreate(request) => &request.schema_version,
        ParsedRequest::BudgetRootAuthorize(request) => &request.schema_version,
        ParsedRequest::BudgetStagePlanBind(request) => &request.schema_version,
        ParsedRequest::BudgetCallsListOpen(request) => &request.schema_version,
        ParsedRequest::BudgetCallReconcile(request) => &request.schema_version,
        ParsedRequest::ImportRegister(request) => &request.schema_version,
        ParsedRequest::ImportExecute(request) => &request.schema_version,
        ParsedRequest::AssetStage(request) => &request.schema_version,
        ParsedRequest::AssetAbort(request) => &request.schema_version,
        ParsedRequest::AssetHandoff(request) => &request.schema_version,
        ParsedRequest::ExportPrepare(request) => &request.schema_version,
        ParsedRequest::ExportComplete(request) => &request.schema_version,
        ParsedRequest::ExportAbort(request) => &request.schema_version,
    };
    if actual != &expected {
        return Err(Error::Invalid(
            "unsupported management payload schema".into(),
        ));
    }
    Ok(parsed)
}

fn stable_id(prefix: &str, ctx: &Context, operation: &str, request_key: &str) -> String {
    let digest = hash(
        format!(
            "{}\0{}\0{}\0{}",
            ctx.namespace(),
            ctx.actor(),
            operation,
            request_key
        )
        .as_bytes(),
    );
    format!("{prefix}-{}", &digest[..24])
}

fn private_dependencies(request: &ParsedRequest) -> Result<Vec<(String, String)>> {
    let mut dependencies = Vec::new();
    match request {
        ParsedRequest::ExperimentRegister(request) => {
            dependencies.push((
                "artifact".into(),
                e05_storage_id("registered_evaluation_control_v41", &request.control.id)?,
            ));
            dependencies.push((
                "artifact".into(),
                e05_storage_id("protected_holdout_v41", &request.holdout.id)?,
            ));
            for input in request
                .holdout
                .rotating_inputs
                .iter()
                .chain(&request.holdout.anchor_inputs)
            {
                dependencies.push(("artifact".into(), input.input_artifact_id.clone()));
            }
        }
        ParsedRequest::ExperimentRegisterCurrent(request) => {
            dependencies.push((
                "artifact".into(),
                e05_storage_id("registered_evaluation_control_v41", &request.control.id)?,
            ));
            dependencies.push((
                "artifact".into(),
                e05_storage_id("protected_holdout_v41", &request.holdout.id)?,
            ));
            for input in request
                .holdout
                .rotating_inputs
                .iter()
                .chain(&request.holdout.anchor_inputs)
            {
                dependencies.push(("artifact".into(), input.input_artifact_id.clone()));
            }
        }
        ParsedRequest::EvaluationStart(request) => {
            dependencies.push((
                "artifact".into(),
                e05_storage_id(
                    "registered_evaluation_control_v41",
                    &request.ticket.registration_id,
                )?,
            ));
            dependencies.push((
                "artifact".into(),
                e05_storage_id("protected_holdout_v41", &request.ticket.holdout_id)?,
            ));
        }
        ParsedRequest::EvaluationStatus(request) => dependencies.push((
            "artifact".into(),
            e05_storage_id("evaluation_ticket_v2", &request.ticket_id)?,
        )),
        ParsedRequest::ReplayRun(request) => {
            dependencies.push((
                "artifact".into(),
                evo_storage::replay::replay_pool_storage_id(&request.pool_digest)?,
            ));
        }
        ParsedRequest::CurriculumStep(request) => {
            dependencies.push((
                "artifact".into(),
                crate::curriculum::curriculum_profile_storage_id(&request.profile_id)?,
            ));
            dependencies.push((
                "artifact".into(),
                crate::curriculum::curriculum_state_storage_id(&request.state_id)?,
            ));
        }
        ParsedRequest::ExplorationStart(request) => {
            // The private input depends on every trusted run of the world's
            // source closure and on the world envelope it will register, so a
            // revoked run or a rewritten world is visible from the job.
            for dependency in &request.world.dependencies {
                if dependency.kind != "run" {
                    return Err(Error::Invalid(
                        "exploration source closure must reference trusted runs".into(),
                    ));
                }
                identifier(&dependency.id)?;
                dependencies.push(("run".into(), dependency.id.clone()));
            }
            dependencies.push((
                "artifact".into(),
                crate::exploration::exploration_world_storage_id(&request.world.id)?,
            ));
        }
        ParsedRequest::OptimizationStep(request) => {
            for id in [
                &request.source_selection_ref,
                &request.edit_template_ref,
                &request.improver_patch_ref,
            ] {
                identifier(id)?;
                dependencies.push(("artifact".into(), id.clone()));
            }
            dependencies.push((
                "artifact".into(),
                crate::development::storage_id(
                    crate::development::CONTROL_KIND,
                    &request.development_control_id,
                )?,
            ));
            dependencies.push((
                "artifact".into(),
                format!("formal-profile-{}", request.compiler.profile.id),
            ));
        }
        ParsedRequest::Blocked(_)
        | ParsedRequest::BackupCreate(_)
        | ParsedRequest::BudgetRootAuthorize(_)
        | ParsedRequest::BudgetStagePlanBind(_)
        | ParsedRequest::BudgetCallsListOpen(_)
        | ParsedRequest::BudgetCallReconcile(_) => {}
        ParsedRequest::ImportRegister(request) => {
            // Every registered blob is a physical input of the future
            // selection; its digest is content-addressed and carries no
            // caller-selected path.
            for source in &request.sources {
                dependencies.push(("blob".into(), source.blob_digest.clone()));
            }
        }
        ParsedRequest::ImportExecute(request) => {
            // A revoked selection refuses the execute request before its
            // first write (and before any blob read).
            dependencies.push(("artifact".into(), request.selection_id.clone()));
        }
        ParsedRequest::AssetStage(request) => {
            // The transient input blob is a physical input of the future
            // staged asset; its digest is content-addressed, not a path.
            dependencies.push(("blob".into(), request.input_blob_digest.clone()));
        }
        // Abort is closure: it must remain possible after a source revocation
        // and never opens a new use of the cancelled material.
        ParsedRequest::AssetAbort(_) | ParsedRequest::ExportAbort(_) => {}
        ParsedRequest::AssetHandoff(request) => {
            dependencies.push(("artifact".into(), request.staged_asset_id.clone()));
            dependencies.push(("artifact".into(), request.candidate_id.clone()));
        }
        ParsedRequest::ExportPrepare(request) => {
            dependencies.push(("artifact".into(), request.staged_asset_id.clone()));
        }
        ParsedRequest::ExportComplete(request) => {
            dependencies.push(("artifact".into(), request.export_id.clone()));
        }
        ParsedRequest::TraceExportPrepare(request) => {
            // The private input depends on every captured source run, so a
            // revoked source redacts the request with the job that holds it.
            for id in &request.source_ids {
                dependencies.push(("run".into(), id.clone()));
            }
        }
        ParsedRequest::TraceExportPublish(request) => {
            dependencies.push((
                "artifact".into(),
                crate::trace_export::trace_export_record_id(&request.export_id)?,
            ));
        }
    }
    dependencies.sort();
    dependencies.dedup();
    Ok(dependencies)
}

fn e05_storage_id(record_kind: &str, id: &str) -> Result<String> {
    identifier(record_kind)?;
    identifier(id)?;
    Ok(format!("e05-{}", fingerprint(&(record_kind, id))?))
}

fn error_code(error: &Error) -> &'static str {
    match error {
        Error::Invalid(_) => "invalid_input",
        Error::Forbidden => "forbidden",
        Error::NotFound => "not_found",
        Error::Conflict(_) | Error::Capacity(_) => "conflict",
        Error::Budget => "budget_unavailable",
        Error::Cancelled => "cancelled",
        Error::ClockRegressed => "clock_regressed",
        Error::Internal => "internal_error",
    }
}

/// Admin-only internal bridge; it does not expose private inputs to Worker or
/// Evaluator, accept a caller-selected job, create a Context, or open a Session.
pub(crate) async fn management_replay_registration_images_in_session(
    session: &mut evo_storage::Session,
    ctx: &Context,
    exploration_world_id: &str,
) -> Result<(
    evo_storage::replay::ValidatedReplayRootSource,
    Vec<ExplorationWorldV1>,
)> {
    ctx.require(&[Role::Admin])?;
    // Strict physical JSON admission happens before ANY legacy Value/typed read.
    // Storage also proves every incoming job that this bridge visits. No write
    // or second Session separates the raw gate from the old ParsedRequest guard.
    let material = evo_storage::replay::capture_replay_root_source_in_session(
        session,
        ctx,
        exploration_world_id,
    )
    .await?;
    let world_id = crate::exploration::exploration_world_storage_id(exploration_world_id)?;
    let mut inputs = session.dependents(ctx, "artifact", &world_id).await?;
    inputs.sort();
    inputs.dedup();
    let mut originals = Vec::new();
    for (kind, input_id) in inputs {
        if kind != "artifact" || !input_id.starts_with("management-input-") {
            continue;
        }
        let body: Value = session.need(ctx, "artifact", &input_id).await?;
        if is_redacted(&body) {
            return Err(redacted_input_error(&input_id));
        }
        let mut owners = session.dependents(ctx, "artifact", &input_id).await?;
        owners.sort();
        owners.dedup();
        let mut matched = false;
        for (job_kind, job_id) in owners {
            if job_kind != "job" {
                continue;
            }
            let row = session
                .object_row(ctx, "job", &job_id)
                .await?
                .ok_or(Error::NotFound)?;
            let record = evo_storage::decode_management_job(&row.body)?;
            let job = record.identity();
            if row.owner != job.owner_actor
                || job.id != job_id
                || job.private_input_ref != input_id
                || job.operation != "exploration.start"
                || job.id != stable_id("management-job", ctx, job.operation, job.request_key)
                || input_id != stable_id("management-input", ctx, job.operation, job.request_key)
            {
                return Err(Error::Conflict("replay_root_source_unproven".into()));
            }
            if job.owner_actor != ctx.actor() || job.owner_role != ctx.role() {
                return Err(Error::NotFound);
            }
            let ParsedRequest::ExplorationStart(request) = decode_private_request_fields(
                body.clone(),
                job.private_input_ref,
                job.operation,
                job.payload_digest,
            )?
            else {
                return Err(Error::Conflict("replay_root_source_unproven".into()));
            };
            if request.world.id != exploration_world_id || request.request_key != job.request_key {
                return Err(Error::Conflict("replay_root_source_unproven".into()));
            }
            originals.push(request.world);
            matched = true;
        }
        if !matched {
            return Err(Error::Conflict("replay_root_source_unproven".into()));
        }
    }
    Ok((material, originals))
}

#[derive(Debug, Clone, Serialize)]
pub struct ManagementJobReadView {
    #[serde(flatten)]
    pub job: VersionedManagementJob,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_read_view: Option<crate::replay::ReplayReportAuditView>,
}
impl ManagementDispatcher {
    /// Adds a non-persistent read projection only for replay.run. Original job,
    /// result, cancellation, owner and private-input revocation checks remain.
    pub async fn status_with_replay_read_view(
        &self,
        ctx: &Context,
        job_id: &str,
    ) -> Result<ManagementJobReadView> {
        let job = self.status(ctx, job_id).await?;
        if job.identity().operation != "replay.run" {
            return Ok(ManagementJobReadView {
                job,
                replay_read_view: None,
            });
        }
        let mut session = self.store.session().await?;
        let current = load_job_record_in_session(ctx, &mut session, job_id).await?;
        if fingerprint(&current)? != fingerprint(&job)? {
            return Err(Error::Conflict(
                "replay job changed during read projection".into(),
            ));
        }
        if current.identity().state == ManagementJobState::Succeeded
            && let Some(row) = session
                .object_row(ctx, "artifact", current.identity().private_input_ref)
                .await?
        {
            let body = evo_storage::lifecycle::parse_strict_json(&row.body)
                .map_err(|_| Error::Invalid("management_private_input_invalid_json".into()))?;
            if is_redacted(&body) {
                return Err(redacted_input_error(current.identity().private_input_ref));
            }
        }
        let replay_read_view = match current.original_result() {
            Some(ManagementResult::ReplayStored {
                report_id,
                pool_digest,
                semantic_reports_digest,
            }) => {
                let read = crate::replay::query_replay_report_record_in_session(
                    &mut session,
                    ctx,
                    report_id,
                )
                .await
                .map_err(crate::replay::replay_read_isolation_for_existing_api)?;
                let binding = if let Some(old) = read.record.report() {
                    (
                        &old.report_id,
                        &old.pool_digest,
                        &old.semantic_reports_digest,
                    )
                } else if let Some(current) = read.record.current_report() {
                    (
                        &current.report_id,
                        &current.pool_digest,
                        &current.semantic_reports_digest,
                    )
                } else {
                    return Err(Error::Invalid("invalid replay audit record".into()));
                };
                if binding != (report_id, pool_digest, semantic_reports_digest) {
                    return Err(Error::Conflict(
                        "replay read projection binding mismatch".into(),
                    ));
                }
                Some(read)
            }
            _ => None,
        };
        session.commit().await?;
        Ok(ManagementJobReadView {
            job: current,
            replay_read_view,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "calls")]
pub enum ManagementFeeKnowledge {
    KnownNone,
    TrustedCalls(Vec<ManagementFeeCallView>),
    UnknownProvenance,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManagementFeeCallView {
    pub billing_scope: String,
    pub call_id: String,
    pub state: evo_storage::budget::BudgetCallState,
    pub actual_cost_micros: Option<i64>,
    pub reserved_micros: i64,
    pub actual_currency: Option<String>,
    pub actual_pricing_version: Option<String>,
    pub execution_closed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ManagementCommittedEffect {
    Backup {
        relative_name: evo_storage::lifecycle::BackupName,
        manifest_sha256: String,
        durability: evo_storage::lifecycle::BackupDurability,
    },
    Control {
        control_id: String,
        control_digest: String,
    },
    OptimizationStep {
        stage_fact_id: String,
        input_digest: String,
    },
    OptimizationCompiled {
        stage_fact_id: String,
        input_digest: String,
    },
    Holdout {
        holdout_id: String,
        holdout_digest: String,
    },
    Ticket {
        ticket_id: String,
    },
    ReplayReport {
        report_id: String,
        pool_digest: String,
        semantic_reports_digest: String,
    },
    Probe {
        probe_job_id: String,
        trigger_digest: String,
    },
    World {
        world_id: String,
        context_signature: String,
        prefix_digest: String,
        legal_actions_digest: String,
        policy_digest: String,
        caps_digest: String,
    },
    TraceExport {
        export_id: String,
        record_id: String,
        candidate_digest: String,
        phase: String,
    },
    BudgetRoot {
        billing_scope: String,
        root_budget_id: String,
    },
    BudgetBinding {
        billing_scope: String,
        binding_seq: i64,
        plan_digest: String,
    },
    BudgetReconciliation {
        billing_scope: String,
        call_id: String,
        accounting_overflow: bool,
    },
    ImportSelection {
        selection_id: String,
        selection_digest: String,
    },
    ImportResult {
        selection_id: String,
        result_id: String,
        result_digest: String,
    },
    AssetStaged {
        staged_asset_id: String,
        manifest_digest: String,
        content_blob_digest: String,
    },
    AssetAborted {
        staged_asset_id: String,
    },
    AssetHandedOff {
        staged_asset_id: String,
        candidate_id: String,
    },
    ExportPrepared {
        export_id: String,
        package_tree_digest: String,
    },
    ExportCompleted {
        export_id: String,
        receipt_digest: String,
        delivery_audit_id: String,
    },
    ExportAborted {
        export_id: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ManagementJobStatusV1 {
    pub schema_version: &'static str,
    pub job_schema_version: String,
    pub id: String,
    pub operation: String,
    pub request_key: String,
    pub payload_digest: String,
    pub owner_actor: String,
    pub owner_role: Role,
    pub state: ManagementJobState,
    pub step: String,
    pub private_input_ref: String,
    pub result: Option<ManagementResultV2>,
    pub error_code: Option<String>,
    pub cancel_requested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_arrival: Option<ManagementCancelArrival>,
    pub lease_until: i64,
    pub generation: u64,
    pub diagnostics: Vec<ManagementDiagnostic>,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_terminal_state: Option<ManagementPendingTerminalState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimization_stage_refs: Option<Vec<ManagementOptimizationStageRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<backup::BackupStatus>,
    // None means unproven, not a proved empty list/zero amount.
    pub committed_side_effects: Option<Vec<ManagementCommittedEffect>>,
    pub fee_knowledge: ManagementFeeKnowledge,
    pub pending_fees: Option<Vec<ManagementFeeCallView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_read_view: Option<crate::replay::ReplayReportAuditView>,
}

fn committed_effects(result: Option<&ManagementResultV2>) -> Vec<ManagementCommittedEffect> {
    match result {
        Some(ManagementResultV2::Backup(ManagementBackupResultV2::BackupCreated {
            relative_name,
            manifest_sha256,
            durability,
        })) => vec![ManagementCommittedEffect::Backup {
            relative_name: relative_name.clone(),
            manifest_sha256: manifest_sha256.clone(),
            durability: *durability,
        }],
        Some(ManagementResultV2::Partial(ManagementPartialResultV2::ControlRegistered {
            control_id,
            control_digest,
        })) => vec![ManagementCommittedEffect::Control {
            control_id: control_id.clone(),
            control_digest: control_digest.clone(),
        }],
        Some(ManagementResultV2::Original(ManagementResult::ExperimentRegistered {
            control_id,
            control_digest,
            holdout_id,
            holdout_digest,
        })) => vec![
            ManagementCommittedEffect::Control {
                control_id: control_id.clone(),
                control_digest: control_digest.clone(),
            },
            ManagementCommittedEffect::Holdout {
                holdout_id: holdout_id.clone(),
                holdout_digest: holdout_digest.clone(),
            },
        ],
        Some(ManagementResultV2::Original(ManagementResult::EvaluationTicketBlocked {
            ticket_id,
        })) => vec![ManagementCommittedEffect::Ticket {
            ticket_id: ticket_id.clone(),
        }],
        Some(ManagementResultV2::Original(ManagementResult::ReplayStored {
            report_id,
            pool_digest,
            semantic_reports_digest,
        })) => vec![ManagementCommittedEffect::ReplayReport {
            report_id: report_id.clone(),
            pool_digest: pool_digest.clone(),
            semantic_reports_digest: semantic_reports_digest.clone(),
        }],
        Some(ManagementResultV2::Original(ManagementResult::ProbeScheduled {
            probe_job_id,
            trigger_digest,
            ..
        })) => vec![ManagementCommittedEffect::Probe {
            probe_job_id: probe_job_id.clone(),
            trigger_digest: trigger_digest.clone(),
        }],
        Some(ManagementResultV2::Original(ManagementResult::ExplorationStarted {
            world_id,
            context_signature,
            prefix_digest,
            legal_actions_digest,
            policy_digest,
            caps_digest,
            ..
        })) => vec![ManagementCommittedEffect::World {
            world_id: world_id.clone(),
            context_signature: context_signature.clone(),
            prefix_digest: prefix_digest.clone(),
            legal_actions_digest: legal_actions_digest.clone(),
            policy_digest: policy_digest.clone(),
            caps_digest: caps_digest.clone(),
        }],
        Some(ManagementResultV2::Optimization(
            ManagementOptimizationResultV2::OptimizationStepCompleted {
                stage_fact_id,
                input_digest,
            },
        )) => vec![ManagementCommittedEffect::OptimizationStep {
            stage_fact_id: stage_fact_id.clone(),
            input_digest: input_digest.clone(),
        }],
        Some(ManagementResultV2::Trace(ManagementTraceExportResultV2::TraceExportPrepared {
            export_id,
            record_id,
            candidate_digest,
        })) => vec![ManagementCommittedEffect::TraceExport {
            export_id: export_id.clone(),
            record_id: record_id.clone(),
            candidate_digest: candidate_digest.clone(),
            phase: "prepared".into(),
        }],
        Some(ManagementResultV2::Trace(ManagementTraceExportResultV2::TraceExportDelivered {
            export_id,
            record_id,
            candidate_digest,
            ..
        })) => vec![ManagementCommittedEffect::TraceExport {
            export_id: export_id.clone(),
            record_id: record_id.clone(),
            candidate_digest: candidate_digest.clone(),
            phase: "delivered".into(),
        }],
        Some(ManagementResultV2::Budget(ManagementBudgetResultV2::RootAuthorized { root })) => {
            vec![ManagementCommittedEffect::BudgetRoot {
                billing_scope: root.billing_scope.clone(),
                root_budget_id: root.root_budget_id.clone(),
            }]
        }
        Some(ManagementResultV2::Budget(ManagementBudgetResultV2::StagePlanBound { binding })) => {
            vec![ManagementCommittedEffect::BudgetBinding {
                billing_scope: binding.billing_scope.clone(),
                binding_seq: binding.seq,
                plan_digest: binding.plan_digest.clone(),
            }]
        }
        Some(ManagementResultV2::Budget(ManagementBudgetResultV2::CallReconciled {
            call,
            accounting_overflow,
            ..
        })) => vec![ManagementCommittedEffect::BudgetReconciliation {
            billing_scope: call.billing_scope.clone(),
            call_id: call.call_id.clone(),
            accounting_overflow: *accounting_overflow,
        }],
        // The listing commits only its own job and audit row, not a business
        // fact of the root or its calls.
        Some(ManagementResultV2::Budget(ManagementBudgetResultV2::OpenCallsListed { .. })) => {
            Vec::new()
        }
        Some(ManagementResultV2::Import(ManagementImportResultV2::ImportRegistered {
            selection_id,
            selection_digest,
            ..
        })) => vec![ManagementCommittedEffect::ImportSelection {
            selection_id: selection_id.clone(),
            selection_digest: selection_digest.clone(),
        }],
        Some(ManagementResultV2::Import(ManagementImportResultV2::ImportCompleted {
            selection_id,
            result_id,
            result_digest,
            ..
        })) => vec![ManagementCommittedEffect::ImportResult {
            selection_id: selection_id.clone(),
            result_id: result_id.clone(),
            result_digest: result_digest.clone(),
        }],
        Some(ManagementResultV2::AssetExport(ManagementAssetExportResultV2::AssetStaged {
            staged_asset_id,
            manifest_digest,
            content_blob_digest,
            ..
        })) => vec![ManagementCommittedEffect::AssetStaged {
            staged_asset_id: staged_asset_id.clone(),
            manifest_digest: manifest_digest.clone(),
            content_blob_digest: content_blob_digest.clone(),
        }],
        Some(ManagementResultV2::AssetExport(ManagementAssetExportResultV2::AssetAborted {
            staged_asset_id,
            ..
        })) => vec![ManagementCommittedEffect::AssetAborted {
            staged_asset_id: staged_asset_id.clone(),
        }],
        Some(ManagementResultV2::AssetExport(ManagementAssetExportResultV2::AssetHandedOff {
            staged_asset_id,
            candidate_id,
        })) => vec![ManagementCommittedEffect::AssetHandedOff {
            staged_asset_id: staged_asset_id.clone(),
            candidate_id: candidate_id.clone(),
        }],
        Some(ManagementResultV2::AssetExport(ManagementAssetExportResultV2::ExportPrepared {
            export_id,
            package_tree_digest,
            ..
        })) => vec![ManagementCommittedEffect::ExportPrepared {
            export_id: export_id.clone(),
            package_tree_digest: package_tree_digest.clone(),
        }],
        Some(ManagementResultV2::AssetExport(ManagementAssetExportResultV2::ExportCompleted {
            export_id,
            receipt_digest,
            delivery_audit_id,
            ..
        })) => vec![ManagementCommittedEffect::ExportCompleted {
            export_id: export_id.clone(),
            receipt_digest: receipt_digest.clone(),
            delivery_audit_id: delivery_audit_id.clone(),
        }],
        Some(ManagementResultV2::AssetExport(ManagementAssetExportResultV2::ExportAborted {
            export_id,
            ..
        })) => vec![ManagementCommittedEffect::ExportAborted {
            export_id: export_id.clone(),
        }],
        Some(ManagementResultV2::Original(
            ManagementResult::EvaluationStatus { .. }
            | ManagementResult::EvaluationStatusReadV2 { .. },
        ))
        | None => Vec::new(),
    }
}

fn project_management_status(
    record: &VersionedManagementJob,
    effects: Option<Vec<ManagementCommittedEffect>>,
    fee_knowledge: ManagementFeeKnowledge,
    pending_fees: Option<Vec<ManagementFeeCallView>>,
    replay_read_view: Option<crate::replay::ReplayReportAuditView>,
) -> ManagementJobStatusV1 {
    let job = record.identity();
    let (diagnostics, created_at) = match record {
        VersionedManagementJob::V1(original) => (original.diagnostics.clone(), original.created_at),
        VersionedManagementJob::V2(current) => (current.diagnostics.clone(), current.created_at),
    };
    ManagementJobStatusV1 {
        schema_version: "rsia.management_job_status.v1",
        job_schema_version: job.schema_version.into(),
        id: job.id.into(),
        operation: job.operation.into(),
        request_key: job.request_key.into(),
        payload_digest: job.payload_digest.into(),
        owner_actor: job.owner_actor.into(),
        owner_role: job.owner_role,
        state: job.state,
        step: record.step().into(),
        private_input_ref: job.private_input_ref.into(),
        result: record.result(),
        error_code: record.error_code().map(str::to_owned),
        cancel_requested: record.cancel_requested(),
        cancel_arrival: record.cancel_arrival(),
        lease_until: job.lease_until,
        generation: record.generation(),
        diagnostics,
        created_at,
        pending_terminal_state: record.pending_terminal_state(),
        optimization_stage_refs: match record {
            VersionedManagementJob::V2(current) => current
                .optimization
                .as_ref()
                .map(|progress| progress.stages.clone()),
            VersionedManagementJob::V1(_) => None,
        },
        backup: backup::status(record),
        committed_side_effects: effects,
        fee_knowledge,
        pending_fees,
        replay_read_view,
    }
}

impl ManagementDispatcher {
    /// Render the real committed acceptance snapshot returned by submit. It
    /// retains the original Queued acceptance contract; later GETs read current
    /// worker state through the same one versioned projection below.
    pub async fn submit_status_view(
        &self,
        ctx: &Context,
        operation: &str,
        payload: Value,
    ) -> Result<ManagementJobStatusV1> {
        let record = self.submit(ctx, operation, payload).await?;
        let identity = record.identity();
        if identity.state == ManagementJobState::Queued
            && record.generation() == 0
            && identity.lease_token.is_none()
            && record.result().is_none()
            && !record.cancel_requested()
        {
            return Ok(project_management_status(
                &record,
                Some(Vec::new()),
                ManagementFeeKnowledge::KnownNone,
                Some(Vec::new()),
                None,
            ));
        }
        self.status_view(ctx, identity.id).await
    }

    /// Shared projection of actual store-backed lifecycle reads. Never renews,
    /// claims, audits, cancels calls or performs reconciliation as a side effect.
    pub async fn status_view(&self, ctx: &Context, job_id: &str) -> Result<ManagementJobStatusV1> {
        let read = self.status_with_replay_read_view(ctx, job_id).await?;
        let record = &read.job;
        let job = record.identity();
        let optimization = if job.operation == "optimization.step" {
            let mut session = self.store.session().await?;
            let current = load_job_record_in_session(ctx, &mut session, job_id).await?;
            if fingerprint(&current)? != fingerprint(record)? {
                return Err(Error::Conflict(
                    "management job changed during status projection".into(),
                ));
            }
            let facts = optimization_accounting_or_unknown(ctx, &mut session, &current).await?;
            session.commit().await?;
            Some(facts)
        } else {
            None
        };
        let progress = if let Some(facts) = &optimization {
            match facts {
                Some(facts) => optimization_accounting_progress(facts, record)?,
                _ => ConsumerProgress::Unknown,
            }
        } else if job.state == ManagementJobState::Queued
            && record.generation() == 0
            && record.result().is_none()
            && job.lease_token.is_none()
        {
            // The fresh, never-claimed queue is producer phase evidence only
            // after its real private request and source bindings are admitted.
            let request = load_private_request_record(ctx, &self.store, record)
                .await
                .map_err(InputFault::into_error)?;
            validate_request_authority(ctx, &request)?;
            let mut session = self.store.session().await?;
            ensure_dependencies_live(ctx, &mut session, &private_dependencies(&request)?).await?;
            session.commit().await?;
            ConsumerProgress::NotCommitted
        } else {
            consumer_progress_or_unknown(ctx, &self.store, record).await?
        };
        let progress = if progress_preserves_committed_result(record.result().as_ref(), &progress)?
        {
            progress
        } else {
            ConsumerProgress::Unknown
        };
        let (mut effects, mut fee_knowledge, mut pending_fees) = match progress {
            _ if matches!(record, VersionedManagementJob::V2(current) if current.operation == "experiment.register"
                && current.step == "experiment_already_registered") =>
            {
                (
                    Some(Vec::new()),
                    ManagementFeeKnowledge::KnownNone,
                    Some(Vec::new()),
                )
            }
            ConsumerProgress::Unknown => {
                let prior = record.result();
                let lower_bound = if matches!(record, VersionedManagementJob::V2(current)
                    if current.operation == "experiment.register"
                        && current.step == "current_holdout_registered")
                {
                    None
                } else {
                    stored_result_has_side_effect(prior.as_ref())
                        .then(|| committed_effects(prior.as_ref()))
                };
                (lower_bound, ManagementFeeKnowledge::UnknownProvenance, None)
            }
            ConsumerProgress::NotCommitted => (
                Some(Vec::new()),
                ManagementFeeKnowledge::KnownNone,
                Some(Vec::new()),
            ),
            ConsumerProgress::Committed(ref outcome)
                if matches!(record, VersionedManagementJob::V2(current)
                    if current.operation == "experiment.register"
                        && current.step == "current_holdout_registered") =>
            {
                let mut effects = committed_effects(outcome.result.as_ref());
                effects
                    .retain(|effect| matches!(effect, ManagementCommittedEffect::Holdout { .. }));
                (
                    Some(effects),
                    ManagementFeeKnowledge::KnownNone,
                    Some(Vec::new()),
                )
            }
            ConsumerProgress::Committed(ref outcome) => (
                Some(if outcome.has_committed_side_effect() {
                    committed_effects(outcome.result.as_ref())
                } else {
                    Vec::new()
                }),
                ManagementFeeKnowledge::KnownNone,
                Some(Vec::new()),
            ),
        };
        if let Some(facts) = &optimization {
            match facts {
                Some(facts) => {
                    fee_knowledge = if facts.known_none {
                        ManagementFeeKnowledge::KnownNone
                    } else {
                        ManagementFeeKnowledge::TrustedCalls(facts.views())
                    };
                    pending_fees = Some(facts.pending_views());
                    for fact in &facts.compiled {
                        effects.get_or_insert_with(Vec::new).push(
                            ManagementCommittedEffect::OptimizationCompiled {
                                stage_fact_id: fact.artifact_id.clone(),
                                input_digest: fact.input_digest.clone(),
                            },
                        );
                    }
                    for reference in &facts.historical_compiled {
                        effects.get_or_insert_with(Vec::new).push(
                            ManagementCommittedEffect::OptimizationCompiled {
                                stage_fact_id: reference.artifact_id.clone(),
                                input_digest: reference.input_digest.clone(),
                            },
                        );
                    }
                }
                None => {
                    fee_knowledge = ManagementFeeKnowledge::UnknownProvenance;
                    pending_fees = None;
                }
            }
        }
        if job.operation == "backup.create" {
            fee_knowledge = ManagementFeeKnowledge::KnownNone;
            pending_fees = Some(Vec::new());
        }
        Ok(project_management_status(
            record,
            effects,
            fee_knowledge,
            pending_fees,
            read.replay_read_view,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_current_job(record: VersionedManagementJob) -> ManagementJobV2 {
        match record {
            VersionedManagementJob::V2(job) => job,
            VersionedManagementJob::V1(_) => panic!("expected the current test job wire"),
        }
    }

    #[test]
    fn model_and_management_surfaces_remain_separate() {
        let agent = Context::new("n", "agent", Role::Agent).unwrap();
        let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        assert!(authorize(&agent, "experiment.register").is_err());
        assert!(authorize(&agent, "evo_prepare").is_ok());
        assert!(authorize(&evaluator, "evaluation.start").is_ok());
        assert!(authorize(&admin, "evaluation.start").is_err());
        assert!(authorize(&admin, "replay.run").is_ok());
        assert!(reject_admin_as_model_tool("evaluation.start").is_err());
    }

    #[test]
    fn expired_lease_cannot_checkpoint_or_finish_without_a_new_claim() {
        let job = ManagementJobV2 {
            id: "management-job-expired".into(),
            schema_version: JOB_SCHEMA.into(),
            operation: "meta.start".into(),
            request_key: "expired".into(),
            payload_digest: "a".repeat(64),
            owner_actor: "admin".into(),
            owner_role: Role::Admin,
            state: ManagementJobState::Running,
            step: "claimed".into(),
            private_input_ref: "management-input-expired".into(),
            result: None,
            error_code: None,
            cancel_requested: false,
            lease_token: Some("lease".into()),
            lease_until: 0,
            generation: 1,
            diagnostics: vec![],
            created_at: 1,
            cancel_arrival: None,
            pending_terminal_state: None,
            optimization: None,
            backup: None,
        };
        assert!(matches!(ensure_fence(&job, &job), Err(Error::Conflict(_))));
    }

    #[tokio::test]
    async fn running_background_error_is_persisted_without_overwriting_lease() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("diagnostic.sqlite3"))
            .await
            .unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
        let mut job = ManagementJobV2 {
            id: stable_id("management-job", &admin, "meta.start", "diagnostic"),
            schema_version: JOB_SCHEMA.into(),
            operation: "meta.start".into(),
            request_key: "diagnostic".into(),
            payload_digest: "a".repeat(64),
            owner_actor: "admin".into(),
            owner_role: Role::Admin,
            state: ManagementJobState::Running,
            step: "claimed".into(),
            private_input_ref: stable_id("management-input", &admin, "meta.start", "diagnostic"),
            result: None,
            error_code: None,
            cancel_requested: false,
            lease_token: Some("active-lease".into()),
            lease_until: now().unwrap().saturating_add(60),
            generation: 2,
            diagnostics: vec![],
            created_at: 1,
            cancel_arrival: None,
            pending_terminal_state: None,
            optimization: None,
            backup: None,
        };
        let mut session = store.session().await.unwrap();
        session
            .put(&admin, "job", &job.id, admin.actor(), &job)
            .await
            .unwrap();
        session.commit().await.unwrap();
        dispatcher
            .record_background_error(&job.id, &Error::Conflict("stale fence".into()))
            .await
            .unwrap();
        job = unit_current_job(dispatcher.status(&admin, &job.id).await.unwrap());
        assert_eq!(job.state, ManagementJobState::Running);
        assert_eq!(job.lease_token.as_deref(), Some("active-lease"));
        assert_eq!(job.diagnostics.len(), 1);
        assert_eq!(job.diagnostics[0].code, "conflict");
    }

    #[tokio::test]
    async fn a_tombstone_judges_a_dependency_only_for_a_source_of_its_own_kind() {
        use evo_storage::lifecycle::{REVOKE_TOMBSTONE_SCHEMA, RevokeTombstone};
        use serde_json::json;

        /// What sits under the dependency's id in the `tombstone` kind. A
        /// tombstone is keyed by the source id alone, so a run and an artifact of
        /// one id share it and `source_kind` says which of them it was written for.
        #[derive(Clone, Copy)]
        enum Under {
            Nothing,
            /// A tombstone as `begin_revoke` writes it, for a source of this kind.
            Source(&'static str),
            /// A body `begin_revoke` never writes.
            Corrupt,
        }
        /// The stored body of the dependency itself.
        #[derive(Clone, Copy)]
        enum Stored {
            Absent,
            Live,
            Redacted,
        }
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Verdict {
            Accepted,
            Revoked,
            Unreadable,
            BodyRedacted,
        }
        use {Stored::*, Under::*, Verdict::*};

        // Ids are unique across the rows: a tombstone is keyed by the id alone.
        const RUN: &str = "run";
        const ART: &str = "artifact";
        const OTHER: &str = "release";
        let cases = [
            // A tombstone of the dependency's own kind refuses it.
            (RUN, "run-revoked", Source(RUN), Absent, Revoked),
            (ART, "art-revoked", Source(ART), Live, Revoked),
            // A tombstone of the other kind belongs to a different object, ...
            (ART, "shared-run", Source(RUN), Live, Accepted),
            (RUN, "shared-art", Source(ART), Absent, Accepted),
            // ... and the body check still applies to the artifact.
            (ART, "shared-red", Source(RUN), Redacted, BodyRedacted),
            // Without a tombstone: live, absent and redacted dependencies as before.
            (RUN, "run-live", Nothing, Absent, Accepted),
            (ART, "art-live", Nothing, Live, Accepted),
            (ART, "art-none", Nothing, Absent, Accepted),
            (ART, "art-red", Nothing, Redacted, BodyRedacted),
            // A tombstone `begin_revoke` never writes fails closed, for a run and
            // for an artifact alike (an unknown source kind is one of them).
            (RUN, "run-bad", Corrupt, Absent, Unreadable),
            (ART, "art-bad", Corrupt, Live, Unreadable),
            (ART, "art-odd", Source(OTHER), Live, Unreadable),
            // Another dependency kind is not judged by a tombstone, whatever it
            // says or whether it can be read; only the redacted-body check applies.
            (OTHER, "oth-run", Source(RUN), Live, Accepted),
            (OTHER, "oth-art", Source(ART), Live, Accepted),
            (OTHER, "oth-bad", Corrupt, Live, Accepted),
            (OTHER, "oth-red", Source(RUN), Redacted, BodyRedacted),
        ];

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tombstone-kind.sqlite3"))
            .await
            .unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        for (kind, id, under, stored, _) in cases {
            let tombstone = match under {
                Nothing => None,
                Source(source_kind) => Some(
                    serde_json::to_value(RevokeTombstone {
                        id: id.into(),
                        schema_version: REVOKE_TOMBSTONE_SCHEMA.into(),
                        source_kind: source_kind.into(),
                        reason: "test".into(),
                        watermark_seq: 1,
                        watermark_digest: "a".repeat(64),
                        created_at: 1,
                    })
                    .unwrap(),
                ),
                Corrupt => Some(json!({"id": id})),
            };
            if let Some(tombstone) = tombstone {
                session
                    .put(&admin, "tombstone", id, admin.actor(), &tombstone)
                    .await
                    .unwrap();
            }
            let body = match stored {
                Absent => None,
                Live => Some(json!({"id": id, "schema_version": "fixture.live.v1"})),
                Redacted => Some(json!({"id": id, "schema_version": REDACTED_SCHEMA})),
            };
            if let Some(body) = body {
                session
                    .put(&admin, kind, id, admin.actor(), &body)
                    .await
                    .unwrap();
            }
        }
        session.commit().await.unwrap();

        for (kind, id, _, _, verdict) in cases {
            let mut session = store.session().await.unwrap();
            let outcome = ensure_dependencies_live(
                &admin,
                &mut session,
                &[(kind.to_string(), id.to_string())],
            )
            .await;
            session.commit().await.unwrap();
            let prefix = format!("management request depends on {kind} {id}, ");
            let expected = match verdict {
                Accepted => None,
                Revoked => Some("whose source was revoked"),
                Unreadable => Some(
                    "whose revocation tombstone cannot be read; the source is treated as revoked",
                ),
                BodyRedacted => Some("which was redacted because its source was revoked"),
            };
            match (expected, outcome) {
                (None, Ok(())) => {}
                (Some(tail), Err(Error::Conflict(message))) => {
                    assert_eq!(message, format!("{prefix}{tail}"), "{kind} {id}");
                }
                (_, outcome) => panic!("{kind} {id}: expected {verdict:?}, got {outcome:?}"),
            }
        }

        // The first dependency that fails decides, in the order they are given.
        let mut session = store.session().await.unwrap();
        let outcome = ensure_dependencies_live(
            &admin,
            &mut session,
            &[
                (ART.to_string(), "shared-run".to_string()),
                (RUN.to_string(), "run-revoked".to_string()),
                (ART.to_string(), "art-red".to_string()),
            ],
        )
        .await;
        match outcome {
            Err(Error::Conflict(message)) => {
                assert!(message.contains("run run-revoked"), "{message}")
            }
            other => panic!("expected the revoked run to refuse the set, got {other:?}"),
        }
    }

    fn ag129_r7_sqlite_snapshot(path: &std::path::Path) -> serde_json::Value {
        let script = r#"import json,sqlite3,struct,sys
from pathlib import Path
con=sqlite3.connect(Path(sys.argv[1]).resolve().as_uri()+'?mode=ro',uri=True)
con.text_factory=bytes
con.execute('PRAGMA query_only=ON')
con.execute('BEGIN')
def quoted(name):
    return '"'+name.replace('"','""')+'"'
def cell(kind,value):
    kind=kind.decode('ascii')
    if kind in ('text','blob'):
        data=value.hex()
    elif kind=='integer':
        data=str(value)
    elif kind=='real':
        data=struct.pack('>d',value).hex()
    elif kind=='null':
        data=''
    else:
        raise ValueError('unknown SQLite storage class')
    return [kind,data]
result={}
for (raw_name,) in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    name=raw_name.decode('utf-8')
    columns=[row[1].decode('utf-8') for row in con.execute('PRAGMA table_info('+quoted(name)+')')]
    projection=','.join('typeof('+quoted(column)+'),'+quoted(column) for column in columns)
    rows=con.execute('SELECT '+projection+' FROM '+quoted(name)).fetchall()
    result[name]={'columns':columns,'rows':sorted([[cell(row[i],row[i+1]) for i in range(0,len(row),2)] for row in rows])}
con.rollback()
con.close()
print(json.dumps(result,sort_keys=True))
"#;
        let output = std::process::Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg("-c")
            .arg(script)
            .arg(path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn ag129_unit_clock_error() -> Error {
        // Trusted input-level conversion only; this never changes the host clock.
        evo_core::clock::unix_seconds_at(std::time::UNIX_EPOCH - std::time::Duration::from_secs(1))
            .unwrap_err()
    }

    fn ag129_unit_job(state: ManagementJobState) -> ManagementJobV2 {
        let ctx = Context::new("n", "admin", Role::Admin).unwrap();
        ManagementJobV2 {
            id: stable_id("management-job", &ctx, "meta.start", "clock-boundary"),
            schema_version: JOB_SCHEMA.into(),
            operation: "meta.start".into(),
            request_key: "clock-boundary".into(),
            payload_digest: "a".repeat(64),
            owner_actor: "admin".into(),
            owner_role: Role::Admin,
            state,
            step: "claimed".into(),
            private_input_ref: stable_id("management-input", &ctx, "meta.start", "clock-boundary"),
            result: None,
            error_code: None,
            cancel_requested: false,
            lease_token: Some("original-lease".into()),
            lease_until: 100,
            generation: 2,
            diagnostics: vec![],
            created_at: 1,
            cancel_arrival: None,
            pending_terminal_state: None,
            optimization: None,
            backup: None,
        }
    }

    #[test]
    fn ag129_private_outcome_propagates_clock_and_keeps_other_failure_diagnostics() {
        let clock = ag129_unit_clock_error();
        let refused =
            execution_outcome(ag129_unit_job(ManagementJobState::Running), Err(clock)).unwrap_err();
        assert_eq!(refused.to_string(), "clock_regressed");
        for (error, code) in [
            (Error::Internal, "internal_error"),
            (Error::Forbidden, "forbidden"),
            (Error::Budget, "budget_unavailable"),
            (Error::Conflict("old refusal".into()), "conflict"),
            (Error::Cancelled, "cancelled"),
            (Error::NotFound, "not_found"),
            (Error::Invalid("old invalid".into()), "invalid_input"),
        ] {
            let result =
                execution_outcome(ag129_unit_job(ManagementJobState::Running), Err(error)).unwrap();
            assert_eq!(result.state, ManagementJobState::Failed);
            assert_eq!(result.step, "failed");
            assert_eq!(result.error_code.as_deref(), Some(code));
            assert!(result.result.is_none());
        }
    }

    #[tokio::test]
    async fn ag129_clock_background_error_keeps_queued_and_running_job_tables() {
        for state in [ManagementJobState::Queued, ManagementJobState::Running] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("clock-job.sqlite3");
            let store = Store::open(&path).await.unwrap();
            let admin = Context::new("n", "admin", Role::Admin).unwrap();
            let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
            let job = ag129_unit_job(state);
            let mut session = store.session().await.unwrap();
            session
                .put(&admin, "job", &job.id, admin.actor(), &job)
                .await
                .unwrap();
            session.commit().await.unwrap();
            let before = ag129_r7_sqlite_snapshot(&path);
            let error = dispatcher
                .record_background_error(&job.id, &ag129_unit_clock_error())
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "clock_regressed");
            assert_eq!(ag129_r7_sqlite_snapshot(&path), before);
            let mut session = store.session().await.unwrap();
            let after: ManagementJobV2 = session.need(&admin, "job", &job.id).await.unwrap();
            session.commit().await.unwrap();
            assert_eq!(
                serde_json::to_value(after).unwrap(),
                serde_json::to_value(job).unwrap()
            );
            store.close().await;
        }
    }
}

#[cfg(test)]
#[path = "dispatch_late_cancel_tests.rs"]
mod late_cancel_tests;

// The original public_evaluation_status and worker durable result stay unchanged.
fn public_evaluation_status_read(
    status: &StreamingEvaluationStatus,
    report_read: crate::streaming_evaluator::FormalReportReadMetadata,
) -> Result<ManagementResult> {
    let lifecycle = serde_json::to_value(status.ticket.lifecycle)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    Ok(ManagementResult::EvaluationStatusReadV2 {
        ticket_id: status.ticket.id.clone(),
        lifecycle,
        planned_targets: status.ticket.targets.len(),
        has_formal_terminal: status.formal.is_some(),
        report_read: serde_json::to_value(report_read).map_err(|_| Error::Internal)?,
    })
}
