//! Deterministic chat bytes and domain-separated digests. These pure values
//! grant no registration, budget, dispatch, or provider capability.
use crate::contract::RunProjection;
use crate::optimization::{ModelInputRole, ModelRequest};
use crate::{Error, Result, canonical_fingerprint, identifier, text};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelProtocolId {
    #[serde(rename = "chat_completions_nonstream_v1")]
    ChatCompletionsNonstreamV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SamplingProfileId {
    #[serde(rename = "temperature_zero_v1")]
    TemperatureZeroV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    System,
    User,
}

#[derive(Debug, Clone, Serialize)]
pub struct SamplingProjectionV1 {
    temperature: u32,
}

/// Unauthenticated configuration data; it cannot authorize a real send.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatProviderProjectionInput {
    pub model_digest: String,
    pub provider_model: String,
    pub max_output_tokens: u32,
    pub protocol_id: ModelProtocolId,
    pub sampling_profile: SamplingProfileId,
}

/// Only the shared projectors can construct this immutable output. The exact
/// bytes, rather than a separately reconstructed JSON body, are sent.
#[derive(Debug, Clone)]
pub struct ProjectedChatRequest {
    wire_utf8: Vec<u8>,
    sampling_digest: String,
    payload_digest: String,
    protocol_id: ModelProtocolId,
}

impl ProjectedChatRequest {
    pub fn wire_utf8(&self) -> &[u8] {
        &self.wire_utf8
    }
    pub fn sampling_digest(&self) -> &str {
        &self.sampling_digest
    }
    pub fn payload_digest(&self) -> &str {
        &self.payload_digest
    }
    pub fn protocol_id(&self) -> ModelProtocolId {
        self.protocol_id
    }
}

pub fn sampling_projection_v1(profile: SamplingProfileId) -> SamplingProjectionV1 {
    match profile {
        SamplingProfileId::TemperatureZeroV1 => SamplingProjectionV1 { temperature: 0 },
    }
}

pub fn sampling_digest_v1(protocol: ModelProtocolId, profile: SamplingProfileId) -> Result<String> {
    canonical_fingerprint(&json!({
        "algorithm_version": "rsia.chat_sampling.sha256.cjson.v1",
        "protocol_id": protocol,
        "sampling": sampling_projection_v1(profile),
        "schema_version": "rsia.chat_sampling_projection.v1",
    }))
}

fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Invalid("invalid chat projection digest".into()));
    }
    Ok(())
}

fn message(role: ChatRole, content: &str) -> Result<Value> {
    text(content, "chat message", 128 * 1024)?;
    Ok(json!({"role": role, "content": content}))
}

fn build_chat_payload_v1(
    provider: &ChatProviderProjectionInput,
    messages: Vec<Value>,
) -> Result<ProjectedChatRequest> {
    digest(&provider.model_digest)?;
    identifier(&provider.provider_model)?;
    if provider.max_output_tokens == 0 || provider.max_output_tokens > 32768 || messages.is_empty()
    {
        return Err(Error::Invalid("invalid chat projection bounds".into()));
    }
    let payload = json!({
        "model": provider.provider_model,
        "messages": messages,
        "max_tokens": provider.max_output_tokens,
        "temperature": sampling_projection_v1(provider.sampling_profile).temperature,
    });
    let payload_digest = canonical_fingerprint(&json!({
        "algorithm_version": "rsia.chat_payload.sha256.cjson.v1",
        "payload": payload,
        "protocol_id": provider.protocol_id,
        "schema_version": "rsia.chat_payload_projection.v1",
    }))?;
    Ok(ProjectedChatRequest {
        wire_utf8: serde_json::to_vec(&payload).map_err(|_| Error::Internal)?,
        sampling_digest: sampling_digest_v1(provider.protocol_id, provider.sampling_profile)?,
        payload_digest,
        protocol_id: provider.protocol_id,
    })
}

pub fn project_bound_chat_v1(
    request: &ModelRequest,
    provider: &ChatProviderProjectionInput,
) -> Result<ProjectedChatRequest> {
    request.validate()?;
    if request.model_digest != provider.model_digest {
        return Err(Error::Forbidden);
    }
    if request.sampling_digest
        != sampling_digest_v1(provider.protocol_id, provider.sampling_profile)?
    {
        return Err(Error::Conflict("model_sampling_projection_mismatch".into()));
    }
    let messages = request
        .input
        .iter()
        .map(|part| {
            message(
                match part.role {
                    ModelInputRole::System => ChatRole::System,
                    ModelInputRole::User | ModelInputRole::Evidence => ChatRole::User,
                },
                &part.content,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    build_chat_payload_v1(provider, messages)
}

pub fn project_formal_chat_v1(
    mandatory_context: &str,
    projection: &RunProjection,
    task_input_utf8: &str,
    provider: &ChatProviderProjectionInput,
) -> Result<ProjectedChatRequest> {
    digest(&projection.bundle_digest)?;
    let mut messages = vec![message(ChatRole::System, mandatory_context)?];
    for instruction in &projection.instructions {
        messages.push(message(ChatRole::System, instruction)?);
    }
    messages.push(message(ChatRole::User, task_input_utf8)?);
    build_chat_payload_v1(provider, messages)
}

pub fn verify_projection_v1(
    expected_protocol: ModelProtocolId,
    expected_sampling_digest: &str,
    expected_payload_digest: &str,
    actual: &ProjectedChatRequest,
) -> Result<()> {
    digest(expected_sampling_digest)?;
    digest(expected_payload_digest)?;
    if expected_protocol != actual.protocol_id || expected_sampling_digest != actual.sampling_digest
    {
        return Err(Error::Conflict("model_sampling_projection_mismatch".into()));
    }
    if expected_payload_digest != actual.payload_digest {
        return Err(Error::Conflict("model_payload_projection_mismatch".into()));
    }
    Ok(())
}
