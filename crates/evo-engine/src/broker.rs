//! Persistent budget gate around the shared model port.
//!
//! No provider protocol or endpoint is implemented here. External calls require
//! a separately authorized `ModelTransport`; the default transport is disabled.

use crate::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelRejectionKind, ModelResponse,
    RejectedDispatch,
};
use async_trait::async_trait;
use evo_core::optimization::{ModelRequest, ModelStage};
use evo_core::{Context, Error, Result, Role, hash, identifier, text};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallRefence, BudgetCallReservation,
    BudgetCallState, BudgetDispatchDecision, BudgetExecutionProvenance, BudgetPreDispatchRefusal,
    BudgetStage, ModelCallSettlementEvidence, RootBudgetRecord, UsageCharge,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const MODEL_REQUEST_ARTIFACT_SCHEMA: &str = "rsia.model_request.artifact.v1";
const MODEL_TRANSPORT_ARTIFACT_SCHEMA: &str = "rsia.model_transport.artifact.v1";
const MODEL_RESPONSE_ARTIFACT_SCHEMA: &str = "rsia.model_response.artifact.v1";
const PRE_DISPATCH_REASON_PREFIX: &str = "broker_pre_dispatch_v1.";

#[derive(Debug, Clone)]
pub struct BrokerConfig {
    pub billing_scope: String,
    pub actor: String,
    pub currency: String,
    pub pricing_version: String,
    pub max_cost_micros: i64,
    pub lease_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetPortBinding {
    pub billing_scope: String,
    pub root_budget_id: String,
}

impl BrokerConfig {
    fn validate(&self) -> Result<()> {
        identifier(&self.billing_scope)?;
        identifier(&self.actor)?;
        identifier(&self.currency)?;
        identifier(&self.pricing_version)?;
        if self.max_cost_micros <= 0 || self.lease_seconds <= 0 {
            return Err(Error::Invalid(
                "broker cost cap and lease duration must be positive".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportCompletion {
    pub response_id: String,
    pub provider_request_id: String,
    pub usage_record_id: String,
    pub actual_model_digest: String,
    pub output: String,
    pub actual_cost_micros: i64,
    pub currency: String,
    pub pricing_version: String,
}

impl TransportCompletion {
    fn validate(&self) -> Result<()> {
        identifier(&self.response_id)?;
        identifier(&self.provider_request_id)?;
        identifier(&self.usage_record_id)?;
        identifier(&self.actual_model_digest)?;
        text(&self.output, "model output", 256 * 1024)?;
        identifier(&self.currency)?;
        identifier(&self.pricing_version)?;
        if self.actual_cost_micros < 0 {
            return Err(Error::Invalid(
                "actual model cost must be nonnegative".into(),
            ));
        }
        Ok(())
    }

    fn validate_accounting(&self) -> Result<()> {
        identifier(&self.provider_request_id)?;
        identifier(&self.usage_record_id)?;
        identifier(&self.currency)?;
        identifier(&self.pricing_version)?;
        if self.actual_cost_micros < 0 {
            return Err(Error::Invalid(
                "actual model cost must be nonnegative".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct InvalidTransportMetadata<'a> {
    schema_version: &'static str,
    provider_request_id: &'a str,
    usage_record_id: &'a str,
    actual_cost_micros: i64,
    currency: &'a str,
    pricing_version: &'a str,
    response_id_bytes: usize,
    response_id_digest: String,
    actual_model_value_bytes: usize,
    actual_model_value_digest: String,
    output_bytes: usize,
    output_digest: String,
    validation_error: String,
}

#[async_trait]
pub trait ModelTransport: Send + Sync {
    /// `None` means no external or fixture dispatch is authorized.
    fn provenance(&self) -> Option<ModelExecutionProvenance>;

    async fn execute(
        &self,
        request: &ModelRequest,
        dispatch_id: &str,
    ) -> Result<TransportCompletion>;
}

#[derive(Debug, Default)]
pub struct DisabledModelTransport;

#[async_trait]
impl ModelTransport for DisabledModelTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        None
    }

    async fn execute(
        &self,
        _request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        Err(Error::Forbidden)
    }
}

pub struct PersistentModelBroker<T> {
    store: Store,
    transport: T,
    config: BrokerConfig,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl PersistentModelBroker<DisabledModelTransport> {
    pub fn disabled(store: Store, config: BrokerConfig) -> Result<Self> {
        Self::new(store, DisabledModelTransport, config)
    }
}

impl<T: ModelTransport> PersistentModelBroker<T> {
    pub fn new(store: Store, transport: T, config: BrokerConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            store,
            transport,
            config,
            clock: Arc::new(evo_core::now),
        })
    }

    pub fn with_clock(
        store: Store,
        transport: T,
        config: BrokerConfig,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            store,
            transport,
            config,
            clock,
        })
    }

    /// Returns the persistent billing root this trusted broker will use before
    /// any model dispatch. This is configuration/accounting identity only.
    pub async fn budget_binding(&self, namespace: &str) -> Result<BudgetPortBinding> {
        identifier(namespace)?;
        let root = self
            .store
            .root_budget(&self.broker_context(namespace)?, &self.config.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        Ok(BudgetPortBinding {
            billing_scope: self.config.billing_scope.clone(),
            root_budget_id: root.root_budget_id,
        })
    }

    pub async fn verify_receipt(
        &self,
        request: &ModelRequest,
        receipt: &ModelExecutionReceipt,
    ) -> Result<()> {
        request.validate()?;
        receipt.validate()?;
        let ctx = self.broker_context(&request.namespace)?;
        let root = self
            .store
            .root_budget(&ctx, &self.config.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        let call = self
            .store
            .budget_call(&ctx, &self.config.billing_scope, &receipt.call_id)
            .await?
            .ok_or(Error::NotFound)?;
        if root.root_budget_id != receipt.root_budget_id
            || call.call_id != request.request_id
            || call.actual_input_digest != request.cache_key_digest
            || !matches!(
                call.state,
                BudgetCallState::Finalized | BudgetCallState::Uncertain
            )
            || !call.execution_closed
            || call.dispatch_id.as_deref() != Some(receipt.dispatch_id.as_str())
            || call.provider_request_id.as_deref() != Some(receipt.provider_request_id.as_str())
            || call.usage_record_id.as_deref() != Some(receipt.usage_record_id.as_str())
            || call.output_digest.is_none()
            || call.execution_provenance != Some(storage_provenance(receipt.provenance))
            || call.response_artifact.is_none()
        {
            return Err(Error::Conflict(
                "model execution receipt does not match persistent broker facts".into(),
            ));
        }
        Ok(())
    }

    async fn verify_persisted_response(
        &self,
        request: &ModelRequest,
        response: &ModelResponse,
    ) -> Result<()> {
        let receipt = match response {
            ModelResponse::Completed {
                execution_receipt, ..
            } => Some(execution_receipt),
            ModelResponse::Rejected {
                dispatch: RejectedDispatch::Dispatched { receipt },
                ..
            } => Some(receipt),
            ModelResponse::Rejected {
                dispatch: RejectedDispatch::NotDispatched,
                ..
            }
            | ModelResponse::Uncertain { .. } => None,
        };
        if let Some(receipt) = receipt {
            self.verify_receipt(request, receipt).await?;
            if let ModelResponse::Completed {
                actual_model_digest,
                output_digest,
                ..
            } = response
            {
                let call = self
                    .store
                    .budget_call(
                        &self.broker_context(&request.namespace)?,
                        &self.config.billing_scope,
                        &receipt.call_id,
                    )
                    .await?
                    .ok_or(Error::NotFound)?;
                if call.actual_model_digest.as_deref() != Some(actual_model_digest.as_str())
                    || call.output_digest.as_deref() != Some(output_digest.as_str())
                    || call.state != BudgetCallState::Finalized
                    || call.response_usable != Some(true)
                {
                    return Err(Error::Conflict(
                        "persisted model response differs from charged execution facts".into(),
                    ));
                }
            } else if matches!(
                response,
                ModelResponse::Rejected {
                    dispatch: RejectedDispatch::Dispatched { .. },
                    ..
                }
            ) {
                let call = self
                    .store
                    .budget_call(
                        &self.broker_context(&request.namespace)?,
                        &self.config.billing_scope,
                        &receipt.call_id,
                    )
                    .await?
                    .ok_or(Error::NotFound)?;
                if call.response_usable != Some(false) {
                    return Err(Error::Conflict(
                        "blocked model response lacks a persistent dispatch block".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn broker_context(&self, namespace: &str) -> Result<Context> {
        Context::new(namespace, self.config.actor.clone(), Role::Host)
    }

    fn now(&self) -> Result<i64> {
        let now = (self.clock)();
        if now < 0 {
            return Err(Error::Invalid("broker clock returned negative time".into()));
        }
        Ok(now)
    }

    async fn existing_response(
        &self,
        request: &ModelRequest,
        root: &RootBudgetRecord,
        call: BudgetCallRecord,
    ) -> Result<ModelResponse> {
        if let Some(artifact) = call.response_artifact.clone() {
            if artifact.schema_version == "rsia.redacted.v1" {
                if let (
                    Some(dispatch_id),
                    Some(provider_request_id),
                    Some(usage_record_id),
                    Some(provenance),
                ) = (
                    call.dispatch_id.clone(),
                    call.provider_request_id.clone(),
                    call.usage_record_id.clone(),
                    call.execution_provenance,
                ) {
                    return Ok(ModelResponse::Rejected {
                        request_id: request.request_id.clone(),
                        kind: ModelRejectionKind::CancelledAfterDispatch,
                        reason: "model response content was revoked and cannot be reused".into(),
                        dispatch: RejectedDispatch::Dispatched {
                            receipt: ModelExecutionReceipt {
                                call_id: call.call_id.clone(),
                                dispatch_id,
                                root_budget_id: root.root_budget_id.clone(),
                                provider_request_id,
                                usage_record_id,
                                provenance: engine_provenance(provenance),
                            },
                        },
                    });
                }
                return Ok(ModelResponse::Uncertain {
                    request_id: request.request_id.clone(),
                    dispatch_id: call.dispatch_id.clone().ok_or(Error::Internal)?,
                    root_budget_id: root.root_budget_id.clone(),
                    provider_request_id: call.provider_request_id,
                    usage_record_id: call.usage_record_id,
                });
            }
            if !call.execution_closed {
                let dispatch_id = call.dispatch_id.as_deref().ok_or(Error::Internal)?;
                self.store
                    .close_budget_call_execution(
                        &self.broker_context(&request.namespace)?,
                        &call.billing_scope,
                        &call.call_id,
                        dispatch_id,
                        "recovered_persisted_model_response",
                        self.now()?,
                    )
                    .await?;
            }
            let response: ModelResponse =
                serde_json::from_str(&artifact.body).map_err(|_| Error::Internal)?;
            response.validate_against(request)?;
            self.verify_persisted_response(request, &response).await?;
            return Ok(response);
        }
        let dispatch_id = call.dispatch_id.clone().unwrap_or_default();
        match call.state {
            BudgetCallState::Dispatched | BudgetCallState::Uncertain => {
                Ok(ModelResponse::Uncertain {
                    request_id: request.request_id.clone(),
                    dispatch_id,
                    root_budget_id: root.root_budget_id.clone(),
                    provider_request_id: call.provider_request_id,
                    usage_record_id: call.usage_record_id,
                })
            }
            BudgetCallState::Finalized => Err(Error::Conflict(
                "finalized model call is missing its response artifact".into(),
            )),
            BudgetCallState::Released | BudgetCallState::Cancelled => {
                Ok(undispatched_terminal_response(request, &call))
            }
            BudgetCallState::Reserved => Err(Error::Internal),
        }
    }

    async fn release_pre_dispatch(
        &self,
        ctx: &Context,
        request: &ModelRequest,
        root: &RootBudgetRecord,
        fence: &BudgetCallFence,
        code: &str,
    ) -> Result<ModelResponse> {
        let released = self
            .store
            .release_undispatched_budget_call(
                ctx,
                fence,
                &format!("{PRE_DISPATCH_REASON_PREFIX}{code}"),
            )
            .await?;
        // Another valid release may already have won. Replay exactly its stored
        // reason instead of replacing it with this attempted release's diagnosis.
        self.existing_response(request, root, released).await
    }

    async fn begin_pre_dispatch(
        &self,
        ctx: &Context,
        request: &ModelRequest,
        root: &RootBudgetRecord,
        fence: &BudgetCallFence,
    ) -> Result<std::result::Result<BudgetDispatchDecision, ModelResponse>> {
        match self.store.begin_budget_dispatch_typed(ctx, fence).await? {
            Ok(decision) => Ok(Ok(decision)),
            Err(refusal) => Ok(Err(self
                .release_pre_dispatch(ctx, request, root, fence, refusal.code())
                .await?)),
        }
    }
}

#[async_trait]
impl<T: ModelTransport> ModelPort for PersistentModelBroker<T> {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        if let Err(error) = request.validate() {
            return Ok(ModelResponse::Rejected {
                request_id: request.request_id,
                kind: ModelRejectionKind::InvalidRequest,
                reason: error.to_string(),
                dispatch: RejectedDispatch::NotDispatched,
            });
        }
        let ctx = self.broker_context(&request.namespace)?;
        let existing = match self
            .store
            .budget_call_typed(&ctx, &self.config.billing_scope, &request.request_id)
            .await?
        {
            Ok(existing) => existing,
            Err(refusal) => return Ok(pre_dispatch_refusal(&request, &refusal)),
        };
        if let Some(existing) = existing {
            if existing.actual_input_digest != request.cache_key_digest {
                return Ok(pre_dispatch_refusal(
                    &request,
                    &BudgetPreDispatchRefusal::CallIdReused,
                ));
            }
            if existing.state != BudgetCallState::Reserved {
                let root = self
                    .store
                    .root_budget(&ctx, &self.config.billing_scope)
                    .await?
                    .ok_or(Error::Budget)?;
                return self.existing_response(&request, &root, existing).await;
            }
        }
        let Some(provenance) = self.transport.provenance() else {
            return Ok(ModelResponse::Rejected {
                request_id: request.request_id,
                kind: ModelRejectionKind::Unauthorized,
                reason: "model transport is disabled".into(),
                dispatch: RejectedDispatch::NotDispatched,
            });
        };
        let now = self.now()?;
        let lease_until = now
            .checked_add(self.config.lease_seconds)
            .ok_or_else(|| Error::Invalid("broker lease overflow".into()))?;
        let lease_token = hash(
            format!(
                "{}\n{}\n{}\n{}",
                self.config.actor,
                self.config.billing_scope,
                request.request_id,
                request.cache_key_digest
            )
            .as_bytes(),
        );
        let reservation = BudgetCallReservation {
            billing_scope: self.config.billing_scope.clone(),
            call_id: request.request_id.clone(),
            dispatch_group_id: request.episode_id.clone(),
            stage: model_stage_budget(request.stage),
            actual_input_digest: request.cache_key_digest.clone(),
            request_artifact: Some(BudgetArtifact::from_serializable(
                MODEL_REQUEST_ARTIFACT_SCHEMA,
                &request,
            )?),
            max_cost_micros: self.config.max_cost_micros,
            lease_token,
            lease_until,
            now,
        };
        let source_ids = request
            .source_closure
            .iter()
            .map(|source| source.id.clone())
            .collect::<Vec<_>>();
        let mut call = match self
            .store
            .reserve_budget_call_with_sources_typed(&ctx, &reservation, &source_ids)
            .await?
        {
            Ok(call) => call,
            Err(refusal) => return Ok(pre_dispatch_refusal(&request, &refusal)),
        };
        if call.state == BudgetCallState::Reserved && now > call.lease_until {
            call = self
                .store
                .refence_reserved_budget_call(
                    &ctx,
                    &BudgetCallRefence {
                        billing_scope: call.billing_scope.clone(),
                        call_id: call.call_id.clone(),
                        expected_epoch: call.lease_epoch,
                        new_lease_token: reservation.lease_token.clone(),
                        new_lease_until: lease_until,
                        now,
                    },
                )
                .await?;
        }
        let fence = BudgetCallFence {
            billing_scope: call.billing_scope.clone(),
            call_id: call.call_id.clone(),
            actual_input_digest: call.actual_input_digest.clone(),
            lease_token: call.lease_token.clone(),
            lease_epoch: call.lease_epoch,
            now,
        };
        let root = self
            .store
            .root_budget(&ctx, &self.config.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        if root.currency != self.config.currency
            || root.pricing_version != self.config.pricing_version
            || self.config.max_cost_micros > root.per_call_cap_micros
        {
            return self
                .release_pre_dispatch(&ctx, &request, &root, &fence, "configuration_mismatch")
                .await;
        }
        let decision = match self
            .begin_pre_dispatch(&ctx, &request, &root, &fence)
            .await?
        {
            Ok(decision) => decision,
            Err(response) => return Ok(response),
        };
        if !decision.new_dispatch {
            return self.existing_response(&request, &root, decision.call).await;
        }
        let dispatch_id = decision.call.dispatch_id.clone().ok_or(Error::Internal)?;
        let completion = match self.transport.execute(&request, &dispatch_id).await {
            Ok(completion) => completion,
            Err(_) => {
                let failure_fence = BudgetCallFence {
                    now: self.now()?,
                    ..fence.clone()
                };
                let uncertain = self
                    .store
                    .mark_budget_call_uncertain(
                        &ctx,
                        &failure_fence,
                        "transport_failed_after_dispatch",
                    )
                    .await?;
                return Ok(ModelResponse::Uncertain {
                    request_id: request.request_id,
                    dispatch_id,
                    root_budget_id: root.root_budget_id,
                    provider_request_id: uncertain.provider_request_id,
                    usage_record_id: uncertain.usage_record_id,
                });
            }
        };
        if let Err(error) = completion.validate() {
            let failure_fence = BudgetCallFence {
                now: self.now()?,
                ..fence.clone()
            };
            if completion.validate_accounting().is_ok() {
                let output_digest = hash(completion.output.as_bytes());
                let receipt = ModelExecutionReceipt {
                    call_id: decision.call.call_id.clone(),
                    dispatch_id: dispatch_id.clone(),
                    root_budget_id: root.root_budget_id.clone(),
                    provider_request_id: completion.provider_request_id.clone(),
                    usage_record_id: completion.usage_record_id.clone(),
                    provenance,
                };
                receipt.validate()?;
                let blocked_response = ModelResponse::Rejected {
                    request_id: request.request_id.clone(),
                    kind: ModelRejectionKind::ProviderRejected,
                    reason: "provider completion was invalid; usage retained for reconciliation"
                        .into(),
                    dispatch: RejectedDispatch::Dispatched {
                        receipt: receipt.clone(),
                    },
                };
                blocked_response.validate_against(&request)?;
                let metadata = InvalidTransportMetadata {
                    schema_version: "rsia.invalid_model_transport_metadata.v1",
                    provider_request_id: &completion.provider_request_id,
                    usage_record_id: &completion.usage_record_id,
                    actual_cost_micros: completion.actual_cost_micros,
                    currency: &completion.currency,
                    pricing_version: &completion.pricing_version,
                    response_id_bytes: completion.response_id.len(),
                    response_id_digest: hash(completion.response_id.as_bytes()),
                    actual_model_value_bytes: completion.actual_model_digest.len(),
                    actual_model_value_digest: hash(completion.actual_model_digest.as_bytes()),
                    output_bytes: completion.output.len(),
                    output_digest: output_digest.clone(),
                    validation_error: error.to_string(),
                };
                let settlement = self
                    .store
                    .settle_model_budget_call(
                        &ctx,
                        &failure_fence,
                        &UsageCharge {
                            amount_micros: completion.actual_cost_micros,
                            currency: completion.currency.clone(),
                            pricing_version: completion.pricing_version.clone(),
                            provider_request_id: completion.provider_request_id.clone(),
                            usage_record_id: completion.usage_record_id.clone(),
                            output_digest,
                        },
                        &ModelCallSettlementEvidence {
                            provenance: storage_provenance(provenance),
                            actual_model_digest: is_sha256(&completion.actual_model_digest)
                                .then(|| completion.actual_model_digest.clone()),
                            transport_artifact: BudgetArtifact::from_serializable(
                                MODEL_TRANSPORT_ARTIFACT_SCHEMA,
                                &metadata,
                            )?,
                            usable_response: None,
                            blocked_response: BudgetArtifact::from_serializable(
                                MODEL_RESPONSE_ARTIFACT_SCHEMA,
                                &blocked_response,
                            )?,
                            forced_block_reason: Some("invalid_transport_completion".into()),
                        },
                    )
                    .await?;
                self.store
                    .close_budget_call_execution(
                        &ctx,
                        &settlement.call.billing_scope,
                        &settlement.call.call_id,
                        &dispatch_id,
                        "invalid_transport_completion_persisted",
                        self.now()?,
                    )
                    .await?;
                let response: ModelResponse =
                    serde_json::from_str(&settlement.response_artifact.body)
                        .map_err(|_| Error::Internal)?;
                response.validate_against(&request)?;
                self.verify_persisted_response(&request, &response).await?;
                return Ok(response);
            }
            self.store
                .mark_budget_call_uncertain(&ctx, &failure_fence, "invalid_transport_completion")
                .await?;
            return Err(error);
        }
        let model_matches = completion.actual_model_digest == request.model_digest;
        let output_digest = hash(completion.output.as_bytes());
        let charge = UsageCharge {
            amount_micros: completion.actual_cost_micros,
            currency: completion.currency.clone(),
            pricing_version: completion.pricing_version.clone(),
            provider_request_id: completion.provider_request_id.clone(),
            usage_record_id: completion.usage_record_id.clone(),
            output_digest: output_digest.clone(),
        };
        let receipt = ModelExecutionReceipt {
            call_id: decision.call.call_id.clone(),
            dispatch_id: dispatch_id.clone(),
            root_budget_id: root.root_budget_id.clone(),
            provider_request_id: completion.provider_request_id.clone(),
            usage_record_id: completion.usage_record_id.clone(),
            provenance,
        };
        receipt.validate()?;
        let completed_response = model_matches.then(|| ModelResponse::Completed {
            request_id: request.request_id.clone(),
            response_id: completion.response_id.clone(),
            actual_model_digest: completion.actual_model_digest.clone(),
            input_digest: request.input_digest.clone(),
            output: completion.output.clone(),
            output_digest: output_digest.clone(),
            execution_receipt: receipt.clone(),
        });
        if let Some(response) = &completed_response {
            response.validate_against(&request)?;
        }
        let blocked_response = ModelResponse::Rejected {
            request_id: request.request_id.clone(),
            kind: if model_matches {
                ModelRejectionKind::CancelledAfterDispatch
            } else {
                ModelRejectionKind::ProviderRejected
            },
            reason: if model_matches {
                "dispatch became ineligible after provider execution; result retained for audit"
                    .into()
            } else {
                "provider used a model different from the frozen request; result retained for audit"
                    .into()
            },
            dispatch: RejectedDispatch::Dispatched {
                receipt: receipt.clone(),
            },
        };
        blocked_response.validate_against(&request)?;
        let settlement_evidence = ModelCallSettlementEvidence {
            provenance: storage_provenance(provenance),
            actual_model_digest: Some(completion.actual_model_digest.clone()),
            transport_artifact: BudgetArtifact::from_serializable(
                MODEL_TRANSPORT_ARTIFACT_SCHEMA,
                &completion,
            )?,
            usable_response: completed_response
                .as_ref()
                .map(|response| {
                    BudgetArtifact::from_serializable(MODEL_RESPONSE_ARTIFACT_SCHEMA, response)
                })
                .transpose()?,
            blocked_response: BudgetArtifact::from_serializable(
                MODEL_RESPONSE_ARTIFACT_SCHEMA,
                &blocked_response,
            )?,
            forced_block_reason: (!model_matches).then_some("actual_model_digest_mismatch".into()),
        };
        let settled_at = self.now()?;
        let settlement_fence = BudgetCallFence {
            now: settled_at,
            ..fence.clone()
        };
        let settlement = match self
            .store
            .settle_model_budget_call(&ctx, &settlement_fence, &charge, &settlement_evidence)
            .await
        {
            Ok(settlement) => settlement,
            Err(error) => {
                let _ = self
                    .store
                    .mark_budget_call_uncertain(
                        &ctx,
                        &settlement_fence,
                        "model_settlement_failed_after_dispatch",
                    )
                    .await;
                return Err(error);
            }
        };
        self.store
            .close_budget_call_execution(
                &ctx,
                &settlement.call.billing_scope,
                &settlement.call.call_id,
                &dispatch_id,
                "transport_completion_persisted",
                self.now()?,
            )
            .await?;
        let response: ModelResponse = serde_json::from_str(&settlement.response_artifact.body)
            .map_err(|_| Error::Internal)?;
        response.validate_against(&request)?;
        self.verify_persisted_response(&request, &response).await?;
        Ok(response)
    }
}

fn pre_dispatch_refusal(
    request: &ModelRequest,
    refusal: &BudgetPreDispatchRefusal,
) -> ModelResponse {
    let kind = match refusal {
        BudgetPreDispatchRefusal::RootStopped
        | BudgetPreDispatchRefusal::GroupStopped
        | BudgetPreDispatchRefusal::LeaseExpired => ModelRejectionKind::CancelledBeforeDispatch,
        BudgetPreDispatchRefusal::SourceRevoked
        | BudgetPreDispatchRefusal::Unauthorized
        | BudgetPreDispatchRefusal::CallNamespaceMismatch
        | BudgetPreDispatchRefusal::GroupNamespaceMismatch => ModelRejectionKind::Unauthorized,
        BudgetPreDispatchRefusal::RootConcurrencyLimit
        | BudgetPreDispatchRefusal::RootAuthorizationMissing
        | BudgetPreDispatchRefusal::PerCallCapExceeded
        | BudgetPreDispatchRefusal::RootBudgetExhausted
        | BudgetPreDispatchRefusal::BudgetArithmeticOverflow
        | BudgetPreDispatchRefusal::ActiveLeaseCapacity { .. } => {
            ModelRejectionKind::BudgetUnavailable
        }
        BudgetPreDispatchRefusal::CallIdReused
        | BudgetPreDispatchRefusal::InvalidRequest { .. }
        | BudgetPreDispatchRefusal::InvalidArtifactDigest
        | BudgetPreDispatchRefusal::DuplicateSource
        | BudgetPreDispatchRefusal::SourceClosureChanged => ModelRejectionKind::InvalidRequest,
    };
    ModelResponse::Rejected {
        request_id: request.request_id.clone(),
        kind,
        reason: format!("{PRE_DISPATCH_REASON_PREFIX}{}", refusal.code()),
        dispatch: RejectedDispatch::NotDispatched,
    }
}

fn undispatched_terminal_response(
    request: &ModelRequest,
    call: &BudgetCallRecord,
) -> ModelResponse {
    let decoded = (call.state == BudgetCallState::Released)
        .then_some(call.terminal_reason.as_deref())
        .flatten()
        .and_then(|reason| {
            reason
                .strip_prefix(PRE_DISPATCH_REASON_PREFIX)
                .map(|code| (reason, code))
        })
        .and_then(|(reason, code)| {
            let kind = match code {
                "root_stopped" | "group_stopped" | "lease_expired" => {
                    ModelRejectionKind::CancelledBeforeDispatch
                }
                "source_revoked"
                | "unauthorized"
                | "group_namespace_mismatch"
                | "configuration_mismatch" => ModelRejectionKind::Unauthorized,
                "root_concurrency_limit"
                | "root_authorization_missing"
                | "per_call_cap_exceeded"
                | "root_budget_exhausted"
                | "budget_arithmetic_overflow"
                | "active_lease_capacity" => ModelRejectionKind::BudgetUnavailable,
                "call_id_reused"
                | "invalid_request"
                | "duplicate_source"
                | "source_closure_changed" => ModelRejectionKind::InvalidRequest,
                _ => return None,
            };
            Some((kind, reason.to_owned()))
        });
    let (kind, reason) = decoded.unwrap_or((
        ModelRejectionKind::CancelledBeforeDispatch,
        "call ended before dispatch".into(),
    ));
    ModelResponse::Rejected {
        request_id: request.request_id.clone(),
        kind,
        reason,
        dispatch: RejectedDispatch::NotDispatched,
    }
}

/// The ledger stage a model call is reserved under. Exhaustive on purpose: a new
/// `ModelStage` must choose its budget stage here. A consolidation call is
/// metered as `Consolidation`, apart from reflection and ranking, yet it is
/// reserved against the same root budget like every other call.
fn model_stage_budget(stage: ModelStage) -> BudgetStage {
    match stage {
        ModelStage::ReflectFailure | ModelStage::ReflectSuccess => BudgetStage::Reflection,
        ModelStage::Merge => BudgetStage::Merge,
        ModelStage::Rank => BudgetStage::Ranking,
        ModelStage::Consolidate => BudgetStage::Consolidation,
    }
}

fn storage_provenance(value: ModelExecutionProvenance) -> BudgetExecutionProvenance {
    match value {
        ModelExecutionProvenance::Fixture => BudgetExecutionProvenance::Fixture,
        ModelExecutionProvenance::ExternalProvider => BudgetExecutionProvenance::ExternalProvider,
    }
}

fn engine_provenance(value: BudgetExecutionProvenance) -> ModelExecutionProvenance {
    match value {
        BudgetExecutionProvenance::Fixture => ModelExecutionProvenance::Fixture,
        BudgetExecutionProvenance::ExternalProvider => ModelExecutionProvenance::ExternalProvider,
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod pre_dispatch_tests {
    use super::*;
    use evo_core::evidence::Purpose;
    use evo_core::optimization::{ModelInputPart, ModelInputRole, ModelRequestContext};
    use evo_core::skill_edit::EvidenceRef;
    use evo_storage::budget::RootBudgetAuthorization;

    struct GateFixture {
        _dir: tempfile::TempDir,
        broker: PersistentModelBroker<DisabledModelTransport>,
        ctx: Context,
        request: ModelRequest,
        call: BudgetCallRecord,
        root: RootBudgetRecord,
    }

    impl GateFixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("broker.sqlite3"))
                .await
                .unwrap();
            let admin = Context::new("n", "admin", Role::Admin).unwrap();
            let ctx = Context::new("n", "broker", Role::Host).unwrap();
            let root = store
                .authorize_root_budget(
                    &admin,
                    &RootBudgetAuthorization {
                        root_budget_id: "root".into(),
                        billing_scope: "scope".into(),
                        allowed_namespaces: vec!["n".into()],
                        currency: "USD".into(),
                        pricing_version: "price".into(),
                        payment_subject: "fixture".into(),
                        authorization_receipt_digest: hash(b"auth"),
                        per_call_cap_micros: 20,
                        total_limit_micros: 100,
                        created_at: 1,
                    },
                )
                .await
                .unwrap();
            let request = ModelRequest::build(
                ModelRequestContext {
                    request_id: "request".into(),
                    namespace: "n".into(),
                    purpose: Purpose::Development,
                    stage: ModelStage::Merge,
                    episode_id: "group".into(),
                    step: 1,
                    attempt: 1,
                    parent_skill_digest: hash(b"parent"),
                    bundle_digest: hash(b"bundle"),
                    source_closure: vec![EvidenceRef {
                        id: "source".into(),
                        digest: hash(b"source"),
                    }],
                    model_digest: hash(b"model"),
                    tools_digest: hash(b"tools"),
                    rules_digest: hash(b"rules"),
                    sampling_digest: hash(b"sampling"),
                    revoke_watermark: 0,
                    max_suggestions: 1,
                },
                vec![ModelInputPart {
                    role: ModelInputRole::User,
                    label: "task".into(),
                    content: "fixture".into(),
                }],
            )
            .unwrap();
            let call = store
                .reserve_budget_call_with_sources(
                    &ctx,
                    &BudgetCallReservation {
                        billing_scope: "scope".into(),
                        call_id: request.request_id.clone(),
                        dispatch_group_id: "group".into(),
                        stage: BudgetStage::Merge,
                        actual_input_digest: request.cache_key_digest.clone(),
                        request_artifact: Some(
                            BudgetArtifact::from_serializable(
                                MODEL_REQUEST_ARTIFACT_SCHEMA,
                                &request,
                            )
                            .unwrap(),
                        ),
                        max_cost_micros: 20,
                        lease_token: "lease".into(),
                        lease_until: 70,
                        now: 10,
                    },
                    &["source".into()],
                )
                .await
                .unwrap();
            let broker = PersistentModelBroker::with_clock(
                store,
                DisabledModelTransport,
                BrokerConfig {
                    billing_scope: "scope".into(),
                    actor: "broker".into(),
                    currency: "USD".into(),
                    pricing_version: "price".into(),
                    max_cost_micros: 20,
                    lease_seconds: 60,
                },
                Arc::new(|| 10),
            )
            .unwrap();
            Self {
                _dir: dir,
                broker,
                ctx,
                request,
                call,
                root,
            }
        }

        fn fence(&self, now: i64) -> BudgetCallFence {
            BudgetCallFence {
                billing_scope: self.call.billing_scope.clone(),
                call_id: self.call.call_id.clone(),
                actual_input_digest: self.call.actual_input_digest.clone(),
                lease_token: self.call.lease_token.clone(),
                lease_epoch: self.call.lease_epoch,
                now,
            }
        }
    }

    fn assert_refusal(response: &ModelResponse, expected: ModelRejectionKind, code: &str) {
        match response {
            ModelResponse::Rejected {
                kind,
                reason,
                dispatch,
                ..
            } => {
                assert_eq!(*kind, expected);
                assert_eq!(reason, &format!("{PRE_DISPATCH_REASON_PREFIX}{code}"));
                assert_eq!(*dispatch, RejectedDispatch::NotDispatched);
            }
            other => panic!("expected pre-dispatch refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn all_typed_refusals_and_released_codes_use_the_fixed_mapping() {
        use BudgetPreDispatchRefusal as R;
        use ModelRejectionKind as K;
        let fixture = GateFixture::new().await;
        for (refusal, kind) in [
            (R::RootStopped, K::CancelledBeforeDispatch),
            (R::GroupStopped, K::CancelledBeforeDispatch),
            (R::LeaseExpired, K::CancelledBeforeDispatch),
            (R::SourceRevoked, K::Unauthorized),
            (R::RootConcurrencyLimit, K::BudgetUnavailable),
            (R::CallIdReused, K::InvalidRequest),
            (R::RootAuthorizationMissing, K::BudgetUnavailable),
            (R::PerCallCapExceeded, K::BudgetUnavailable),
            (R::RootBudgetExhausted, K::BudgetUnavailable),
            (R::BudgetArithmeticOverflow, K::BudgetUnavailable),
            (
                R::ActiveLeaseCapacity {
                    used: 10,
                    limit: 10,
                },
                K::BudgetUnavailable,
            ),
            (R::Unauthorized, K::Unauthorized),
            (R::CallNamespaceMismatch, K::Unauthorized),
            (R::GroupNamespaceMismatch, K::Unauthorized),
            (
                R::InvalidRequest {
                    message: "fixture secret omitted".into(),
                },
                K::InvalidRequest,
            ),
            (R::InvalidArtifactDigest, K::InvalidRequest),
            (R::DuplicateSource, K::InvalidRequest),
            (R::SourceClosureChanged, K::InvalidRequest),
        ] {
            let response = pre_dispatch_refusal(&fixture.request, &refusal);
            assert_refusal(&response, kind, refusal.code());
            let mut released = fixture.call.clone();
            released.state = BudgetCallState::Released;
            released.terminal_reason =
                Some(format!("{PRE_DISPATCH_REASON_PREFIX}{}", refusal.code()));
            assert_eq!(
                serde_json::to_value(undispatched_terminal_response(&fixture.request, &released))
                    .unwrap(),
                serde_json::to_value(response).unwrap()
            );
        }
    }

    #[tokio::test]
    async fn expired_owned_lease_is_classified_released_and_replayed() {
        let fixture = GateFixture::new().await;
        let response = fixture
            .broker
            .begin_pre_dispatch(
                &fixture.ctx,
                &fixture.request,
                &fixture.root,
                &fixture.fence(71),
            )
            .await
            .unwrap()
            .unwrap_err();
        assert_refusal(
            &response,
            ModelRejectionKind::CancelledBeforeDispatch,
            "lease_expired",
        );
        let replay = fixture
            .broker
            .dispatch(fixture.request.clone())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::to_value(replay).unwrap()
        );
        assert_eq!(
            fixture
                .broker
                .store
                .root_budget(&fixture.ctx, "scope")
                .await
                .unwrap()
                .unwrap()
                .reserved_micros,
            0
        );
    }

    #[tokio::test]
    async fn release_failure_for_stale_fence_is_propagated_and_winning_reason_is_used() {
        let fixture = GateFixture::new().await;
        let newer = fixture
            .broker
            .store
            .refence_reserved_budget_call(
                &fixture.ctx,
                &BudgetCallRefence {
                    billing_scope: "scope".into(),
                    call_id: "request".into(),
                    expected_epoch: 1,
                    new_lease_token: "new-lease".into(),
                    new_lease_until: 200,
                    now: 71,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .broker
                .release_pre_dispatch(
                    &fixture.ctx,
                    &fixture.request,
                    &fixture.root,
                    &fixture.fence(71),
                    "root_concurrency_limit"
                )
                .await,
            Err(Error::Cancelled)
        ));
        assert_eq!(
            fixture
                .broker
                .store
                .budget_call(&fixture.ctx, "scope", "request")
                .await
                .unwrap()
                .unwrap(),
            newer
        );
        assert_eq!(
            fixture
                .broker
                .store
                .root_budget(&fixture.ctx, "scope")
                .await
                .unwrap()
                .unwrap()
                .reserved_micros,
            20
        );
        let new_fence = BudgetCallFence {
            lease_token: newer.lease_token.clone(),
            lease_epoch: newer.lease_epoch,
            now: 72,
            ..fixture.fence(10)
        };
        fixture
            .broker
            .store
            .release_undispatched_budget_call(
                &fixture.ctx,
                &new_fence,
                "broker_pre_dispatch_v1.source_revoked",
            )
            .await
            .unwrap();
        let response = fixture
            .broker
            .release_pre_dispatch(
                &fixture.ctx,
                &fixture.request,
                &fixture.root,
                &new_fence,
                "root_concurrency_limit",
            )
            .await
            .unwrap();
        assert_refusal(
            &response,
            ModelRejectionKind::Unauthorized,
            "source_revoked",
        );
        let replay = fixture
            .broker
            .dispatch(fixture.request.clone())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::to_value(replay).unwrap()
        );
    }

    #[tokio::test]
    async fn dispatch_winning_release_race_keeps_money_and_propagates_failure() {
        let fixture = GateFixture::new().await;
        let fence = fixture.fence(11);
        let dispatched = fixture
            .broker
            .store
            .begin_budget_dispatch(&fixture.ctx, &fence)
            .await
            .unwrap()
            .call;
        assert!(
            matches!(fixture.broker.release_pre_dispatch(&fixture.ctx, &fixture.request, &fixture.root, &fence,
            "root_concurrency_limit").await, Err(Error::Conflict(message)) if message == "only_undispatched_reservation_can_be_released")
        );
        assert_eq!(
            fixture
                .broker
                .store
                .budget_call(&fixture.ctx, "scope", "request")
                .await
                .unwrap()
                .unwrap(),
            dispatched
        );
        assert_eq!(
            fixture
                .broker
                .store
                .root_budget(&fixture.ctx, "scope")
                .await
                .unwrap()
                .unwrap()
                .reserved_micros,
            20
        );
    }

    #[tokio::test]
    async fn generic_begin_error_is_not_a_refusal_or_refund() {
        let fixture = GateFixture::new().await;
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let digest =
            evo_core::fingerprint(&("rsia.budget_call_ref.v1", "n", "scope", "request")).unwrap();
        let mut session = fixture.broker.store.session().await.unwrap();
        session
            .put(
                &admin,
                "artifact",
                &format!("budget-ref-{}", &digest[..32]),
                admin.actor(),
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert!(matches!(
            fixture
                .broker
                .begin_pre_dispatch(
                    &fixture.ctx,
                    &fixture.request,
                    &fixture.root,
                    &fixture.fence(11)
                )
                .await,
            Err(Error::Internal)
        ));
        assert_eq!(
            fixture
                .broker
                .store
                .budget_call(&fixture.ctx, "scope", "request")
                .await
                .unwrap()
                .unwrap(),
            fixture.call
        );
        assert_eq!(
            fixture
                .broker
                .store
                .root_budget(&fixture.ctx, "scope")
                .await
                .unwrap()
                .unwrap()
                .reserved_micros,
            20
        );
    }
}
