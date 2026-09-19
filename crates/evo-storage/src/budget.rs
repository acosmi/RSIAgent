//! Persistent root monetary budget and per-call accounting.
//!
//! Monetary values are integer micros in the root's authorized currency/pricing
//! version. No floating-point conversion occurs here.

use super::{Session, Store, internal};
use evo_core::evaluation::OptimizationStage;
use evo_core::{Context, Error, Result, Role, hash, identifier, text};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Row, Sqlite, Transaction};
use std::collections::BTreeSet;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetArtifact {
    pub schema_version: String,
    pub digest: String,
    pub body: String,
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
        identifier(&self.schema_version)?;
        validate_digest(&self.digest, "artifact digest")?;
        if self.body.len() > 4 * 1024 * 1024 {
            return Err(Error::Invalid("budget artifact exceeds 4 MiB".into()));
        }
        if hash(self.body.as_bytes()) != self.digest {
            return Err(Error::Conflict("artifact digest mismatch".into()));
        }
        serde_json::from_str::<serde_json::Value>(&self.body).map_err(internal)?;
        Ok(())
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

impl Store {
    pub async fn authorize_root_budget(
        &self,
        ctx: &Context,
        authorization: &RootBudgetAuthorization,
    ) -> Result<RootBudgetRecord> {
        ctx.require(&[Role::Admin])?;
        let namespaces = validate_authorization(ctx, authorization)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        if let Some(row) = sqlx::query("SELECT * FROM root_budgets WHERE billing_scope=?")
            .bind(&authorization.billing_scope)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?
        {
            let existing_namespaces =
                load_namespaces(&mut tx, &authorization.billing_scope).await?;
            let existing = root_from_row(&row, existing_namespaces)?;
            if existing.root_budget_id != authorization.root_budget_id
                || existing.authorizing_namespace != ctx.namespace()
                || existing.allowed_namespaces != namespaces
                || existing.currency != authorization.currency
                || existing.pricing_version != authorization.pricing_version
                || existing.payment_subject != authorization.payment_subject
                || existing.authorization_receipt_digest
                    != authorization.authorization_receipt_digest
                || existing.per_call_cap_micros != authorization.per_call_cap_micros
                || existing.total_limit_micros != authorization.total_limit_micros
            {
                return Err(Error::Conflict(
                    "billing_scope_already_bound_to_different_root_or_authorization".into(),
                ));
            }
            tx.commit().await.map_err(internal)?;
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
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        for namespace in &namespaces {
            sqlx::query("INSERT INTO root_budget_namespaces(billing_scope,namespace) VALUES(?,?)")
                .bind(&authorization.billing_scope)
                .bind(namespace)
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
        }
        insert_event(
            &mut tx,
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
        tx.commit().await.map_err(internal)?;
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
        let Some(row) = sqlx::query("SELECT * FROM root_budgets WHERE billing_scope=?")
            .bind(billing_scope)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?
        else {
            return Ok(None);
        };
        let namespaces: Vec<String> = sqlx::query_scalar(
            "SELECT namespace FROM root_budget_namespaces WHERE billing_scope=? ORDER BY namespace",
        )
        .bind(billing_scope)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        if !namespaces.iter().any(|value| value == ctx.namespace()) {
            return Err(Error::NotFound);
        }
        Ok(Some(root_from_row(&row, namespaces)?))
    }

    pub async fn reserve_budget_call(
        &self,
        ctx: &Context,
        request: &BudgetCallReservation,
    ) -> Result<BudgetCallRecord> {
        require_budget_caller(ctx)?;
        validate_reservation(request)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        if let Some(existing) = load_call(&mut tx, &request.billing_scope, &request.call_id).await?
        {
            ensure_same_reservation(ctx, &existing, request)?;
            tx.commit().await.map_err(internal)?;
            return Ok(existing);
        }
        let row = sqlx::query("SELECT * FROM root_budgets WHERE billing_scope=?")
            .bind(&request.billing_scope)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?
            .ok_or(Error::Budget)?;
        require_namespace(&mut tx, &request.billing_scope, ctx.namespace()).await?;
        if row.try_get::<i64, _>("stopped").map_err(internal)? != 0 {
            return Err(Error::Cancelled);
        }
        let per_call_cap: i64 = row.try_get("per_call_cap_micros").map_err(internal)?;
        let total_limit: i64 = row.try_get("total_limit_micros").map_err(internal)?;
        let spent: i64 = row.try_get("spent_micros").map_err(internal)?;
        let reserved: i64 = row.try_get("reserved_micros").map_err(internal)?;
        if request.max_cost_micros > per_call_cap {
            return Err(Error::Budget);
        }
        let committed = spent
            .checked_add(reserved)
            .and_then(|value| value.checked_add(request.max_cost_micros))
            .ok_or(Error::Budget)?;
        if committed > total_limit {
            return Err(Error::Budget);
        }
        match load_dispatch_group(&mut tx, &request.billing_scope, &request.dispatch_group_id)
            .await?
        {
            Some(group) => {
                if group.owner_namespace != ctx.namespace() {
                    return Err(Error::Conflict(
                        "dispatch_group_owned_by_different_namespace".into(),
                    ));
                }
                if group.stopped {
                    return Err(Error::Cancelled);
                }
            }
            None => {
                sqlx::query(
                    "INSERT INTO root_budget_dispatch_groups(
                       billing_scope,dispatch_group_id,owner_namespace,created_at
                     ) VALUES(?,?,?,?)",
                )
                .bind(&request.billing_scope)
                .bind(&request.dispatch_group_id)
                .bind(ctx.namespace())
                .bind(request.now)
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
                insert_event(
                    &mut tx,
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
        sqlx::query("UPDATE root_budgets SET reserved_micros=? WHERE billing_scope=?")
            .bind(
                reserved
                    .checked_add(request.max_cost_micros)
                    .ok_or(Error::Budget)?,
            )
            .bind(&request.billing_scope)
            .execute(&mut *tx)
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
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        insert_event(
            &mut tx,
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
        let call = load_call(&mut tx, &request.billing_scope, &request.call_id)
            .await?
            .ok_or(Error::Internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(call)
    }

    pub async fn refence_reserved_budget_call(
        &self,
        ctx: &Context,
        request: &BudgetCallRefence,
    ) -> Result<BudgetCallRecord> {
        require_budget_caller(ctx)?;
        identifier(&request.billing_scope)?;
        identifier(&request.call_id)?;
        identifier(&request.new_lease_token)?;
        valid_time(request.now)?;
        if request.new_lease_until <= request.now {
            return Err(Error::Invalid("new lease must expire after now".into()));
        }
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = load_call(&mut tx, &request.billing_scope, &request.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        require_call_access(&mut tx, ctx, &call).await?;
        if call.state != BudgetCallState::Reserved
            || call.lease_epoch != request.expected_epoch
            || request.now <= call.lease_until
        {
            return Err(Error::Cancelled);
        }
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
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        insert_event(
            &mut tx,
            &request.billing_scope,
            Some(&request.call_id),
            "refenced",
            request.now,
            json!({"lease_epoch": next_epoch, "lease_until": request.new_lease_until}),
        )
        .await?;
        let updated = load_call(&mut tx, &request.billing_scope, &request.call_id)
            .await?
            .ok_or(Error::Internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(updated)
    }

    pub async fn begin_budget_dispatch(
        &self,
        ctx: &Context,
        fence: &BudgetCallFence,
    ) -> Result<BudgetDispatchDecision> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let decision = begin_budget_dispatch_in_tx(&mut tx, ctx, fence).await?;
        tx.commit().await.map_err(internal)?;
        Ok(decision)
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

    /// Atomically accounts a model charge and stores the exact request/transport/response facts.
    /// Stop and lease state are re-read in this transaction after the provider await.
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
            ensure_same_model_settlement(&call, evidence)?;
            let settlement = settlement_from_call(call, false)?;
            tx.commit().await.map_err(internal)?;
            return Ok(settlement);
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
        let preexisting_block = if fence.now > call.lease_until {
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
        sqlx::query(
            "UPDATE root_budget_calls SET
               execution_provenance=?,actual_model_digest=?,
               transport_artifact_schema=?,transport_artifact_digest=?,transport_artifact_body=?,
               response_artifact_schema=?,response_artifact_digest=?,response_artifact_body=?,
               response_usable=?,response_block_reason=?
             WHERE billing_scope=? AND call_id=?",
        )
        .bind(evidence.provenance.as_str())
        .bind(&evidence.actual_model_digest)
        .bind(&evidence.transport_artifact.schema_version)
        .bind(&evidence.transport_artifact.digest)
        .bind(&evidence.transport_artifact.body)
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
        insert_event(
            &mut tx,
            &call.billing_scope,
            Some(&call.call_id),
            "model_response_persisted",
            fence.now,
            json!({
                "request_artifact_digest": call.request_artifact.as_ref().map(|value| &value.digest),
                "transport_artifact_digest": evidence.transport_artifact.digest,
                "response_artifact_digest": response.digest,
                "response_usable": response_usable,
                "response_block_reason": block_reason,
                "execution_provenance": evidence.provenance,
                "actual_model_digest": evidence.actual_model_digest,
            }),
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
        ctx.require(&[Role::Admin])?;
        identifier(billing_scope)?;
        identifier(call_id)?;
        valid_time(now)?;
        validate_charge(charge)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let call = load_call(&mut tx, billing_scope, call_id)
            .await?
            .ok_or(Error::NotFound)?;
        require_call_access(&mut tx, ctx, &call).await?;
        if call.state == BudgetCallState::Finalized {
            ensure_same_charge(&call, charge)?;
            tx.commit().await.map_err(internal)?;
            return Ok(call);
        }
        if !matches!(
            call.state,
            BudgetCallState::Dispatched | BudgetCallState::Uncertain
        ) {
            return Err(Error::Conflict(
                "reconcile_requires_dispatched_or_uncertain_call".into(),
            ));
        }
        let accounting_overflow =
            apply_charge(&mut tx, &call, charge, now, "cost_reconciled").await?;
        let updated = load_call(&mut tx, billing_scope, call_id)
            .await?
            .ok_or(Error::Internal)?;
        tx.commit().await.map_err(internal)?;
        if accounting_overflow {
            return Err(Error::Conflict("cost_accounting_overflow".into()));
        }
        Ok(updated)
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
        require_namespace(&mut tx, billing_scope, ctx.namespace()).await?;
        let row = sqlx::query("SELECT stopped,stop_reason FROM root_budgets WHERE billing_scope=?")
            .bind(billing_scope)
            .fetch_optional(&mut *tx)
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
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            insert_event(
                &mut tx,
                billing_scope,
                None,
                "root_stopped",
                now,
                json!({"reason": reason}),
            )
            .await?;
        }
        tx.commit().await.map_err(internal)?;
        self.root_budget(ctx, billing_scope)
            .await?
            .ok_or(Error::Internal)
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

    pub async fn begin_budget_dispatch(
        &mut self,
        ctx: &Context,
        fence: &BudgetCallFence,
    ) -> Result<BudgetDispatchDecision> {
        begin_budget_dispatch_in_tx(&mut self.tx, ctx, fence).await
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

async fn budget_call_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    billing_scope: &str,
    call_id: &str,
) -> Result<Option<BudgetCallRecord>> {
    require_budget_reader(ctx)?;
    identifier(billing_scope)?;
    identifier(call_id)?;
    let call = load_call(tx, billing_scope, call_id).await?;
    if let Some(call) = &call {
        require_call_access(tx, ctx, call).await?;
    }
    Ok(call)
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

async fn begin_budget_dispatch_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    fence: &BudgetCallFence,
) -> Result<BudgetDispatchDecision> {
    require_budget_caller(ctx)?;
    validate_fence(fence)?;
    let call = load_call(tx, &fence.billing_scope, &fence.call_id)
        .await?
        .ok_or(Error::NotFound)?;
    require_call_access(tx, ctx, &call).await?;
    fence_call(&call, fence, false)?;
    if call.state != BudgetCallState::Reserved {
        return Ok(BudgetDispatchDecision {
            call,
            new_dispatch: false,
        });
    }
    if fence.now > call.lease_until {
        return Err(Error::Cancelled);
    }
    let stopped: i64 = sqlx::query_scalar("SELECT stopped FROM root_budgets WHERE billing_scope=?")
        .bind(&fence.billing_scope)
        .fetch_one(&mut **tx)
        .await
        .map_err(internal)?;
    if stopped != 0 {
        return Err(Error::Cancelled);
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
        return Err(Error::Cancelled);
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
        return Err(Error::Conflict("root_budget_concurrency_limit".into()));
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
    Ok(BudgetDispatchDecision {
        call,
        new_dispatch: true,
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

fn validate_reservation(value: &BudgetCallReservation) -> Result<()> {
    identifier(&value.billing_scope)?;
    identifier(&value.call_id)?;
    identifier(&value.dispatch_group_id)?;
    validate_digest(&value.actual_input_digest, "actual_input_digest")?;
    identifier(&value.lease_token)?;
    if let Some(artifact) = &value.request_artifact {
        artifact.validate()?;
    }
    valid_time(value.now)?;
    if value.max_cost_micros <= 0 {
        return Err(Error::Invalid("reservation must be positive".into()));
    }
    if value.lease_until <= value.now {
        return Err(Error::Invalid("lease must expire after reservation".into()));
    }
    Ok(())
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

fn ensure_same_reservation(
    ctx: &Context,
    call: &BudgetCallRecord,
    request: &BudgetCallReservation,
) -> Result<()> {
    if call.namespace != ctx.namespace()
        || call.dispatch_group_id != request.dispatch_group_id
        || call.stage != request.stage
        || call.actual_input_digest != request.actual_input_digest
        || call.request_artifact != request.request_artifact
        || call.reserved_micros != request.max_cost_micros
    {
        return Err(Error::Conflict(
            "call_id_reused_with_different_content".into(),
        ));
    }
    Ok(())
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
    if call.execution_provenance != Some(evidence.provenance)
        || call.actual_model_digest != evidence.actual_model_digest
        || call.transport_artifact.as_ref() != Some(&evidence.transport_artifact)
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
    let allowed: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM root_budget_namespaces WHERE billing_scope=? AND namespace=?",
    )
    .bind(billing_scope)
    .bind(namespace)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    if allowed.is_none() {
        return Err(Error::Forbidden);
    }
    Ok(())
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

async fn load_call(
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
