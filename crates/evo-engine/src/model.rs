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
    /// A port must explicitly support the complete Current wire. The legacy
    /// adapter remains useful for historical accounting, never for a cast.
    async fn dispatch_compiler_bound(
        &self,
        request: ReadBoundModelRequest,
    ) -> Result<ModelResponse> {
        match request.legacy() {
            Some(legacy) => self.dispatch_bound(legacy.clone()).await,
            None => Err(Error::Forbidden),
        }
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
        Err(Error::Conflict(
            "legacy compiler use is not Current input".into(),
        ))
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

pub const CURRENT_COMPILER_USE_SCHEMA: &str = "rsia.optimization.compiler_use.v2";
pub const CURRENT_BOUND_MODEL_SCHEMA: &str = "rsia.bound_model_request.v2";

/// Data only. All original sources are authenticated again before reservation
/// or a continuation write; Deserialize cannot mint the opaque resolver result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentCompilerInputUse {
    pub schema_version: String,
    pub request: crate::formal_authority::CompilerInputRequest,
    pub binding: crate::formal_authority::CurrentFormalCompilerInputBinding,
}
impl CurrentCompilerInputUse {
    pub fn new(
        request: crate::formal_authority::CompilerInputRequest,
        binding: crate::formal_authority::CurrentFormalCompilerInputBinding,
    ) -> Self {
        Self {
            schema_version: CURRENT_COMPILER_USE_SCHEMA.into(),
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
        if self.schema_version != CURRENT_COMPILER_USE_SCHEMA
            || self.binding.schema_version != crate::formal_authority::CURRENT_BINDING_SCHEMA
            || self.binding.namespace != ctx.namespace()
        {
            return Err(Error::Forbidden);
        }
        let actual =
            crate::formal_authority::resolve_current_compiler_inputs(s, ctx, &self.request).await?;
        if serde_json::to_vec(actual.binding()).map_err(|_| Error::Internal)?
            != serde_json::to_vec(&self.binding).map_err(|_| Error::Internal)?
        {
            return Err(Error::Conflict(
                "compiler authority changed before continuation".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentBoundModelRequest {
    pub schema_version: String,
    pub request: ModelRequest,
    pub compiler_use: CurrentCompilerInputUse,
}
impl CurrentBoundModelRequest {
    pub fn new(request: ModelRequest, compiler_use: CurrentCompilerInputUse) -> Result<Self> {
        let value = Self {
            schema_version: CURRENT_BOUND_MODEL_SCHEMA.into(),
            request,
            compiler_use,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        self.request.validate()?;
        if self.schema_version != CURRENT_BOUND_MODEL_SCHEMA
            || self.compiler_use.schema_version != CURRENT_COMPILER_USE_SCHEMA
            || self.compiler_use.binding.schema_version
                != crate::formal_authority::CURRENT_BINDING_SCHEMA
            || self.request.namespace != self.compiler_use.binding.namespace
            || self.request.purpose != self.compiler_use.binding.purpose
            || self.request.parent_skill_digest
                != evo_core::skill_edit::skill_snapshot_digest(
                    &self.compiler_use.request.search_parent,
                )?
            || self.request.revoke_watermark != self.compiler_use.request.revoke_watermark
        {
            return Err(Error::Conflict(
                "model request differs from complete Current compiler input".into(),
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

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum CompilerUseWire {
    Current(CurrentCompilerInputUse),
    Legacy(CompilerInputUse),
}
#[derive(Debug, Clone)]
pub struct ReadCompilerUseView {
    pub schema_version: String,
    pub request: crate::formal_authority::CompilerInputRequest,
    pub binding: crate::formal_authority::ReadCompilerBinding,
}
#[derive(Debug, Clone)]
pub struct ReadCompilerInputUse {
    wire: Box<CompilerUseWire>,
    view: Box<ReadCompilerUseView>,
}
impl ReadCompilerInputUse {
    fn from_wire(wire: CompilerUseWire) -> Result<Self> {
        let view = match &wire {
            CompilerUseWire::Current(v) => {
                if v.schema_version != CURRENT_COMPILER_USE_SCHEMA
                    || v.binding.schema_version != crate::formal_authority::CURRENT_BINDING_SCHEMA
                {
                    return Err(Error::Invalid(
                        "unsupported Current compiler-use wire".into(),
                    ));
                }
                ReadCompilerUseView {
                    schema_version: v.schema_version.clone(),
                    request: v.request.clone(),
                    binding: crate::formal_authority::ReadCompilerBinding::from_current(
                        v.binding.clone(),
                    )?,
                }
            }
            CompilerUseWire::Legacy(v) => {
                if v.schema_version != "rsia.optimization.compiler_use.v1"
                    || v.binding.schema_version != crate::formal_authority::BINDING_SCHEMA
                {
                    return Err(Error::Invalid(
                        "unsupported legacy compiler-use wire".into(),
                    ));
                }
                ReadCompilerUseView {
                    schema_version: v.schema_version.clone(),
                    request: v.request.clone(),
                    binding: crate::formal_authority::ReadCompilerBinding::from_legacy(
                        v.binding.clone(),
                    )?,
                }
            }
        };
        Ok(Self {
            wire: Box::new(wire),
            view: Box::new(view),
        })
    }
    pub(crate) fn from_current(v: CurrentCompilerInputUse) -> Result<Self> {
        Self::from_wire(CompilerUseWire::Current(v))
    }
    pub(crate) fn from_legacy(v: CompilerInputUse) -> Result<Self> {
        Self::from_wire(CompilerUseWire::Legacy(v))
    }
    pub fn artifact_id(&self) -> Result<String> {
        Ok(format!("compiler-use-{}", evo_core::fingerprint(self)?))
    }
    pub(crate) fn is_current(&self) -> bool {
        matches!(self.wire.as_ref(), CompilerUseWire::Current(_))
    }
    pub async fn verify_current(
        &self,
        s: &mut evo_storage::Session,
        ctx: &evo_core::Context,
    ) -> Result<()> {
        match self.wire.as_ref() {
            CompilerUseWire::Current(v) => v.verify_current(s, ctx).await,
            CompilerUseWire::Legacy(v) => v.verify_current(s, ctx).await,
        }
    }
}
impl std::ops::Deref for ReadCompilerInputUse {
    type Target = ReadCompilerUseView;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}
impl Serialize for ReadCompilerInputUse {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self.wire.as_ref() {
            CompilerUseWire::Current(v) => v.serialize(s),
            CompilerUseWire::Legacy(v) => v.serialize(s),
        }
    }
}
impl<'de> Deserialize<'de> for ReadCompilerInputUse {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::from_wire(CompilerUseWire::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum BoundRequestWire {
    Current(CurrentBoundModelRequest),
    Legacy(BoundModelRequest),
}
#[derive(Debug, Clone)]
pub struct ReadBoundRequestView {
    pub schema_version: String,
    pub request: ModelRequest,
    pub compiler_use: ReadCompilerInputUse,
}
#[derive(Debug, Clone)]
pub struct ReadBoundModelRequest {
    wire: Box<BoundRequestWire>,
    view: Box<ReadBoundRequestView>,
}
impl ReadBoundModelRequest {
    fn from_wire(wire: BoundRequestWire) -> Result<Self> {
        let view = match &wire {
            BoundRequestWire::Current(v) => {
                v.validate()?;
                ReadBoundRequestView {
                    schema_version: v.schema_version.clone(),
                    request: v.request.clone(),
                    compiler_use: ReadCompilerInputUse::from_current(v.compiler_use.clone())?,
                }
            }
            BoundRequestWire::Legacy(v) => {
                v.validate()?;
                ReadBoundRequestView {
                    schema_version: v.schema_version.clone(),
                    request: v.request.clone(),
                    compiler_use: ReadCompilerInputUse::from_legacy(v.compiler_use.clone())?,
                }
            }
        };
        Ok(Self {
            wire: Box::new(wire),
            view: Box::new(view),
        })
    }
    pub(crate) fn from_current(v: CurrentBoundModelRequest) -> Result<Self> {
        Self::from_wire(BoundRequestWire::Current(v))
    }
    pub(crate) fn from_legacy(v: BoundModelRequest) -> Result<Self> {
        Self::from_wire(BoundRequestWire::Legacy(v))
    }
    fn legacy(&self) -> Option<&BoundModelRequest> {
        match self.wire.as_ref() {
            BoundRequestWire::Legacy(v) => Some(v),
            _ => None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self.wire.as_ref() {
            BoundRequestWire::Current(v) => v.validate(),
            BoundRequestWire::Legacy(v) => v.validate(),
        }
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
impl std::ops::Deref for ReadBoundModelRequest {
    type Target = ReadBoundRequestView;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}
impl Serialize for ReadBoundModelRequest {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self.wire.as_ref() {
            BoundRequestWire::Current(v) => v.serialize(s),
            BoundRequestWire::Legacy(v) => v.serialize(s),
        }
    }
}
impl<'de> Deserialize<'de> for ReadBoundModelRequest {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::from_wire(BoundRequestWire::deserialize(d)?).map_err(serde::de::Error::custom)
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
