//! MCP adapter; management capabilities are not model tools.
use evo_core::Result;
use evo_core::contract::{
    MODEL_TOOLS, V1_TOOL_DESCRIPTOR_MAX, model_tool_names, reject_admin_as_model_tool,
};
use serde_json::{Value, json};

pub const TOOL_COUNT: usize = 4;

pub fn model_tools() -> &'static [&'static str] {
    model_tool_names()
}

pub fn tool_descriptors() -> Result<Vec<Value>> {
    let tools = MODEL_TOOLS
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "description": "RSIAgent v1 model tool",
                "inputSchema": {"type": "object"}
            })
        })
        .collect::<Vec<_>>();
    for tool in &tools {
        let bytes = serde_json::to_vec(tool).map_err(|_| evo_core::Error::Internal)?;
        if bytes.len() > V1_TOOL_DESCRIPTOR_MAX {
            return Err(evo_core::Error::Invalid(
                "v1 tool descriptor exceeds 2000-byte commitment".into(),
            ));
        }
    }
    Ok(tools)
}

pub fn reject_if_admin(name: &str) -> Result<()> {
    reject_admin_as_model_tool(name)
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
    }
}
