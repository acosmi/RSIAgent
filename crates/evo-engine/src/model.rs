//! Trusted model-dispatch port. Model output can never mint its own receipt.

use async_trait::async_trait;
use evo_core::optimization::ModelRequest;
use evo_core::{Error, Result, hash, identifier, text};
use serde::{Deserialize, Serialize};

fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(format!("{name} must be a lowercase sha256")));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelExecutionProvenance {
    Fixture,
    ExternalProvider,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelExecutionReceipt {
    pub call_id: String,
    pub dispatch_id: String,
    pub root_budget_id: String,
    pub provider_request_id: String,
    pub usage_record_id: String,
    pub provenance: ModelExecutionProvenance,
}

impl ModelExecutionReceipt {
    pub fn validate(&self) -> Result<()> {
        for value in [
            &self.call_id,
            &self.dispatch_id,
            &self.root_budget_id,
            &self.provider_request_id,
            &self.usage_record_id,
        ] {
            identifier(value)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRejectionKind {
    Unauthorized,
    BudgetUnavailable,
    InvalidRequest,
    ProviderRejected,
    CancelledBeforeDispatch,
    CancelledAfterDispatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum RejectedDispatch {
    NotDispatched,
    Dispatched { receipt: ModelExecutionReceipt },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelResponse {
    Completed {
        request_id: String,
        response_id: String,
        actual_model_digest: String,
        input_digest: String,
        output: String,
        output_digest: String,
        execution_receipt: ModelExecutionReceipt,
    },
    Rejected {
        request_id: String,
        kind: ModelRejectionKind,
        reason: String,
        dispatch: RejectedDispatch,
    },
    Uncertain {
        request_id: String,
        dispatch_id: String,
        root_budget_id: String,
        provider_request_id: Option<String>,
        usage_record_id: Option<String>,
    },
}

impl ModelResponse {
    pub fn validate_against(&self, request: &ModelRequest) -> Result<()> {
        match self {
            Self::Completed {
                request_id,
                response_id,
                actual_model_digest,
                input_digest,
                output,
                output_digest,
                execution_receipt,
            } => {
                if request_id != &request.request_id || input_digest != &request.input_digest {
                    return Err(Error::Conflict(
                        "model response request binding mismatch".into(),
                    ));
                }
                identifier(response_id)?;
                validate_digest(actual_model_digest, "actual model digest")?;
                if actual_model_digest != &request.model_digest {
                    return Err(Error::Conflict(
                        "actual model differs from frozen request".into(),
                    ));
                }
                text(output, "model output", 256 * 1024)?;
                if output_digest != &hash(output.as_bytes()) {
                    return Err(Error::Invalid("model output digest mismatch".into()));
                }
                execution_receipt.validate()
            }
            Self::Rejected {
                request_id,
                kind,
                reason,
                dispatch,
            } => {
                if request_id != &request.request_id {
                    return Err(Error::Conflict("model rejection request mismatch".into()));
                }
                text(reason, "model rejection reason", 4096)?;
                if let RejectedDispatch::Dispatched { receipt } = dispatch {
                    receipt.validate()?;
                }
                match (kind, dispatch) {
                    (
                        ModelRejectionKind::CancelledBeforeDispatch,
                        RejectedDispatch::Dispatched { .. },
                    )
                    | (
                        ModelRejectionKind::CancelledAfterDispatch,
                        RejectedDispatch::NotDispatched,
                    ) => {
                        return Err(Error::Conflict(
                            "model cancellation phase contradicts dispatch evidence".into(),
                        ));
                    }
                    _ => {}
                }
                Ok(())
            }
            Self::Uncertain {
                request_id,
                dispatch_id,
                root_budget_id,
                provider_request_id,
                usage_record_id,
            } => {
                if request_id != &request.request_id {
                    return Err(Error::Conflict(
                        "uncertain model response request mismatch".into(),
                    ));
                }
                identifier(dispatch_id)?;
                identifier(root_budget_id)?;
                if let Some(value) = provider_request_id {
                    identifier(value)?;
                }
                if let Some(value) = usage_record_id {
                    identifier(value)?;
                }
                Ok(())
            }
        }
    }
}

#[async_trait]
pub trait ModelPort: Send + Sync {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse>;
    /// A production continuation must carry the full compiler authority. An
    /// adapter which only implements the legacy diagnostic port cannot send it.
    async fn dispatch_bound(&self, _request: BoundModelRequest) -> Result<ModelResponse> {
        Err(Error::Forbidden)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerInputUse {
    pub schema_version: String,
    pub request: crate::formal_authority::CompilerInputRequest,
    pub binding: crate::formal_authority::FormalCompilerInputBinding,
}
impl CompilerInputUse {
    pub fn new(
        request: crate::formal_authority::CompilerInputRequest,
        binding: crate::formal_authority::FormalCompilerInputBinding,
    ) -> Self {
        Self {
            schema_version: "rsia.optimization.compiler_use.v1".into(),
            request,
            binding,
        }
    }
    pub fn artifact_id(&self) -> Result<String> {
        Ok(format!("compiler-use-{}", evo_core::fingerprint(self)?))
    }
    pub async fn verify_current(
        &self,
        s: &mut evo_storage::Session,
        ctx: &evo_core::Context,
    ) -> Result<()> {
        if self.schema_version != "rsia.optimization.compiler_use.v1"
            || self.binding.namespace != ctx.namespace()
        {
            return Err(Error::Forbidden);
        }
        let actual =
            crate::formal_authority::resolve_compiler_inputs(s, ctx, &self.request).await?;
        if evo_core::fingerprint(actual.binding())? != evo_core::fingerprint(&self.binding)? {
            return Err(Error::Conflict(
                "compiler authority changed before continuation".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundModelRequest {
    pub schema_version: String,
    pub request: ModelRequest,
    pub compiler_use: CompilerInputUse,
}
impl BoundModelRequest {
    pub fn new(request: ModelRequest, compiler_use: CompilerInputUse) -> Result<Self> {
        let value = Self {
            schema_version: "rsia.bound_model_request.v1".into(),
            request,
            compiler_use,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        self.request.validate()?;
        if self.schema_version != "rsia.bound_model_request.v1"
            || self.compiler_use.schema_version != "rsia.optimization.compiler_use.v1"
            || self.request.namespace != self.compiler_use.binding.namespace
            || self.request.purpose != self.compiler_use.binding.purpose
            || self.request.parent_skill_digest
                != evo_core::skill_edit::skill_snapshot_digest(
                    &self.compiler_use.request.search_parent,
                )?
            || self.request.revoke_watermark != self.compiler_use.request.revoke_watermark
        {
            return Err(Error::Conflict(
                "model request differs from complete compiler input".into(),
            ));
        }
        Ok(())
    }
    pub async fn verify_current(
        &self,
        s: &mut evo_storage::Session,
        ctx: &evo_core::Context,
    ) -> Result<()> {
        self.validate()?;
        self.compiler_use.verify_current(s, ctx).await
    }
}

/// Task input only. Protected oracle, database and approval material have no
/// representable field in the provider request. The trusted broker reloads
/// candidate, ticket and application independently before any reservation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalTaskRequest {
    pub task: crate::streaming_evaluator::BrokerTaskView,
    pub candidate_id: String,
    pub profile_registration_id: String,
    pub compiler_binding_digest: String,
    pub model_digest: String,
    pub mandatory_context: String,
    pub projection: evo_core::contract::RunProjection,
    pub task_input_utf8: String,
}
impl FormalTaskRequest {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.candidate_id)?;
        identifier(&self.profile_registration_id)?;
        validate_digest(&self.compiler_binding_digest, "compiler binding")?;
        validate_digest(&self.model_digest, "formal model")?;
        text(&self.task_input_utf8, "formal task input", 128 * 1024)?;
        if hash(self.task_input_utf8.as_bytes()) != self.task.input_digest
            || self.projection.bundle_digest != self.task.bundle_digest
        {
            return Err(Error::Conflict("formal input/application mismatch".into()));
        }
        Ok(())
    }
}
