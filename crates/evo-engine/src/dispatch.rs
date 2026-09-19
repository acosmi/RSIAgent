//! Role-gated model and persistent management dispatch.
use crate::streaming_evaluator::{
    IndependentEvaluationControl, IssueTicketRequest, ProtectedHoldoutRecord,
    RegisteredEvaluationControl, StreamingEvaluationStatus,
};
use evo_core::contract::{MODEL_TOOLS, admin_ops, reject_admin_as_model_tool};
use evo_core::replay::{ReplaySimulationProfile, WorldPartition};
use evo_core::strategy::{ElasticPolicyV1, ExplorationCapsV1};
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier, now};
use evo_storage::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const JOB_SCHEMA: &str = "rsia.management_job.v1";
const INPUT_SCHEMA: &str = "rsia.management_private_input.v1";

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
        "exploration.start" | "replay.run" | "curriculum.step" | "meta.start" => {
            ctx.require(&[Role::Admin])
        }
        _ => Err(Error::Invalid("unknown management operation".into())),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagementJobState {
    Queued,
    Running,
    Draining,
    Succeeded,
    Failed,
    Cancelled,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result", deny_unknown_fields)]
pub enum ManagementResult {
    ExperimentRegistered {
        control_id: String,
        control_digest: String,
        holdout_id: String,
        holdout_digest: String,
    },
    EvaluationTicketBlocked {
        ticket_id: String,
    },
    EvaluationStatus {
        ticket_id: String,
        lifecycle: String,
        planned_targets: usize,
        has_formal_terminal: bool,
    },
    ReplayStored {
        report_id: String,
        pool_digest: String,
        semantic_reports_digest: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementDiagnostic {
    pub code: String,
    pub observed_generation: u64,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementJob {
    pub id: String,
    pub schema_version: String,
    pub operation: String,
    pub request_key: String,
    pub payload_digest: String,
    pub owner_actor: String,
    pub owner_role: Role,
    pub state: ManagementJobState,
    pub step: String,
    pub private_input_ref: String,
    pub result: Option<ManagementResult>,
    pub error_code: Option<String>,
    pub cancel_requested: bool,
    pub lease_token: Option<String>,
    pub lease_until: i64,
    pub generation: u64,
    #[serde(default)]
    pub diagnostics: Vec<ManagementDiagnostic>,
    pub created_at: i64,
}

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
pub struct BlockedManagementRequest {
    pub schema_version: String,
    pub request_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "operation")]
enum ParsedRequest {
    ExperimentRegister(Box<ExperimentRegisterRequest>),
    EvaluationStart(EvaluationStartRequest),
    EvaluationStatus(EvaluationStatusRequest),
    ReplayRun(Box<ReplayRunRequest>),
    Blocked(BlockedManagementRequest),
}

#[derive(Clone)]
pub struct ManagementDispatcher {
    store: Store,
    trusted_contexts: Arc<Vec<Context>>,
    recovery_started: Arc<AtomicBool>,
}

impl ManagementDispatcher {
    pub fn new(store: Store, trusted_contexts: Vec<Context>) -> Result<Self> {
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
        })
    }

    pub async fn recover_pending(&self) -> Result<usize> {
        if self.recovery_started.swap(true, Ordering::AcqRel) {
            return Ok(0);
        }
        let mut pending = Vec::new();
        for context in self.trusted_contexts.iter() {
            let mut session = self.store.session().await?;
            let values = session.list::<Value>(context, "job").await?;
            for value in values {
                let schema = value.get("schema_version").and_then(Value::as_str);
                if schema != Some(JOB_SCHEMA) {
                    if schema.is_some_and(|schema| schema.starts_with("rsia.management_job.")) {
                        return Err(Error::Conflict(
                            "unknown management job schema during recovery".into(),
                        ));
                    }
                    continue;
                }
                let mut job: ManagementJob = serde_json::from_value(value)
                    .map_err(|_| Error::Conflict("damaged management job".into()))?;
                if job.owner_actor != context.actor() || job.owner_role != context.role() {
                    continue;
                }
                if !is_terminal(job.state) {
                    if job.state == ManagementJobState::Running {
                        job.lease_until = 0;
                        session
                            .put(context, "job", &job.id, context.actor(), &job)
                            .await?;
                    }
                    pending.push(job.id);
                }
            }
            session.commit().await?;
        }
        for job_id in &pending {
            self.spawn_job(job_id.clone());
        }
        Ok(pending.len())
    }

    pub async fn submit(
        &self,
        ctx: &Context,
        operation: &str,
        payload: Value,
    ) -> Result<ManagementJob> {
        self.require_trusted(ctx)?;
        let job = persist_management(ctx, &self.store, operation, payload).await?;
        if !is_terminal(job.state) {
            self.spawn_job(job.id.clone());
        }
        Ok(job)
    }

    pub async fn status(&self, ctx: &Context, job_id: &str) -> Result<ManagementJob> {
        self.require_trusted(ctx)?;
        let mut job = load_job(ctx, &self.store, job_id).await?;
        if job.operation == "evaluation.status" && job.state == ManagementJobState::Succeeded {
            let request = load_private_request(ctx, &self.store, &job).await?;
            if let ParsedRequest::EvaluationStatus(request) = request {
                let status =
                    IndependentEvaluationControl::status(ctx, &self.store, &request.ticket_id)
                        .await?;
                job.result = Some(public_evaluation_status(&status));
            }
        }
        if job.operation == "replay.run" && job.state == ManagementJobState::Succeeded {
            if let Some(ManagementResult::ReplayStored {
                report_id,
                pool_digest,
                semantic_reports_digest,
            }) = &job.result
            {
                let view =
                    crate::replay::verified_replay_report_view(ctx, &self.store, report_id).await?;
                if &view.report_id != report_id
                    || &view.pool_digest != pool_digest
                    || &view.semantic_digest != semantic_reports_digest
                {
                    return Err(Error::Conflict("replay report view digest mismatch".into()));
                }
            } else {
                return Err(Error::Conflict(
                    "replay job succeeded without valid ReplayStored result".into(),
                ));
            }
        }
        Ok(job)
    }

    pub async fn cancel(&self, ctx: &Context, job_id: &str) -> Result<ManagementJob> {
        self.require_trusted(ctx)?;
        cancel_job_atomic(ctx, &self.store, job_id).await
    }

    fn spawn_job(&self, job_id: String) {
        let dispatcher = self.clone();
        tokio::spawn(async move {
            if let Err(error) = dispatcher.run_job(&job_id).await {
                tracing::error!(job_id = %job_id, error = %error, "management background worker failed");
                let _ = dispatcher.record_background_error(&job_id, &error).await;
            }
        });
    }

    async fn run_job(&self, job_id: &str) -> Result<()> {
        let (ctx, claimed) = self.claim(job_id).await?;
        let Some(job) = claimed else { return Ok(()) };
        let request = match load_private_request(&ctx, &self.store, &job).await {
            Ok(request) => request,
            Err(error) => {
                return finish_claim(
                    &ctx,
                    &self.store,
                    &job,
                    JobOutcome {
                        state: ManagementJobState::Failed,
                        step: "private_input_invalid".into(),
                        result: None,
                        error_code: Some(error_code(&error).into()),
                    },
                )
                .await;
            }
        };
        let outcome = execute_request(&ctx, &self.store, &job, request).await;
        finish_claim(&ctx, &self.store, &job, outcome).await
    }

    async fn claim(&self, job_id: &str) -> Result<(Context, Option<ManagementJob>)> {
        identifier(job_id)?;
        for trusted in self.trusted_contexts.iter() {
            let mut session = self.store.session().await?;
            let Some(mut job) = session.get::<ManagementJob>(trusted, "job", job_id).await? else {
                session.commit().await?;
                continue;
            };
            if job.schema_version != JOB_SCHEMA
                || job.owner_actor != trusted.actor()
                || job.owner_role != trusted.role()
            {
                session.commit().await?;
                continue;
            }
            if is_terminal(job.state)
                || (job.state == ManagementJobState::Running && job.lease_until > now())
            {
                session.commit().await?;
                return Ok((trusted.clone(), None));
            }
            job.generation = job
                .generation
                .checked_add(1)
                .ok_or_else(|| Error::Conflict("management job generation overflow".into()))?;
            let token = hash(format!("{}\0{}\0{}", job.id, job.generation, now()).as_bytes());
            job.lease_token = Some(token);
            job.lease_until = now().saturating_add(60);
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

    async fn record_background_error(&self, job_id: &str, error: &Error) -> Result<()> {
        for context in self.trusted_contexts.iter() {
            let mut session = self.store.session().await?;
            let Some(value) = session.get::<Value>(context, "job", job_id).await? else {
                session.commit().await?;
                continue;
            };
            if value.get("schema_version").and_then(Value::as_str) != Some(JOB_SCHEMA) {
                return Err(Error::Conflict(
                    "background error belongs to an unreadable management job".into(),
                ));
            }
            let mut job: ManagementJob = serde_json::from_value(value)
                .map_err(|_| Error::Conflict("damaged management job".into()))?;
            if job.owner_actor != context.actor() || job.owner_role != context.role() {
                session.commit().await?;
                continue;
            }
            if !is_terminal(job.state) {
                if job.diagnostics.len() >= 32 {
                    job.diagnostics.remove(0);
                }
                job.diagnostics.push(ManagementDiagnostic {
                    code: error_code(error).into(),
                    observed_generation: job.generation,
                    recorded_at: now(),
                });
                if job.state == ManagementJobState::Queued {
                    job.state = ManagementJobState::Failed;
                    job.step = "background_failed".into();
                    job.error_code = Some(error_code(error).into());
                    job.lease_token = None;
                    job.lease_until = 0;
                }
                session
                    .put(context, "job", &job.id, context.actor(), &job)
                    .await?;
                session
                    .audit(context, "management.job.background_failed", &job.id)
                    .await?;
            }
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
            Self::EvaluationStart(request) => &request.request_key,
            Self::EvaluationStatus(request) => &request.request_key,
            Self::ReplayRun(request) => &request.request_key,
            Self::Blocked(request) => &request.request_key,
        }
    }
}

async fn persist_management(
    ctx: &Context,
    store: &Store,
    operation: &str,
    payload: Value,
) -> Result<ManagementJob> {
    authorize(ctx, operation)?;
    let request = parse_request(operation, payload)?;
    validate_request_authority(ctx, &request)?;
    identifier(request.request_key())?;
    let payload_digest = fingerprint(&request)?;
    let job_id = stable_id("management-job", ctx, operation, request.request_key());
    let private_id = stable_id("management-input", ctx, operation, request.request_key());
    let mut session = store.session().await?;
    if let Some(cached) = session
        .cached::<ManagementJob, _>(ctx, operation, request.request_key(), &request)
        .await?
    {
        let current: ManagementJob = session.need(ctx, "job", &cached.id).await?;
        ensure_owner(ctx, &current)?;
        session.commit().await?;
        return Ok(current);
    }
    let private = PrivateManagementInput {
        id: private_id.clone(),
        schema_version: INPUT_SCHEMA.into(),
        operation: operation.into(),
        payload_digest: payload_digest.clone(),
        payload: serde_json::to_value(&request).map_err(|_| Error::Internal)?,
    };
    let job = ManagementJob {
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
        created_at: now(),
    };
    session
        .put(ctx, "artifact", &private_id, ctx.actor(), &private)
        .await?;
    for (kind, id) in private_dependencies(&request)? {
        session
            .put_edge(ctx, "artifact", &private_id, &kind, &id)
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
    Ok(job)
}

async fn load_job(ctx: &Context, store: &Store, job_id: &str) -> Result<ManagementJob> {
    identifier(job_id)?;
    let mut session = store.session().await?;
    let job: ManagementJob = session.need(ctx, "job", job_id).await?;
    ensure_owner(ctx, &job)?;
    session.commit().await?;
    Ok(job)
}

#[derive(Debug)]
struct JobOutcome {
    state: ManagementJobState,
    step: String,
    result: Option<ManagementResult>,
    error_code: Option<String>,
}

async fn execute_request(
    ctx: &Context,
    store: &Store,
    claimed: &ManagementJob,
    request: ParsedRequest,
) -> JobOutcome {
    let mut job = claimed.clone();
    let result = match request {
        ParsedRequest::ExperimentRegister(request) => {
            run_experiment_register(ctx, store, &mut job, *request).await
        }
        ParsedRequest::EvaluationStart(request) => {
            run_evaluation_start(ctx, store, &mut job, request).await
        }
        ParsedRequest::EvaluationStatus(request) => {
            run_evaluation_status(ctx, store, &mut job, request).await
        }
        ParsedRequest::ReplayRun(request) => run_replay(ctx, store, &mut job, *request).await,
        ParsedRequest::Blocked(_) => {
            job.state = ManagementJobState::Blocked;
            job.step = "blocked_feature".into();
            job.error_code = Some(format!("{}_consumer_unavailable", job.operation));
            Ok(())
        }
    };
    if let Err(error) = result {
        job.state = ManagementJobState::Failed;
        job.step = "failed".into();
        job.error_code = Some(error_code(&error).into());
    }
    JobOutcome {
        state: job.state,
        step: job.step,
        result: job.result,
        error_code: job.error_code,
    }
}

async fn run_experiment_register(
    ctx: &Context,
    store: &Store,
    job: &mut ManagementJob,
    request: ExperimentRegisterRequest,
) -> Result<()> {
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
    if progress.control_digest.is_none() {
        IndependentEvaluationControl::register_control(ctx, store, request.control.clone()).await?;
        job.step = "control_registered".into();
        checkpoint_claim(ctx, store, job, "control_registered").await?;
        progress = IndependentEvaluationControl::registration_progress(
            ctx,
            store,
            &request.control.id,
            &request.holdout.id,
        )
        .await?;
    }
    if progress.holdout_digest.is_none() {
        checkpoint_claim(ctx, store, job, "before_holdout_registration").await?;
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
    job.result = Some(ManagementResult::ExperimentRegistered {
        control_id: request.control.id,
        control_digest: expected_control,
        holdout_id: request.holdout.id,
        holdout_digest: expected_holdout,
    });
    Ok(())
}

async fn run_evaluation_start(
    ctx: &Context,
    store: &Store,
    job: &mut ManagementJob,
    request: EvaluationStartRequest,
) -> Result<()> {
    checkpoint_claim(ctx, store, job, "before_ticket_issue").await?;
    let ticket = match IndependentEvaluationControl::issue_ticket(
        ctx,
        store,
        request.ticket.engine_request(),
    )
    .await
    {
        Ok(ticket) => ticket,
        Err(Error::Conflict(_)) => {
            let existing =
                IndependentEvaluationControl::status(ctx, store, &request.ticket.ticket_id).await?;
            if existing.ticket.registration_id != request.ticket.registration_id
                || existing.ticket.holdout_id != request.ticket.holdout_id
                || existing.ticket.issued_at_unix_seconds != request.ticket.issued_at_unix_seconds
                || existing.ticket.evaluator_actor != ctx.actor()
            {
                return Err(Error::Conflict(
                    "existing ticket differs from request".into(),
                ));
            }
            existing.ticket
        }
        Err(error) => return Err(error),
    };
    job.state = ManagementJobState::Blocked;
    job.step = "ticket_issued_execution_blocked".into();
    job.error_code = Some("execution_provider_unconfigured".into());
    job.result = Some(ManagementResult::EvaluationTicketBlocked {
        ticket_id: ticket.id,
    });
    Ok(())
}

async fn run_evaluation_status(
    ctx: &Context,
    store: &Store,
    job: &mut ManagementJob,
    request: EvaluationStatusRequest,
) -> Result<()> {
    checkpoint_claim(ctx, store, job, "before_status_read").await?;
    let status = IndependentEvaluationControl::status(ctx, store, &request.ticket_id).await?;
    job.state = ManagementJobState::Succeeded;
    job.step = "status_read".into();
    job.result = Some(public_evaluation_status(&status));
    Ok(())
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

async fn run_replay(
    ctx: &Context,
    store: &Store,
    job: &mut ManagementJob,
    request: ReplayRunRequest,
) -> Result<()> {
    checkpoint_claim(ctx, store, job, "before_replay_run").await?;
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
    job.result = Some(ManagementResult::ReplayStored {
        report_id: stored.report_id,
        pool_digest: stored.pool_digest,
        semantic_reports_digest: stored.semantic_reports_digest,
    });
    Ok(())
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
        ParsedRequest::ReplayRun(_) => {
            ctx.require(&[Role::Admin])?;
        }
        _ => {}
    }
    Ok(())
}

async fn load_private_request(
    ctx: &Context,
    store: &Store,
    job: &ManagementJob,
) -> Result<ParsedRequest> {
    let mut session = store.session().await?;
    let private: PrivateManagementInput = session
        .need(ctx, "artifact", &job.private_input_ref)
        .await?;
    session.commit().await?;
    if private.id != job.private_input_ref
        || private.schema_version != INPUT_SCHEMA
        || private.operation != job.operation
        || private.payload_digest != job.payload_digest
    {
        return Err(Error::Conflict(
            "private management input differs from job".into(),
        ));
    }
    let request: ParsedRequest = serde_json::from_value(private.payload)
        .map_err(|_| Error::Invalid("invalid persisted management input".into()))?;
    if fingerprint(&request)? != job.payload_digest {
        return Err(Error::Conflict(
            "persisted management payload digest changed".into(),
        ));
    }
    Ok(request)
}

async fn checkpoint_claim(
    ctx: &Context,
    store: &Store,
    claimed: &ManagementJob,
    step: &str,
) -> Result<()> {
    let mut session = store.session().await?;
    let mut current: ManagementJob = session.need(ctx, "job", &claimed.id).await?;
    ensure_owner(ctx, &current)?;
    ensure_fence(&current, claimed)?;
    if current.cancel_requested {
        current.state = ManagementJobState::Cancelled;
        current.step = "cancelled".into();
        current.lease_token = None;
        current.lease_until = 0;
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session.commit().await?;
        return Err(Error::Cancelled);
    }
    current.step = step.into();
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session.commit().await
}

async fn finish_claim(
    ctx: &Context,
    store: &Store,
    claimed: &ManagementJob,
    outcome: JobOutcome,
) -> Result<()> {
    let mut session = store.session().await?;
    let mut current: ManagementJob = session.need(ctx, "job", &claimed.id).await?;
    ensure_owner(ctx, &current)?;
    if is_terminal(current.state) {
        session.commit().await?;
        return Ok(());
    }
    ensure_fence(&current, claimed)?;
    if current.cancel_requested {
        current.state = ManagementJobState::Cancelled;
        current.step = "cancelled".into();
        current.result = None;
        current.error_code = Some("cancelled".into());
    } else {
        current.state = outcome.state;
        current.step = outcome.step;
        current.result = outcome.result;
        current.error_code = outcome.error_code;
    }
    current.lease_token = None;
    current.lease_until = 0;
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.job.finish", &current.id)
        .await?;
    session.commit().await
}

async fn cancel_job_atomic(ctx: &Context, store: &Store, job_id: &str) -> Result<ManagementJob> {
    identifier(job_id)?;
    let mut session = store.session().await?;
    let mut job: ManagementJob = session.need(ctx, "job", job_id).await?;
    ensure_owner(ctx, &job)?;
    if !is_terminal(job.state) {
        job.cancel_requested = true;
        if job.state == ManagementJobState::Queued {
            job.state = ManagementJobState::Cancelled;
            job.step = "cancelled_before_claim".into();
            job.error_code = Some("cancelled".into());
        }
        session.put(ctx, "job", &job.id, ctx.actor(), &job).await?;
        session.audit(ctx, "management.job.cancel", &job.id).await?;
    }
    session.commit().await?;
    Ok(job)
}

fn ensure_fence(current: &ManagementJob, claimed: &ManagementJob) -> Result<()> {
    if current.state != ManagementJobState::Running
        || current.generation != claimed.generation
        || current.lease_token != claimed.lease_token
        || current.lease_until <= now()
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
    if job.schema_version != JOB_SCHEMA
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
        "experiment.register" => ParsedRequest::ExperimentRegister(Box::new(
            serde_json::from_value(payload).map_err(|_| invalid())?,
        )),
        "evaluation.start" => {
            ParsedRequest::EvaluationStart(serde_json::from_value(payload).map_err(|_| invalid())?)
        }
        "evaluation.status" => {
            ParsedRequest::EvaluationStatus(serde_json::from_value(payload).map_err(|_| invalid())?)
        }
        "replay.run" => ParsedRequest::ReplayRun(Box::new(
            serde_json::from_value(payload).map_err(|_| invalid())?,
        )),
        "exploration.start" | "curriculum.step" | "meta.start" => {
            ParsedRequest::Blocked(serde_json::from_value(payload).map_err(|_| invalid())?)
        }
        _ => return Err(Error::Invalid("unknown management operation".into())),
    };
    let expected = format!("rsia.management.{}.v1", operation.replace('.', "_"));
    let actual = match &parsed {
        ParsedRequest::ExperimentRegister(request) => &request.schema_version,
        ParsedRequest::EvaluationStart(request) => &request.schema_version,
        ParsedRequest::EvaluationStatus(request) => &request.schema_version,
        ParsedRequest::ReplayRun(request) => &request.schema_version,
        ParsedRequest::Blocked(request) => &request.schema_version,
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
        ParsedRequest::Blocked(_) => {}
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
        Error::Conflict(_) => "conflict",
        Error::Budget => "budget_unavailable",
        Error::Cancelled => "cancelled",
        Error::Internal => "internal_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let job = ManagementJob {
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
        let mut job = ManagementJob {
            id: "management-job-diagnostic".into(),
            schema_version: JOB_SCHEMA.into(),
            operation: "meta.start".into(),
            request_key: "diagnostic".into(),
            payload_digest: "a".repeat(64),
            owner_actor: "admin".into(),
            owner_role: Role::Admin,
            state: ManagementJobState::Running,
            step: "claimed".into(),
            private_input_ref: "management-input-diagnostic".into(),
            result: None,
            error_code: None,
            cancel_requested: false,
            lease_token: Some("active-lease".into()),
            lease_until: now().saturating_add(60),
            generation: 2,
            diagnostics: vec![],
            created_at: 1,
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
        job = dispatcher.status(&admin, &job.id).await.unwrap();
        assert_eq!(job.state, ManagementJobState::Running);
        assert_eq!(job.lease_token.as_deref(), Some("active-lease"));
        assert_eq!(job.diagnostics.len(), 1);
        assert_eq!(job.diagnostics[0].code, "conflict");
    }
}
