//! MCP stdio adapter. Its model identity is fixed during trusted process startup.
//! Management, approval, budget and evaluation operations are never registered
//! as model tools.
//!
//! A bounded line pump recursively rejects duplicate keys before handing raw
//! frames to rmcp, so typed tool dispatch never sees a last-wins JSON object.

use evo_core::contract::{
    MODEL_TOOLS, V1_TOOL_DESCRIPTOR_MAX, model_tool_names, reject_admin_as_model_tool,
};
use evo_core::{Context, Error, Feedback, Inspect, Prepare, Proposal, Result as CoreResult, Role};
use evo_engine::service::{HostPrepareConfig, HostService};
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    handler::server::{
        tool::{ToolCallContext, ToolRouter},
        wrapper::Parameters,
    },
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool, tool_router,
};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::{Number, Value, json};
use std::{collections::BTreeSet, fmt};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub const TOOL_COUNT: usize = 4;
pub const MCP_MAX_FRAME_BYTES: usize = 64 * 1024;

pub fn model_tools() -> &'static [&'static str] {
    model_tool_names()
}

pub fn reject_if_admin(name: &str) -> CoreResult<()> {
    reject_admin_as_model_tool(name)
}

#[derive(Clone)]
pub struct McpHost {
    service: HostService,
    caller: Context,
    prepare_config: HostPrepareConfig,
    tool_router: ToolRouter<Self>,
}

#[tool_router(router = tool_router)]
impl McpHost {
    pub fn new(
        service: HostService,
        caller: Context,
        prepare_config: HostPrepareConfig,
    ) -> CoreResult<Self> {
        caller.require(&[Role::Agent])?;
        if caller.namespace() != service.trusted_namespace() {
            return Err(Error::Forbidden);
        }
        prepare_config.validate()?;
        Ok(Self {
            service,
            caller,
            prepare_config,
            tool_router: Self::tool_router(),
        })
    }
    #[tool(
        name = "evo_prepare",
        description = "Prepare a fixed run snapshot and return only approved active skills"
    )]
    async fn evo_prepare(&self, Parameters(request): Parameters<Prepare>) -> CallToolResult {
        match self
            .service
            .prepare(&self.caller, request, &self.prepare_config)
            .await
        {
            Ok(value) => structured(value),
            Err(error) => structured_error(error),
        }
    }

    #[tool(
        name = "evo_feedback",
        description = "Record self-reported run feedback without treating it as independent scoring"
    )]
    async fn evo_feedback(&self, Parameters(request): Parameters<Feedback>) -> CallToolResult {
        match self.service.feedback(&self.caller, request).await {
            Ok(value) => structured(value),
            Err(error) => structured_error(error),
        }
    }

    #[tool(
        name = "evo_propose",
        description = "Submit a bounded proposal for validation; this never approves or releases it"
    )]
    async fn evo_propose(&self, Parameters(request): Parameters<Proposal>) -> CallToolResult {
        match self.service.propose(&self.caller, request).await {
            Ok(value) => structured(value),
            Err(error) => structured_error(error),
        }
    }

    #[tool(
        name = "evo_inspect",
        description = "Inspect an authorized redacted object in the caller's fixed scope"
    )]
    async fn evo_inspect(&self, Parameters(request): Parameters<Inspect>) -> CallToolResult {
        match self.service.inspect(&self.caller, request).await {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => structured_error(error),
        }
    }
}

impl ServerHandler for McpHost {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("rsia", "0.2.0"))
            .with_instructions(
                "Four bounded RSIA model tools. Identity is fixed by trusted startup configuration.",
            )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, rmcp::ErrorData> {
        self.tool_router
            .call(ToolCallContext::new(self, request, context))
            .await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            ..Default::default()
        })
    }
}

pub fn tool_descriptors() -> CoreResult<Vec<Value>> {
    let tools = McpHost::tool_router().list_all();
    if tools.len() != TOOL_COUNT
        || !tools
            .iter()
            .all(|tool| MODEL_TOOLS.contains(&tool.name.as_ref()))
    {
        return Err(Error::Internal);
    }
    tools
        .into_iter()
        .map(|tool| {
            let value = serde_json::to_value(tool).map_err(|_| Error::Internal)?;
            let bytes = serde_json::to_vec(&value).map_err(|_| Error::Internal)?;
            if bytes.len() > V1_TOOL_DESCRIPTOR_MAX {
                return Err(Error::Invalid(
                    "v1 tool descriptor exceeds 2000-byte commitment".into(),
                ));
            }
            Ok(value)
        })
        .collect()
}

pub async fn serve_stdio(
    server: McpHost,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_strict(server, tokio::io::stdin(), tokio::io::stdout()).await
}

pub async fn serve_strict<R, W>(
    server: McpHost,
    reader: R,
    writer: W,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let (validated_reader, validated_writer) = tokio::io::duplex(MCP_MAX_FRAME_BYTES + 1);
    let pump = tokio::spawn(pump_strict_frames(reader, validated_writer));
    let service = match server.serve((validated_reader, writer)).await {
        Ok(service) => service,
        Err(error) => {
            let pump_result = pump.await?;
            if let Err(pump_error) = pump_result {
                return Err(Box::new(pump_error));
            }
            return Err(Box::new(error));
        }
    };
    let (pump_result, service_result) = tokio::join!(pump, service.waiting());
    pump_result??;
    service_result?;
    Ok(())
}

async fn pump_strict_frames<R, W>(reader: R, mut writer: W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut frame = Vec::with_capacity(8 * 1024);
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if frame.is_empty() {
                writer.shutdown().await?;
                return Ok(());
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unterminated MCP JSON frame",
            ));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if frame.len().saturating_add(take) > MCP_MAX_FRAME_BYTES + 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MCP JSON frame exceeds 65536 bytes",
            ));
        }
        frame.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_none() {
            continue;
        }
        let line = frame.strip_suffix(b"\n").unwrap_or(&frame);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            frame.clear();
            continue;
        }
        validate_unique_json(line)?;
        writer.write_all(line).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        frame.clear();
    }
}

fn validate_unique_json(frame: &[u8]) -> std::io::Result<()> {
    let mut deserializer = serde_json::Deserializer::from_slice(frame);
    UniqueValue::deserialize(&mut deserializer)
        .and_then(|_| deserializer.end())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

struct UniqueValue;

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
        Ok(UniqueValue)
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue)
    }

    fn visit_bool<E>(self, _value: bool) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue)
    }

    fn visit_i64<E>(self, _value: i64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue)
    }

    fn visit_u64<E>(self, _value: u64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue)
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(|_| UniqueValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, _value: &str) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue)
    }

    fn visit_string<E>(self, _value: String) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue)
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<UniqueValue>()?.is_some() {}
        Ok(UniqueValue)
    }

    fn visit_map<A>(self, mut object: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate JSON key: {key}"
                )));
            }
            object.next_value::<UniqueValue>()?;
        }
        Ok(UniqueValue)
    }
}

fn structured<T: Serialize>(value: T) -> CallToolResult {
    match serde_json::to_value(value) {
        Ok(value) => CallToolResult::structured(value),
        Err(_) => structured_error(Error::Internal),
    }
}

fn structured_error(error: Error) -> CallToolResult {
    let code = match error {
        Error::Invalid(_) => "invalid_input",
        Error::Forbidden => "forbidden",
        Error::NotFound => "not_found",
        Error::Conflict(_) => "conflict",
        Error::Budget => "budget_unavailable",
        Error::Cancelled => "cancelled",
        Error::Internal => "internal_error",
    };
    CallToolResult::structured_error(json!({"error":{"code":code}}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_tools_within_v1_byte_budget() {
        assert_eq!(TOOL_COUNT, 4);
        let tools = tool_descriptors().unwrap();
        assert_eq!(tools.len(), 4);
        assert!(reject_if_admin("evaluation.start").is_err());
        assert!(reject_if_admin("evo_inspect").is_ok());
        for descriptor in tools {
            assert!(serde_json::to_vec(&descriptor).unwrap().len() <= V1_TOOL_DESCRIPTOR_MAX);
        }
    }
}
