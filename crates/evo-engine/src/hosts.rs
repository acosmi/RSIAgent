//! Extra host adapters and trusted host-use verification.
//!
//! Surface fixtures are useful for structural compatibility checks, but they are
//! not execution authority. A use result is derived only from the live E06 run
//! snapshot and its persisted application and execution receipts.
use crate::release_store::{
    HOST_APPLICATION_SCHEMA, HostApplicationRecord, ReleaseStore, TrustedHostExecutionReceipt,
};
use evo_core::contract::{HostSurfaceManifest, SurfaceCoverage, SurfaceItem};
use evo_core::{Context, Error, Result, Role, identifier};
use evo_storage::Store;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const CLAUDE_CODE_TARGET: &str = "claude-code-mcp-tool-only";
pub const CLAUDE_CODE_ADAPTER_VERSION: &str = "0.2.0";
pub const CLAUDE_CODE_SOURCE_DIGEST: &str = "claude_code_mcp_tool_only_v1_fixed_spec";

/// Claude Code has no fixed, smoke-tested host version in this repository.
/// Merely finding a binary is deliberately insufficient.
pub fn claude_code_support(binary: Option<&Path>) -> Result<&'static str> {
    let detail = match binary {
        Some(path) if path.is_file() => {
            "binary found, but fixed version, handshake, and real smoke receipt are absent"
        }
        _ => "binary is absent and fixed version, handshake, and real smoke receipt are absent",
    };
    Err(Error::Invalid(format!(
        "E16.4 blocked: Claude Code host unsupported: {detail}"
    )))
}

/// Untrusted candidate used only to compare a checked-in fixture with an
/// extraction. Registering this value as supported is forbidden until a real
/// fixed-version smoke run exists.
pub fn unverified_claude_code_surface_candidate() -> HostSurfaceManifest {
    HostSurfaceManifest {
        schema_version: evo_core::contract::SURFACE_SCHEMA.into(),
        host: CLAUDE_CODE_TARGET.into(),
        host_version: "unverified-no-fixed-version".into(),
        adapter_version: CLAUDE_CODE_ADAPTER_VERSION.into(),
        source_digest: CLAUDE_CODE_SOURCE_DIGEST.into(),
        items: vec![
            SurfaceItem {
                name: "evo_prepare".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("skill.required_capabilities".into()),
                consumer: Some("runner".into()),
                reason: "candidate MCP tool mapping; not a support declaration".into(),
            },
            SurfaceItem {
                name: "evo_feedback".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("skill.counterexample".into()),
                consumer: Some("runner".into()),
                reason: "candidate MCP tool mapping; not a support declaration".into(),
            },
            SurfaceItem {
                name: "evo_propose".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("skill.content".into()),
                consumer: Some("runner".into()),
                reason: "candidate MCP tool mapping; not a support declaration".into(),
            },
            SurfaceItem {
                name: "evo_inspect".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("skill.applicability".into()),
                consumer: Some("runner".into()),
                reason: "candidate MCP tool mapping; not a support declaration".into(),
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
pub enum VerifiedHostStage {
    Offered,
    Attached,
    Used,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedHostUse {
    pub run_id: String,
    pub application_record_id: String,
    pub execution_receipt_id: Option<String>,
    pub stage: VerifiedHostStage,
    pub used_ids: Vec<String>,
    pub benefit_verified: bool,
}

/// Reload and validate the authoritative E06 closure for a run.
///
/// Stage and used IDs come from persisted records. Live snapshot validation
/// also rechecks the release, E05 report view, source closure, environment,
/// and revoke watermark.
pub async fn verify_host_receipt(
    ctx: &Context,
    store: &Store,
    run_id: &str,
) -> Result<VerifiedHostUse> {
    ctx.require(&[Role::Host, Role::Admin])?;
    identifier(run_id)?;
    let mut session = store.session().await?;
    let snapshot =
        ReleaseStore::validate_run_snapshot_live_in_session(ctx, &mut session, run_id).await?;
    let application_id = format!("host-application-{run_id}");
    let application: HostApplicationRecord = session.need(ctx, "receipt", &application_id).await?;
    if application.id != application_id
        || application.schema_version != HOST_APPLICATION_SCHEMA
        || application.run_id != run_id
        || application.release_id != snapshot.release_id
        || application.actual_request_digest != snapshot.request_digest
    {
        return Err(Error::Conflict(
            "stored host application differs from the live run snapshot".into(),
        ));
    }

    let receipt = application
        .receipt
        .as_ref()
        .ok_or_else(|| Error::Conflict("host application contains no applied receipt".into()))?;
    receipt.validate()?;
    if receipt.request_digest != snapshot.request_digest
        || Some(receipt.bundle_digest.as_str()) != snapshot.bundle_digest.as_deref()
        || !receipt.verified_benefit.is_empty()
    {
        return Err(Error::Conflict(
            "host application claims facts outside the frozen E06 closure".into(),
        ));
    }

    let stage = if !receipt.used.is_empty() {
        VerifiedHostStage::Used
    } else if !receipt.attached.is_empty() {
        VerifiedHostStage::Attached
    } else if !receipt.offered.is_empty() {
        VerifiedHostStage::Offered
    } else {
        return Err(Error::Conflict(
            "host application has no attributable offered, attached, or used IDs".into(),
        ));
    };

    let execution_receipt_id = application.execution_receipt_id.clone();
    if stage == VerifiedHostStage::Used {
        let execution_id = execution_receipt_id.as_deref().ok_or_else(|| {
            Error::Conflict("used application lacks a trusted execution receipt".into())
        })?;
        let execution: TrustedHostExecutionReceipt =
            session.need(ctx, "artifact", execution_id).await?;
        if execution.id != execution_id
            || execution.schema_version != "rsia.host_execution_receipt.v1"
            || execution.run_id != run_id
            || execution.request_digest != snapshot.request_digest
            || execution.environment_digest != snapshot.environment_digest
            || execution.host_surface_digest != snapshot.host_surface_digest
            || !receipt
                .used
                .iter()
                .all(|used| execution.used_ids.contains(used))
        {
            return Err(Error::Conflict(
                "trusted execution receipt does not prove the live run's actual used IDs".into(),
            ));
        }
    } else if execution_receipt_id.is_some() {
        return Err(Error::Conflict(
            "non-used application cannot attach an execution receipt as authority".into(),
        ));
    }

    let used_ids = receipt.used.clone();
    session.commit().await?;
    Ok(VerifiedHostUse {
        run_id: run_id.into(),
        application_record_id: application_id,
        execution_receipt_id,
        stage,
        used_ids,
        benefit_verified: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn claude_code_is_blocked_without_real_smoke() {
        assert!(claude_code_support(None).is_err());
        assert!(claude_code_support(Some(&PathBuf::from("/definitely/not/claude"))).is_err());
    }

    #[test]
    fn candidate_surface_only_supports_structural_drift_checks() {
        let manifest = unverified_claude_code_surface_candidate();
        let extracted: Vec<String> = manifest
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect();
        assert!(detect_surface_drift(&manifest, &extracted).is_ok());
        assert!(manifest.host_version.starts_with("unverified-"));
    }
}
