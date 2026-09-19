//! Extra host adapters and surface drift protection.
//! Missing real hosts are blocked, not mocked.
use evo_core::contract::{HostSurfaceManifest, SURFACE_SCHEMA, SurfaceCoverage, SurfaceItem};
use evo_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const CLAUDE_CODE_TARGET: &str = "claude-code-mcp-tool-only";
pub const CLAUDE_CODE_PROPOSED_VERSION: &str = "1.0.0";
pub const CLAUDE_CODE_ADAPTER_VERSION: &str = "0.2.0";
pub const CLAUDE_CODE_SOURCE_DIGEST: &str = "claude_code_mcp_tool_only_v1_fixed_spec";

pub fn claude_code_support(binary: Option<&Path>) -> Result<&'static str> {
    match binary {
        Some(p) if p.is_file() => Ok("installed_but_not_yet_smoked"),
        _ => Err(Error::Invalid(
            "E16.4 blocked: Claude Code is not installed; not substituting a mock host".into(),
        )),
    }
}

pub fn claude_code_surface_manifest() -> HostSurfaceManifest {
    HostSurfaceManifest {
        schema_version: SURFACE_SCHEMA.into(),
        host: CLAUDE_CODE_TARGET.into(),
        host_version: CLAUDE_CODE_PROPOSED_VERSION.into(),
        adapter_version: CLAUDE_CODE_ADAPTER_VERSION.into(),
        source_digest: CLAUDE_CODE_SOURCE_DIGEST.into(),
        items: vec![
            SurfaceItem {
                name: "evo_prepare".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("skill.required_capabilities".into()),
                consumer: Some("runner".into()),
                reason: "prepare tool exposed via MCP tools endpoint".into(),
            },
            SurfaceItem {
                name: "evo_feedback".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("skill.counterexample".into()),
                consumer: Some("runner".into()),
                reason: "feedback tool exposed via MCP tools endpoint".into(),
            },
            SurfaceItem {
                name: "evo_propose".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("skill.content".into()),
                consumer: Some("runner".into()),
                reason: "propose tool exposed via MCP tools endpoint".into(),
            },
            SurfaceItem {
                name: "evo_inspect".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("skill.applicability".into()),
                consumer: Some("runner".into()),
                reason: "inspect tool exposed via MCP tools endpoint".into(),
            },
            SurfaceItem {
                name: "shell_tool".into(),
                coverage: SurfaceCoverage::Unsupported,
                mapped_field: None,
                consumer: None,
                reason: "Claude Code internal shell execution is out of scope for tool-only mode"
                    .into(),
            },
            SurfaceItem {
                name: "web_search".into(),
                coverage: SurfaceCoverage::Unsupported,
                mapped_field: None,
                consumer: None,
                reason: "external web search is unverified and out of scope".into(),
            },
            SurfaceItem {
                name: "model_selection".into(),
                coverage: SurfaceCoverage::RuntimeOwned,
                mapped_field: None,
                consumer: None,
                reason: "model selection is host-controlled runtime property".into(),
            },
        ],
    }
}

pub fn detect_surface_drift(
    manifest: &HostSurfaceManifest,
    extracted_fields: &[String],
) -> Result<()> {
    manifest.validate_against_extraction(extracted_fields)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostToolStage {
    Offered,
    Attached,
    Used,
    Truncated,
    Omitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillAttribution {
    NotSelected,
    NotAttached,
    Truncated,
    ExecutionOmitted,
    SkillDefect,
    Uncertain,
    VerifiedBenefit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExecutionReceipt {
    pub host_id: String,
    pub run_id: String,
    pub stage: HostToolStage,
    pub attribution: SkillAttribution,
    pub is_truncated: bool,
    pub is_overridden: bool,
    pub claimed_used: bool,
    pub claimed_benefit: bool,
}

pub fn verify_host_receipt(receipt: &HostExecutionReceipt) -> Result<()> {
    if (receipt.stage == HostToolStage::Truncated || receipt.is_truncated) && receipt.claimed_used {
        return Err(Error::Invalid(
            "receipt_forgery: cannot claim full 'used' when host content was truncated".into(),
        ));
    }
    if receipt.is_overridden && receipt.claimed_used {
        return Err(Error::Invalid(
            "receipt_forgery: cannot claim 'used' when host overrides skill content".into(),
        ));
    }
    if receipt.stage == HostToolStage::Omitted && receipt.claimed_used {
        return Err(Error::Invalid(
            "receipt_forgery: cannot claim 'used' when execution omitted tool invocation".into(),
        ));
    }
    if receipt.attribution != SkillAttribution::VerifiedBenefit && receipt.claimed_benefit {
        return Err(Error::Invalid(
            "receipt_forgery: cannot claim verified benefit without independent verification"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn missing_claude_code_is_blocked() {
        assert!(claude_code_support(None).is_err());
        assert!(claude_code_support(Some(&PathBuf::from("/definitely/not/claude"))).is_err());
    }

    #[test]
    fn surface_manifest_validates_cleanly() {
        let manifest = claude_code_surface_manifest();
        let extracted: Vec<String> = manifest.items.iter().map(|i| i.name.clone()).collect();
        assert!(detect_surface_drift(&manifest, &extracted).is_ok());
    }
}
