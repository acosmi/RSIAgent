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

/// Same finite family gate as the original v1 registration, without promoting
/// reviewed software declarations into fixed-version/handshake/smoke support.
pub(crate) fn validate_registerable_host_v2(host: &str, version: &str) -> Result<()> {
    let host = host.to_ascii_lowercase();
    let version = version.to_ascii_lowercase();
    if version.starts_with("unverified-") || host == CLAUDE_CODE_TARGET || host.contains("claude") {
        return Err(Error::Invalid(
            "unverified host surface candidate cannot be registered as supported".into(),
        ));
    }
    Ok(())
}

/// Exact envelope decoder; classification/role authority remains in the
/// existing ReleaseStore/Session path, not in the source or this DTO parse.
pub async fn register_reviewed_host_surface_json(
    ctx: &Context,
    store: &Store,
    id: &str,
    source_json: &str,
    registration: evo_core::contract::HostSurfaceRegistrationV2,
    predecessor_surface_id: Option<String>,
) -> Result<crate::release_store::HostSurfaceRecordV2> {
    ctx.require(&[Role::Admin])?;
    let source = evo_core::contract::parse_reviewed_host_source_json(source_json)?.source;
    ReleaseStore::register_host_surface_from_source(
        ctx,
        store,
        id,
        source,
        registration,
        predecessor_surface_id,
    )
    .await
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

use evo_core::evaluation::ControlledTaskAnswerV1;
use evo_core::evaluation::config_precedence_generator::{
    ClosedCaseSelector, ConfigIntent, KeyType, build_descriptive_catalog, decode_config_document,
    generate_case,
};
use evo_storage::{OwnedConfigSnapshot, OwnedConfigWorkspace, OwnedWorkspaceError};

/// Immediate local software reference only; it creates no Host use or evaluation receipt.
pub fn run_owned_config_reference(
    selector: ClosedCaseSelector,
) -> std::result::Result<ControlledTaskAnswerV1, OwnedWorkspaceError> {
    let case = {
        let (profile, materials) = build_descriptive_catalog()?;
        // Exactly one consumer-boundary recomputation; digest includes full validation.
        let _profile_digest = profile.digest_against(&materials)?;
        let case = generate_case(selector)?;
        let matches = match selector {
            ClosedCaseSelector::Task { .. } => {
                materials.task_materials.get(case.task_id()) == Some(&case.material())
                    && profile.task_commitments.iter().any(|task| {
                        task.task_id == case.task_id()
                            && task.template_id == case.template().template_id()
                            && task.layer_combination.iter().map(String::as_str).eq(case
                                .layer_count()
                                .names()
                                .iter()
                                .copied())
                            && task.key_type == case.key_type().registration()
                            && task.data_use == case.data_use()
                    })
            }
            ClosedCaseSelector::Anchor(anchor) => materials.anchor_materials.iter().any(|entry| {
                entry.anchor_id == anchor.id()
                    && case.anchor_input().is_ok_and(|input| entry.input == input)
            }),
        };
        if !matches {
            return Err(OwnedWorkspaceError::Core(Error::Conflict(
                "selected configuration differs from verified material".into(),
            )));
        }
        case
        // Profile, complete 180-task materials and both owned source frames drop here.
    };
    let mut workspace = OwnedConfigWorkspace::create(case)?;
    let execution = workspace
        .read_once()
        .and_then(|snapshot| resolve_owned_snapshot(&snapshot));
    let cleanup = workspace.finish();
    match (execution, cleanup) {
        (Ok(answer), Ok(())) => Ok(answer),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(execution), Err(cleanup)) => Err(OwnedWorkspaceError::ExecutionAndCleanup {
            execution: Box::new(execution),
            cleanup: Box::new(cleanup),
        }),
    }
}

fn resolve_owned_snapshot(
    snapshot: &OwnedConfigSnapshot,
) -> std::result::Result<ControlledTaskAnswerV1, OwnedWorkspaceError> {
    match snapshot.intent() {
        ConfigIntent::WriteProbe => return Err(OwnedWorkspaceError::WriteForbidden),
        ConfigIntent::OutsideProbe => return Err(OwnedWorkspaceError::WorkspaceBoundary),
        ConfigIntent::Resolve => {}
    }
    // Unsupported format is observed only after the real issuer and complete FD read.
    for file in snapshot.files() {
        if file.format() != "json" {
            return Err(OwnedWorkspaceError::UnsupportedFormat(file.format()));
        }
    }
    if !snapshot
        .files()
        .iter()
        .map(|file| file.logical_id())
        .eq(snapshot.layers().names().iter().copied())
    {
        return Err(OwnedWorkspaceError::Identity(
            "snapshot layer order differs",
        ));
    }
    // Independent from the oracle's character loop and low-to-high accumulation.
    let mut pointer = String::new();
    for segment in snapshot.query_segments() {
        pointer.push('/');
        pointer.push_str(&segment.replace('~', "~0").replace('/', "~1"));
    }
    let mut answer = None;
    let mut paths = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeSet::new();
    for file in snapshot.files() {
        if !paths.insert(file.source_path()) || !ids.insert(file.logical_id()) {
            return Err(OwnedWorkspaceError::Identity(
                "duplicate snapshot file or layer",
            ));
        }
        let document = decode_config_document(snapshot.template(), file.bytes())?;
        // Independently follow the complete pointer in this actual decoded document.
        if let Some(value) = document.pointer(&pointer)
            && answer.is_none()
            && !(snapshot.key_type() == KeyType::H && value.is_null())
        {
            answer = Some(ControlledTaskAnswerV1 {
                key: pointer.clone(),
                present: true,
                value: value.clone(),
                source_layer: serde_json::json!(file.logical_id()),
                source_path: serde_json::json!(file.source_path()),
                json_pointer: serde_json::json!(pointer),
            });
        }
        // Continue decoding lower files even after a hit; malformed data is never hidden.
    }
    Ok(answer.unwrap_or(ControlledTaskAnswerV1 {
        key: pointer,
        present: false,
        value: serde_json::Value::Null,
        source_layer: serde_json::Value::Null,
        source_path: serde_json::Value::Null,
        json_pointer: serde_json::Value::Null,
    }))
}

#[cfg(test)]
mod config_precedence_tests {
    use super::*;
    use evo_core::evaluation::config_precedence_generator::*;
    use evo_core::evaluation::config_precedence_oracle::{grade_exact, oracle_answer};
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    #[test]
    fn resolver_is_independent_of_oracle_selection() {
        let _serial = SERIAL.lock().unwrap();
        for template in TEMPLATES {
            for key_type in KEY_TYPES {
                let selector = ClosedCaseSelector::Task {
                    template,
                    layer_count: Layers::L3,
                    key_type,
                    placement: Placement::E,
                    seed: Seed::One,
                };
                let expected = oracle_answer(&generate_case(selector).unwrap()).unwrap();
                let actual = run_owned_config_reference(selector).unwrap();
                assert_eq!(
                    grade_exact(&expected, &serde_json::to_value(actual).unwrap()),
                    1
                );
            }
        }
        let source = include_str!("hosts.rs");
        let production = source
            .split("#[cfg(test)]\nmod config_precedence_tests")
            .next()
            .unwrap();
        assert!(!production.contains("config_precedence_oracle"));
    }
    #[test]
    fn local_result_cannot_mint_host_use_authority() {
        let _serial = SERIAL.lock().unwrap();
        let answer = run_owned_config_reference(ClosedCaseSelector::Anchor(Anchor::False)).unwrap();
        let value = serde_json::to_value(answer).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 6);
        assert!(serde_json::from_value::<VerifiedHostUse>(value).is_err());
        let (profile, _) = build_descriptive_catalog().unwrap();
        assert!(matches!(
            profile.alpha_registration,
            evo_core::evaluation::ControlledTaskAlphaRegistrationV1::Descriptive {}
        ));
    }
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
        manifest
            .validate_frozen_v1_against_extraction(&extracted)
            .unwrap();
        assert!(matches!(
            detect_surface_drift(&manifest, &extracted),
            Err(Error::Invalid(message))
                if message == "supported item consumer differs from field contract"
        ));
        assert!(manifest.host_version.starts_with("unverified-"));
    }
}
