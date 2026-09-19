//! Authenticated HTTP adapter for the shared `HostService`.
//!
//! Tokens are hashed at trusted startup and map to an immutable Context.
//! Request headers and bodies cannot select namespace, actor or role.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use evo_core::{Context, Error, Feedback, Inspect, Prepare, Proposal, Role, hash, identifier};
use evo_engine::evidence::StoredTraceAuthority;
use evo_engine::release_store::{AppliedRequestMaterial, TrustedHostExecutionEvidence};
use evo_engine::service::{HostPrepareConfig, HostService};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeOwned, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};
use tower_http::limit::RequestBodyLimitLayer;

pub const DEFAULT_BIND: &str = "127.0.0.1:7788";
pub const MAX_HTTP_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct AuthIdentity {
    namespace: String,
    actor: String,
    role: Role,
}

impl AuthIdentity {
    pub fn new(namespace: &str, actor: &str, role: Role) -> evo_core::Result<Self> {
        let context = Context::new(namespace, actor, role)?;
        Ok(Self {
            namespace: context.namespace().into(),
            actor: context.actor().into(),
            role: context.role(),
        })
    }

    fn context(&self) -> evo_core::Result<Context> {
        Context::new(&self.namespace, &self.actor, self.role)
    }
}

#[derive(Debug, Clone, Default)]
pub struct AuthRegistry {
    identities: Arc<BTreeMap<String, AuthIdentity>>,
}

impl AuthRegistry {
    pub fn from_plaintext(entries: Vec<(String, AuthIdentity)>) -> evo_core::Result<Self> {
        let mut identities = BTreeMap::new();
        for (token, identity) in entries {
            if token.len() < 16 || token.len() > 4096 || token.contains(['\0', '\n', '\r']) {
                return Err(Error::Invalid(
                    "auth token must be 16..=4096 bytes without control separators".into(),
                ));
            }
            let digest = hash(token.as_bytes());
            if identities.insert(digest, identity).is_some() {
                return Err(Error::Conflict("duplicate auth token".into()));
            }
        }
        Ok(Self {
            identities: Arc::new(identities),
        })
    }

    pub fn is_configured(&self) -> bool {
        !self.identities.is_empty()
    }

    fn authenticate(&self, headers: &HeaderMap) -> std::result::Result<Context, HttpError> {
        if !self.is_configured() {
            return Err(HttpError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "authentication_not_configured",
            ));
        }
        for forbidden in ["x-role", "x-namespace", "x-actor"] {
            if headers.contains_key(forbidden) {
                return Err(HttpError::new(
                    StatusCode::FORBIDDEN,
                    "identity_headers_forbidden",
                ));
            }
        }
        let raw = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| HttpError::new(StatusCode::UNAUTHORIZED, "bearer_token_required"))?;
        let token = raw
            .strip_prefix("Bearer ")
            .filter(|token| !token.is_empty())
            .ok_or_else(|| HttpError::new(StatusCode::UNAUTHORIZED, "bearer_token_required"))?;
        let identity = self
            .identities
            .get(&hash(token.as_bytes()))
            .ok_or_else(|| HttpError::new(StatusCode::UNAUTHORIZED, "invalid_bearer_token"))?;
        identity.context().map_err(HttpError::from)
    }
}

#[derive(Clone)]
pub struct HttpState {
    pub service: HostService,
    pub prepare_config: HostPrepareConfig,
    pub auth: AuthRegistry,
}

pub fn router(state: HttpState) -> Router {
    Router::new()
        .route("/v1/tools/prepare", post(prepare))
        .route("/v1/tools/feedback", post(feedback))
        .route("/v1/tools/propose", post(propose))
        .route("/v1/tools/inspect", post(inspect))
        .route("/v1/host/trace", post(record_trace))
        .route("/v1/host/snapshot", post(read_snapshot))
        .route("/v1/host/application", post(record_application))
        .route("/v1/manage/{operation}", post(management))
        .layer(RequestBodyLimitLayer::new(MAX_HTTP_BODY_BYTES))
        .with_state(state)
}

async fn prepare(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<Value>, HttpError> {
    let caller = state.auth.authenticate(&headers)?;
    let request: Prepare = strict_typed(&body, "invalid_prepare_payload")?;
    let result = state
        .service
        .prepare(&caller, request, &state.prepare_config)
        .await?;
    to_json(result)
}

async fn feedback(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<Value>, HttpError> {
    let caller = state.auth.authenticate(&headers)?;
    let request: Feedback = strict_typed(&body, "invalid_feedback_payload")?;
    to_json(state.service.feedback(&caller, request).await?)
}

async fn propose(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<Value>, HttpError> {
    let caller = state.auth.authenticate(&headers)?;
    let request: Proposal = strict_typed(&body, "invalid_proposal_payload")?;
    to_json(state.service.propose(&caller, request).await?)
}

async fn inspect(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<Value>, HttpError> {
    let caller = state.auth.authenticate(&headers)?;
    let request: Inspect = strict_typed(&body, "invalid_inspect_payload")?;
    Ok(Json(state.service.inspect(&caller, request).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostApplicationInput {
    pub run_id: String,
    pub actual_request_material: AppliedRequestMaterial,
    pub environment_digest: String,
    pub host_surface_digest: String,
    pub host_capabilities_digest: String,
    #[serde(default)]
    pub offered: Vec<String>,
    #[serde(default)]
    pub attached: Vec<String>,
    #[serde(default)]
    pub used: Vec<String>,
    pub execution_receipt_id: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostSnapshotInput {
    run_id: String,
}

async fn read_snapshot(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<Value>, HttpError> {
    let caller = state.auth.authenticate(&headers)?;
    let input: HostSnapshotInput = strict_typed(&body, "invalid_snapshot_payload")?;
    to_json(
        state
            .service
            .read_run_snapshot(&caller, &input.run_id)
            .await?,
    )
}

async fn record_trace(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<Value>, HttpError> {
    let caller = state.auth.authenticate(&headers)?;
    let authority: StoredTraceAuthority = strict_typed(&body, "invalid_trace_payload")?;
    state.service.record_trace(&caller, &authority).await?;
    Ok(Json(json!({"id":authority.record.id,"stored":true})))
}

async fn record_application(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<Value>, HttpError> {
    let caller = state.auth.authenticate(&headers)?;
    let input: HostApplicationInput = strict_typed(&body, "invalid_application_payload")?;
    identifier(&input.run_id)?;
    let result = state
        .service
        .record_application(
            &caller,
            &input.run_id,
            TrustedHostExecutionEvidence {
                actual_request_material: input.actual_request_material,
                environment_digest: input.environment_digest,
                host_surface_digest: input.host_surface_digest,
                host_capabilities_digest: input.host_capabilities_digest,
                offered: input.offered,
                attached: input.attached,
                used: input.used,
                execution_receipt_id: input.execution_receipt_id,
                truncated: input.truncated,
            },
        )
        .await?;
    to_json(result)
}

async fn management(
    State(state): State<HttpState>,
    Path(operation): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<Value>, HttpError> {
    let caller = state.auth.authenticate(&headers)?;
    let body = parse_unique_value(&body, "invalid_management_payload")?;
    reject_body_identity(&body)?;
    caller.require(&[Role::Admin])?;
    if !evo_core::contract::admin_ops().contains(&operation.as_str()) {
        return Err(HttpError::new(
            StatusCode::NOT_FOUND,
            "unknown_management_operation",
        ));
    }
    Err(HttpError::new(
        StatusCode::NOT_IMPLEMENTED,
        "unsupported_pending_real_evaluation_backend",
    ))
}

fn reject_body_identity(body: &Value) -> std::result::Result<(), HttpError> {
    for key in ["actor", "role", "namespace"] {
        if body.get(key).is_some() {
            return Err(HttpError::new(
                StatusCode::FORBIDDEN,
                "body_identity_forbidden",
            ));
        }
    }
    Ok(())
}

fn strict_typed<T>(body: &[u8], code: &'static str) -> std::result::Result<T, HttpError>
where
    T: DeserializeOwned,
{
    let value = parse_unique_value(body, code)?;
    reject_body_identity(&value)?;
    serde_json::from_slice(body).map_err(|_| HttpError::new(StatusCode::BAD_REQUEST, code))
}

fn parse_unique_value(body: &[u8], code: &'static str) -> std::result::Result<Value, HttpError> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let value = UniqueValue::deserialize(&mut deserializer)
        .map_err(|_| HttpError::new(StatusCode::BAD_REQUEST, code))?;
    deserializer
        .end()
        .map_err(|_| HttpError::new(StatusCode::BAD_REQUEST, code))?;
    Ok(value.0)
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueValueVisitor)
    }
}

struct UniqueValueVisitor;

impl<'de> Visitor<'de> for UniqueValueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON with unique object keys")
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value.into())))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueValue>()? {
            values.push(value.0);
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate JSON key: {key}"
                )));
            }
            let value = object.next_value::<UniqueValue>()?;
            values.insert(key, value.0);
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}

fn to_json<T: Serialize>(value: T) -> std::result::Result<Json<Value>, HttpError> {
    serde_json::to_value(value)
        .map(Json)
        .map_err(|_| HttpError::from(Error::Internal))
}

#[derive(Debug)]
struct HttpError {
    status: StatusCode,
    code: &'static str,
}

impl HttpError {
    fn new(status: StatusCode, code: &'static str) -> Self {
        Self { status, code }
    }
}

impl From<Error> for HttpError {
    fn from(error: Error) -> Self {
        match error {
            Error::Invalid(_) => Self::new(StatusCode::BAD_REQUEST, "invalid_input"),
            Error::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden"),
            Error::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found"),
            Error::Conflict(_) => Self::new(StatusCode::CONFLICT, "conflict"),
            Error::Budget => Self::new(StatusCode::TOO_MANY_REQUESTS, "budget_unavailable"),
            Error::Cancelled => Self::new(StatusCode::CONFLICT, "cancelled"),
            Error::Internal => Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error":{"code":self.code}}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_hashes_tokens_and_rejects_short_secrets() {
        let identity = AuthIdentity::new("n", "a", Role::Agent).unwrap();
        assert!(AuthRegistry::from_plaintext(vec![("short".into(), identity.clone())]).is_err());
        let registry =
            AuthRegistry::from_plaintext(vec![("0123456789abcdef".into(), identity)]).unwrap();
        assert!(registry.is_configured());
        assert!(!registry.identities.contains_key("0123456789abcdef"));
    }

    #[test]
    fn body_cannot_mint_admin() {
        assert!(reject_body_identity(&json!({"actor":"root","role":"admin"})).is_err());
        assert!(reject_body_identity(&json!({"goal":"x"})).is_ok());
    }
}
