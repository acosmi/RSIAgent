//! Replay vs online pairing. Replay-only wins do not publish.
use crate::replay::ReplayReport;
use evo_core::evaluation::Verdict;
use evo_core::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct PairCost {
    pub history: u64,
    pub generate: u64,
    pub replay: u64,
    pub accept: u64,
    pub review: u64,
}

impl PairCost {
    pub fn total(self) -> u64 {
        self.history + self.generate + self.replay + self.accept + self.review
    }
}

pub fn decide_publish(
    replay_better: bool,
    online_verdict: Verdict,
    cost: PairCost,
) -> Result<bool> {
    if cost.total() == 0 {
        return Err(Error::Invalid(
            "cost must be fully accounted; calls-only is insufficient".into(),
        ));
    }
    if replay_better && (online_verdict == Verdict::Regressed || online_verdict == Verdict::Invalid)
    {
        return Ok(false);
    }
    if online_verdict == Verdict::Improved || online_verdict == Verdict::Noninferior {
        return Ok(true);
    }
    Ok(false)
}

pub fn replay_report_is_not_online_evidence(_: &ReplayReport) -> bool {
    true
}

use crate::replay::{VerifiedReplayReportView, verified_replay_report_view};
use crate::streaming_evaluator::IndependentEvaluationControl;
use evo_core::fingerprint;
use evo_core::replay::{OBJECTIVE_V2, ReplayTerminal, WorldPartition};
use evo_core::replay_economics::{
    COST_RECEIPT_SCHEMA, ComponentBillingSourceV1, CostComponentKind, CostComponentReceiptV1,
    CostScope, CostSourceKind, EconomicComputation, EconomicReportTerminal, MeasurementState,
    PairedQualityOutcome, REPORT_SCHEMA, ReplayEconomicExperimentV1, ReplayEconomicReportV1,
    compute_economics,
};
use evo_core::{Context, Role, identifier};
use evo_storage::budget::BudgetCallState;
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const EXPERIMENT_KIND: &str = "replay_economic_experiment_v1";
const JOB_KIND: &str = "replay_economic_job_v1";
const COST_KIND: &str = "replay_economic_cost_receipt_v1";
const REPORT_KIND: &str = "replay_economic_report_v1";
const ENVELOPE_SCHEMA: &str = "rsia.replay_economic_artifact_envelope.v1";

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
pub enum ReplayEconomicJobState {
    Registered,
    BlockedSupport,
    UsageUncertain,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayEconomicJobV1 {
    pub schema_version: String,
    pub id: String,
    pub experiment_id: String,
    pub request_key: String,
    pub request_digest: String,
    pub paired_ticket_id: String,
    pub state: ReplayEconomicJobState,
    pub blocked_reasons: Vec<String>,
    pub cost_receipt_ids: Vec<String>,
    pub report_id: Option<String>,
    pub cancel_reason: Option<String>,
    pub created_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayEconomicStatusV1 {
    pub job_id: String,
    pub experiment_id: String,
    pub state: ReplayEconomicJobState,
    pub paired_ticket_id: String,
    pub cost_receipt_count: u32,
    pub report_id: Option<String>,
    pub blocked_reasons: Vec<String>,
}

pub struct PersistentReplayEconomicCoordinator;

impl PersistentReplayEconomicCoordinator {
    pub async fn register(
        ctx: &Context,
        store: &Store,
        experiment: ReplayEconomicExperimentV1,
    ) -> Result<String> {
        ctx.require(&[Role::Evaluator])?;
        experiment.validate()?;
        if experiment.evaluator_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        let digest = experiment.digest()?;
        load_live_selection(ctx, store, &experiment).await?;
        let mut session = store.session().await?;
        verify_experiment_sources(&mut session, ctx, &experiment).await?;
        if let Some(existing) = get_record::<ReplayEconomicExperimentV1>(
            &mut session,
            ctx,
            EXPERIMENT_KIND,
            &experiment.id,
        )
        .await?
        {
            if fingerprint(&existing)? != fingerprint(&experiment)? {
                return Err(Error::Conflict(
                    "economic experiment idempotency conflict".into(),
                ));
            }
            session.commit().await?;
            return Ok(digest);
        }
        put_record(
            &mut session,
            ctx,
            EXPERIMENT_KIND,
            &experiment.id,
            ctx.actor(),
            &experiment,
        )
        .await?;
        let stored_id = storage_id(EXPERIMENT_KIND, &experiment.id)?;
        for source in experiment
            .sources
            .iter()
            .map(|source| &source.id)
            .chain(std::iter::once(
                &experiment.replay_selection.report_artifact_id,
            ))
        {
            session
                .put_edge(ctx, "artifact", &stored_id, "artifact", source)
                .await?;
        }
        session
            .audit(ctx, "replay_economic.register", &experiment.id)
            .await?;
        session.commit().await?;
        Ok(digest)
    }

    pub async fn start(
        ctx: &Context,
        store: &Store,
        experiment_id: &str,
        request_key: &str,
        created_seq: u64,
    ) -> Result<ReplayEconomicJobV1> {
        ctx.require(&[Role::Evaluator])?;
        identifier(experiment_id)?;
        identifier(request_key)?;
        if created_seq == 0 {
            return Err(Error::Invalid("job created_seq must be positive".into()));
        }
        let mut session = store.session().await?;
        let experiment: ReplayEconomicExperimentV1 =
            need_record(&mut session, ctx, EXPERIMENT_KIND, experiment_id).await?;
        if experiment.evaluator_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        verify_experiment_sources(&mut session, ctx, &experiment).await?;
        let request_digest = fingerprint(&(
            experiment.digest()?,
            request_key,
            created_seq,
            experiment.paired_ticket_id.as_str(),
        ))?;
        let job_id = format!(
            "replay-economic-job-{}",
            &fingerprint(&(experiment_id, request_key))?[..24]
        );
        if let Some(existing) =
            get_record::<ReplayEconomicJobV1>(&mut session, ctx, JOB_KIND, &job_id).await?
        {
            if existing.request_digest != request_digest {
                return Err(Error::Conflict("economic start key changed input".into()));
            }
            session.commit().await?;
            return Ok(existing);
        }
        session.commit().await?;
        load_live_selection(ctx, store, &experiment).await?;

        let mut reasons = vec!["real_provider_and_independent_runner_unavailable".to_string()];
        match IndependentEvaluationControl::status(ctx, store, &experiment.paired_ticket_id).await {
            Ok(status) => {
                let ticket = status.ticket;
                if ticket.id != experiment.paired_ticket_id
                    || fingerprint(&ticket)? != experiment.paired_ticket_digest
                    || ticket.candidate_bundle_digest != experiment.runtime.candidate_bundle_digest
                    || ticket.baseline_bundle_digest != experiment.runtime.baseline_bundle_digest
                    || ticket.environment_digest != experiment.runtime.environment_digest
                    || ticket.evaluator_actor != experiment.evaluator_actor
                    || ticket.executor_actor != experiment.executor_actor
                    || ticket.proposer_actor != experiment.proposer_actor
                    || ticket.approver_actor != experiment.approver_actor
                {
                    reasons.push("paired_e05_ticket_binding_mismatch".into());
                }
                if status.formal.is_some() {
                    match IndependentEvaluationControl::verified_report_view(
                        ctx,
                        store,
                        &experiment.paired_ticket_id,
                    )
                    .await
                    {
                        Ok(view) => {
                            if !view.promotion_eligible {
                                reasons.extend(view.ineligibility_reasons);
                            }
                        }
                        Err(_) => reasons.push("e05_verified_report_unavailable".into()),
                    }
                } else {
                    reasons.push("e05_paired_report_not_complete".into());
                }
            }
            Err(Error::NotFound) => reasons.push("e05_paired_ticket_not_found".into()),
            Err(error) => reasons.push(format!("e05_paired_ticket_unavailable: {error}")),
        }
        reasons.sort();
        reasons.dedup();
        let job = ReplayEconomicJobV1 {
            schema_version: "rsia.replay_economic_job.v1".into(),
            id: job_id.clone(),
            experiment_id: experiment.id.clone(),
            request_key: request_key.into(),
            request_digest,
            paired_ticket_id: experiment.paired_ticket_id.clone(),
            state: ReplayEconomicJobState::BlockedSupport,
            blocked_reasons: reasons,
            cost_receipt_ids: vec![],
            report_id: None,
            cancel_reason: None,
            created_seq,
        };
        let mut session = store.session().await?;
        if let Some(existing) =
            get_record::<ReplayEconomicJobV1>(&mut session, ctx, JOB_KIND, &job_id).await?
        {
            if existing.request_digest != job.request_digest {
                return Err(Error::Conflict("economic job concurrently changed".into()));
            }
            session.commit().await?;
            return Ok(existing);
        }
        put_record(&mut session, ctx, JOB_KIND, &job.id, ctx.actor(), &job).await?;
        let job_storage_id = storage_id(JOB_KIND, &job.id)?;
        let experiment_storage_id = storage_id(EXPERIMENT_KIND, &experiment.id)?;
        session
            .put_edge(
                ctx,
                "artifact",
                &job_storage_id,
                "artifact",
                &experiment_storage_id,
            )
            .await?;
        session.commit().await?;
        Ok(job)
    }

    pub async fn status(
        ctx: &Context,
        store: &Store,
        job_id: &str,
    ) -> Result<ReplayEconomicStatusV1> {
        let (job, _) = load_job_experiment(ctx, store, job_id).await?;
        Ok(ReplayEconomicStatusV1 {
            job_id: job.id,
            experiment_id: job.experiment_id,
            state: job.state,
            paired_ticket_id: job.paired_ticket_id,
            cost_receipt_count: job.cost_receipt_ids.len() as u32,
            report_id: job.report_id,
            blocked_reasons: job.blocked_reasons,
        })
    }

    pub async fn cancel(
        ctx: &Context,
        store: &Store,
        job_id: &str,
        reason: &str,
    ) -> Result<ReplayEconomicJobV1> {
        ctx.require(&[Role::Evaluator])?;
        text_reason(reason)?;
        let mut session = store.session().await?;
        let mut job: ReplayEconomicJobV1 = need_record(&mut session, ctx, JOB_KIND, job_id).await?;
        let experiment: ReplayEconomicExperimentV1 =
            need_record(&mut session, ctx, EXPERIMENT_KIND, &job.experiment_id).await?;
        if experiment.evaluator_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        if job.state == ReplayEconomicJobState::Cancelled {
            if job.cancel_reason.as_deref() != Some(reason) {
                return Err(Error::Conflict("cancel reason changed".into()));
            }
            session.commit().await?;
            return Ok(job);
        }
        job.state = ReplayEconomicJobState::Cancelled;
        job.cancel_reason = Some(reason.into());
        put_record(&mut session, ctx, JOB_KIND, &job.id, ctx.actor(), &job).await?;
        session.commit().await?;
        Ok(job)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_budget_cost(
        ctx: &Context,
        store: &Store,
        job_id: &str,
        billing_scope: &str,
        call_id: &str,
        component: CostComponentKind,
        scope: CostScope,
        created_seq: u64,
    ) -> Result<CostComponentReceiptV1> {
        ctx.require(&[Role::Evaluator, Role::Admin])?;
        let (job, experiment) = load_job_experiment(ctx, store, job_id).await?;
        if billing_scope != experiment.budget_binding.billing_scope {
            return Err(Error::Forbidden);
        }
        let ComponentBillingSourceV1::BudgetCall {
            dispatch_group_id,
            stage,
        } = experiment.budget_binding.component(component)?
        else {
            return Err(Error::Conflict(
                "component is registered for Admin measurement, not a budget call".into(),
            ));
        };
        let call = store
            .budget_call(ctx, billing_scope, call_id)
            .await?
            .ok_or(Error::NotFound)?;
        let root = store
            .root_budget(ctx, billing_scope)
            .await?
            .ok_or(Error::NotFound)?;
        if root.root_budget_id != experiment.budget_binding.root_budget_id
            || root.billing_scope != experiment.budget_binding.billing_scope
            || root.authorizing_namespace != ctx.namespace()
            || root.currency != experiment.budget_binding.currency
            || root.pricing_version != experiment.budget_binding.pricing_version
            || root.payment_subject != experiment.budget_binding.payment_subject
            || call.billing_scope != experiment.budget_binding.billing_scope
            || call.namespace != ctx.namespace()
            || call.dispatch_group_id != *dispatch_group_id
            || call.stage.as_str() != stage.as_str()
        {
            return Err(Error::Conflict(
                "budget call/root differs from the preregistered component binding".into(),
            ));
        }
        let source_digest = fingerprint(&call)?;
        let (measurement_state, amount, currency, pricing, payment, proof, reason) =
            match call.state {
                BudgetCallState::Finalized
                    if call.execution_closed
                        && call.actual_cost_micros.is_some()
                        && call.actual_currency.is_some()
                        && call.actual_pricing_version.is_some() =>
                {
                    (
                        MeasurementState::KnownFinal,
                        call.actual_cost_micros,
                        call.actual_currency.clone(),
                        call.actual_pricing_version.clone(),
                        Some(root.payment_subject.clone()),
                        None,
                        None,
                    )
                }
                BudgetCallState::Released | BudgetCallState::Cancelled
                    if call.dispatch_id.is_none() && call.dispatched_at.is_none() =>
                {
                    (
                        MeasurementState::NotIncurred,
                        None,
                        None,
                        None,
                        None,
                        Some(fingerprint(&(
                            call.call_id.as_str(),
                            call.state,
                            call.terminal_reason.as_deref(),
                        ))?),
                        Some("budget call was provably never dispatched".into()),
                    )
                }
                BudgetCallState::Dispatched
                | BudgetCallState::Uncertain
                | BudgetCallState::Finalized => (
                    MeasurementState::UsageUncertain,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some("dispatched usage is not finalized and execution-closed".into()),
                ),
                BudgetCallState::Reserved => (
                    MeasurementState::UnsupportedMeasurement,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some("reservation is neither released nor final usage".into()),
                ),
                _ => (
                    MeasurementState::UsageUncertain,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some("budget call state cannot prove final cost".into()),
                ),
            };
        if measurement_state == MeasurementState::KnownFinal
            && (currency.as_deref() != Some(root.currency.as_str())
                || pricing.as_deref() != Some(root.pricing_version.as_str()))
        {
            return Err(Error::Conflict(
                "finalized call pricing differs from root authorization".into(),
            ));
        }
        let receipt = CostComponentReceiptV1 {
            schema_version: COST_RECEIPT_SCHEMA.into(),
            id: format!(
                "economic-cost-{}",
                &fingerprint(&(experiment.id.as_str(), call_id))?[..24]
            ),
            experiment_id: experiment.id.clone(),
            component,
            scope,
            source_kind: CostSourceKind::BudgetCall,
            source_id: call_id.into(),
            source_digest,
            billing_scope: Some(billing_scope.into()),
            budget_call_id: Some(call_id.into()),
            amount_micros: amount,
            currency,
            pricing_version: pricing,
            payment_subject: payment,
            tokens: None,
            latency_micros: None,
            storage_bytes: None,
            cpu_nanos: None,
            human_minutes: None,
            measurement_state,
            proof_digest: proof,
            reason,
            created_seq,
        };
        persist_cost_receipt(ctx, store, &job.id, receipt).await
    }

    pub async fn register_admin_cost(
        ctx: &Context,
        store: &Store,
        job_id: &str,
        receipt: CostComponentReceiptV1,
    ) -> Result<CostComponentReceiptV1> {
        ctx.require(&[Role::Admin])?;
        if receipt.source_kind != CostSourceKind::AdminMeasurement {
            return Err(Error::Forbidden);
        }
        let (job, experiment) = load_job_experiment(ctx, store, job_id).await?;
        if receipt.experiment_id != experiment.id {
            return Err(Error::Conflict("admin cost experiment differs".into()));
        }
        let ComponentBillingSourceV1::AdminMeasurement { source_id } =
            experiment.budget_binding.component(receipt.component)?
        else {
            return Err(Error::Conflict(
                "component is registered for budget-call evidence".into(),
            ));
        };
        if receipt.source_id != *source_id
            || receipt.currency.as_deref() != Some(experiment.budget_binding.currency.as_str())
            || receipt.pricing_version.as_deref()
                != Some(experiment.budget_binding.pricing_version.as_str())
            || receipt.payment_subject.as_deref()
                != Some(experiment.budget_binding.payment_subject.as_str())
        {
            return Err(Error::Conflict(
                "Admin measurement differs from preregistered source/pricing".into(),
            ));
        }
        persist_cost_receipt(ctx, store, &job.id, receipt).await
    }

    pub async fn build_blocked_report(
        ctx: &Context,
        store: &Store,
        job_id: &str,
        projected_tasks: u64,
    ) -> Result<ReplayEconomicReportV1> {
        ctx.require(&[Role::Evaluator])?;
        let (job, experiment) = load_job_experiment(ctx, store, job_id).await?;
        if job.state == ReplayEconomicJobState::Cancelled {
            return Err(Error::Cancelled);
        }
        load_live_selection(ctx, store, &experiment).await?;
        let mut session = store.session().await?;
        verify_experiment_sources(&mut session, ctx, &experiment).await?;
        if let Some(report_id) = &job.report_id {
            let existing: ReplayEconomicReportV1 =
                need_record(&mut session, ctx, REPORT_KIND, report_id).await?;
            existing.validate_against(&experiment)?;
            session.commit().await?;
            return Ok(existing);
        }
        let mut receipts: Vec<CostComponentReceiptV1> =
            Vec::with_capacity(job.cost_receipt_ids.len());
        for receipt_id in &job.cost_receipt_ids {
            receipts.push(need_record(&mut session, ctx, COST_KIND, receipt_id).await?);
        }
        let job_snapshot_digest = fingerprint(&job)?;
        let receipts_snapshot_digest = fingerprint(&receipts)?;
        session.commit().await?;
        for receipt in &receipts {
            if receipt.source_kind == CostSourceKind::BudgetCall {
                let call_id = receipt.budget_call_id.as_deref().ok_or(Error::Internal)?;
                let billing_scope = receipt.billing_scope.as_deref().ok_or(Error::Internal)?;
                let call = store
                    .budget_call(ctx, billing_scope, call_id)
                    .await?
                    .ok_or(Error::NotFound)?;
                if fingerprint(&call)? != receipt.source_digest {
                    return Err(Error::Conflict(
                        "budget call changed after economic cost capture".into(),
                    ));
                }
            }
        }
        let mut economics = if receipts.is_empty() {
            EconomicComputation::Blocked {
                reasons: vec!["no_cost_receipts".into()],
            }
        } else {
            compute_economics(&experiment.cost_plan, &receipts, 1, projected_tasks)?
        };
        if matches!(economics, EconomicComputation::Complete { .. }) {
            economics = EconomicComputation::Blocked {
                reasons: vec!["paired_online_receipt_closure_unavailable".into()],
            };
        }
        let terminal = if matches!(economics, EconomicComputation::UsageUncertain { .. }) {
            EconomicReportTerminal::UsageUncertain
        } else {
            EconomicReportTerminal::BlockedSupport
        };
        let report = ReplayEconomicReportV1 {
            schema_version: REPORT_SCHEMA.into(),
            id: format!("replay-economic-report-{}", &fingerprint(&job)?[..24]),
            experiment_id: experiment.id.clone(),
            experiment_digest: experiment.digest()?,
            paired_ticket_id: experiment.paired_ticket_id.clone(),
            paired_ticket_digest: experiment.paired_ticket_digest.clone(),
            paired_receipt_closure_digest: None,
            completed_paired_tasks: 0,
            paired_units: vec![],
            latency: None,
            quality_outcome: PairedQualityOutcome::ZeroOrInconclusive,
            cost_receipt_ids: job.cost_receipt_ids.clone(),
            economics,
            terminal,
            reasons: job.blocked_reasons.clone(),
        };
        report.validate_against(&experiment)?;
        let mut session = store.session().await?;
        let mut current_job: ReplayEconomicJobV1 =
            need_record(&mut session, ctx, JOB_KIND, &job.id).await?;
        let current_experiment: ReplayEconomicExperimentV1 = need_record(
            &mut session,
            ctx,
            EXPERIMENT_KIND,
            &current_job.experiment_id,
        )
        .await?;
        if current_experiment.evaluator_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        if current_job.state == ReplayEconomicJobState::Cancelled {
            return Err(Error::Cancelled);
        }
        verify_experiment_sources(&mut session, ctx, &current_experiment).await?;
        let mut current_receipts = Vec::with_capacity(current_job.cost_receipt_ids.len());
        for receipt_id in &current_job.cost_receipt_ids {
            current_receipts.push(
                need_record::<CostComponentReceiptV1>(&mut session, ctx, COST_KIND, receipt_id)
                    .await?,
            );
        }
        if fingerprint(&current_experiment)? != fingerprint(&experiment)?
            || fingerprint(&current_job)? != job_snapshot_digest
            || fingerprint(&current_receipts)? != receipts_snapshot_digest
        {
            return Err(Error::Conflict(
                "economic job/cost snapshot changed while report was being built".into(),
            ));
        }
        if let Some(existing) =
            get_record::<ReplayEconomicReportV1>(&mut session, ctx, REPORT_KIND, &report.id).await?
        {
            if fingerprint(&existing)? != fingerprint(&report)? {
                return Err(Error::Conflict(
                    "immutable economic report id has different content".into(),
                ));
            }
        } else {
            put_record(
                &mut session,
                ctx,
                REPORT_KIND,
                &report.id,
                ctx.actor(),
                &report,
            )
            .await?;
        }
        current_job.report_id = Some(report.id.clone());
        put_record(
            &mut session,
            ctx,
            JOB_KIND,
            &current_job.id,
            ctx.actor(),
            &current_job,
        )
        .await?;
        session.commit().await?;
        Ok(report)
    }
}

async fn persist_cost_receipt(
    ctx: &Context,
    store: &Store,
    job_id: &str,
    receipt: CostComponentReceiptV1,
) -> Result<CostComponentReceiptV1> {
    receipt.validate()?;
    let mut session = store.session().await?;
    let mut job: ReplayEconomicJobV1 = need_record(&mut session, ctx, JOB_KIND, job_id).await?;
    let experiment: ReplayEconomicExperimentV1 =
        need_record(&mut session, ctx, EXPERIMENT_KIND, &job.experiment_id).await?;
    if ctx.role() == Role::Evaluator && experiment.evaluator_actor != ctx.actor() {
        return Err(Error::Forbidden);
    }
    for existing_id in &job.cost_receipt_ids {
        let indexed: CostComponentReceiptV1 =
            need_record(&mut session, ctx, COST_KIND, existing_id).await?;
        if indexed.source_kind == receipt.source_kind
            && indexed.source_id == receipt.source_id
            && indexed.id != receipt.id
        {
            return Err(Error::Conflict(
                "one audited cost source cannot use multiple receipt ids".into(),
            ));
        }
    }
    if let Some(existing) =
        get_record::<CostComponentReceiptV1>(&mut session, ctx, COST_KIND, &receipt.id).await?
    {
        if existing.experiment_id != receipt.experiment_id
            || existing.component != receipt.component
            || existing.scope != receipt.scope
            || existing.source_kind != receipt.source_kind
            || existing.source_id != receipt.source_id
            || existing.billing_scope != receipt.billing_scope
            || existing.budget_call_id != receipt.budget_call_id
            || existing.created_seq != receipt.created_seq
        {
            return Err(Error::Conflict("cost receipt idempotency conflict".into()));
        }
        if fingerprint(&existing)? == fingerprint(&receipt)? {
            session.commit().await?;
            return Ok(existing);
        }
        if existing.source_kind == CostSourceKind::AdminMeasurement
            || existing.measurement_state == MeasurementState::KnownFinal
            || !(existing.measurement_state == MeasurementState::UsageUncertain
                && receipt.measurement_state == MeasurementState::KnownFinal)
        {
            return Err(Error::Conflict(
                "final/Admin cost receipt is immutable; only budget uncertain-to-final may refresh"
                    .into(),
            ));
        }
        put_record(
            &mut session,
            ctx,
            COST_KIND,
            &receipt.id,
            ctx.actor(),
            &receipt,
        )
        .await?;
        session
            .audit(ctx, "replay_economic.cost_refresh", &receipt.id)
            .await?;
        put_record(&mut session, ctx, JOB_KIND, &job.id, ctx.actor(), &job).await?;
        session.commit().await?;
        return Ok(receipt);
    }
    if job.cost_receipt_ids.iter().any(|id| id == &receipt.id) {
        return Err(Error::Conflict("job cost receipt index differs".into()));
    }
    job.cost_receipt_ids.push(receipt.id.clone());
    job.cost_receipt_ids.sort();
    if receipt.measurement_state == MeasurementState::UsageUncertain
        && job.state != ReplayEconomicJobState::Cancelled
    {
        job.state = ReplayEconomicJobState::UsageUncertain;
    }
    put_record(
        &mut session,
        ctx,
        COST_KIND,
        &receipt.id,
        ctx.actor(),
        &receipt,
    )
    .await?;
    session
        .audit(ctx, "replay_economic.cost_capture", &receipt.id)
        .await?;
    if receipt.source_kind == CostSourceKind::AdminMeasurement {
        session
            .audit(ctx, "replay_economic.admin_measurement", &receipt.id)
            .await?;
    }
    put_record(&mut session, ctx, JOB_KIND, &job.id, ctx.actor(), &job).await?;
    session.commit().await?;
    Ok(receipt)
}

async fn load_job_experiment(
    ctx: &Context,
    store: &Store,
    job_id: &str,
) -> Result<(ReplayEconomicJobV1, ReplayEconomicExperimentV1)> {
    ctx.require(&[Role::Evaluator, Role::Admin])?;
    let mut session = store.session().await?;
    let job: ReplayEconomicJobV1 = need_record(&mut session, ctx, JOB_KIND, job_id).await?;
    let experiment: ReplayEconomicExperimentV1 =
        need_record(&mut session, ctx, EXPERIMENT_KIND, &job.experiment_id).await?;
    if ctx.role() == Role::Evaluator && experiment.evaluator_actor != ctx.actor() {
        return Err(Error::Forbidden);
    }
    session.commit().await?;
    Ok((job, experiment))
}

async fn verify_experiment_sources(
    session: &mut Session,
    ctx: &Context,
    experiment: &ReplayEconomicExperimentV1,
) -> Result<()> {
    let current = session
        .watermark(ctx)
        .await?
        .and_then(|(sequence, _)| u64::try_from(sequence).ok())
        .ok_or_else(|| Error::Conflict("missing economic source watermark".into()))?;
    if current != experiment.source_watermark {
        return Err(Error::Conflict("economic source watermark changed".into()));
    }
    for source in &experiment.sources {
        let value = session.raw_object(ctx, "artifact", &source.id).await?;
        if fingerprint(&value)? != source.digest {
            return Err(Error::Conflict(
                "economic source artifact digest changed".into(),
            ));
        }
    }
    Ok(())
}

async fn load_live_selection(
    ctx: &Context,
    store: &Store,
    experiment: &ReplayEconomicExperimentV1,
) -> Result<VerifiedReplayReportView> {
    let selection =
        verified_replay_report_view(ctx, store, &experiment.replay_selection.report_artifact_id)
            .await?;
    let pool = evo_storage::replay::load_live_replay_pool(
        ctx,
        store,
        &experiment.replay_selection.world_pool_digest,
    )
    .await?;
    if selection.report_digest != experiment.replay_selection.report_digest
        || selection.semantic_digest != experiment.replay_selection.semantic_digest
        || selection.pool_digest != experiment.replay_selection.world_pool_digest
        || selection.policy_digest != experiment.replay_selection.policy_digest
        || selection.profile_digest != experiment.replay_selection.profile_digest
        || selection.caps_digest != experiment.replay_selection.caps_digest
        || selection.partition != WorldPartition::Select
        || !selection.support_complete
        || !selection.development_only
        || selection
            .ineligibility_reasons
            .iter()
            .any(|reason| reason == "simulation_only_width")
        || pool.manifest.compatibility.generation_signature
            != experiment.runtime.generation_strategy_digest
        || pool.manifest.compatibility.model_digest != experiment.runtime.model_digest
        || pool.manifest.compatibility.tools_digest != experiment.runtime.tools_digest
        || pool.manifest.compatibility.scorer_digest != experiment.runtime.grader_digest
        || pool.manifest.compatibility.guidance_digest != experiment.runtime.guidance_digest
        || pool.manifest.compatibility.revoke_watermark != experiment.source_watermark
        || selection.reports.iter().any(|report| {
            report.objective_version != OBJECTIVE_V2
                || !report.development_only
                || !report.coverage.complete_support
                || !matches!(
                    report.terminal,
                    ReplayTerminal::PolicyStop | ReplayTerminal::BudgetExhausted
                )
        })
    {
        return Err(Error::Conflict(
            "live E10 selection report differs or lacks complete support".into(),
        ));
    }
    Ok(selection)
}

fn text_reason(reason: &str) -> Result<()> {
    if reason.is_empty() || reason.len() > 512 {
        return Err(Error::Invalid("cancel reason must be 1..=512 bytes".into()));
    }
    Ok(())
}

fn storage_id(record_kind: &str, id: &str) -> Result<String> {
    identifier(record_kind)?;
    identifier(id)?;
    Ok(format!("e11-{}", fingerprint(&(record_kind, id))?))
}

async fn get_record<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
) -> Result<Option<T>> {
    let storage_id = storage_id(record_kind, id)?;
    let envelope = session
        .get::<ArtifactEnvelope<T>>(ctx, "artifact", &storage_id)
        .await?;
    match envelope {
        Some(envelope)
            if envelope.schema_version == ENVELOPE_SCHEMA
                && envelope.id == storage_id
                && envelope.record_kind == record_kind =>
        {
            Ok(Some(envelope.payload))
        }
        Some(_) => Err(Error::Conflict(
            "economic artifact envelope mismatch".into(),
        )),
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
    session
        .put(
            ctx,
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
    use crate::replay::ReplayReport;

    fn cost() -> PairCost {
        PairCost {
            history: 1,
            generate: 1,
            replay: 1,
            accept: 1,
            review: 1,
        }
    }

    #[test]
    fn replay_win_online_regression_does_not_publish() {
        assert!(!decide_publish(true, Verdict::Regressed, cost()).unwrap());
    }

    #[test]
    fn zero_cost_is_invalid() {
        assert!(
            decide_publish(
                true,
                Verdict::Improved,
                PairCost {
                    history: 0,
                    generate: 0,
                    replay: 0,
                    accept: 0,
                    review: 0
                }
            )
            .is_err()
        );
    }

    #[test]
    fn replay_report_is_not_online() {
        let r = ReplayReport {
            world_id: "w".into(),
            policy: "p".into(),
            seed: "s".into(),
            action: "a".into(),
            oos: false,
            censored: false,
        };
        assert!(replay_report_is_not_online_evidence(&r));
    }
}
