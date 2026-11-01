//! Private in-memory Native candidates from authenticated local service facts.
//! These bytes contain prospective scan/review declarations. They are not a
//! review receipt or a deliverable; publication must bind a real Admin decision
//! to these exact bytes after the complete publication checks.

use crate::evidence::{
    SERVICE_FACTS_LICENSE, SERVICE_FACTS_RECIPIENT, ServiceFactSourceV1, load_stored_source,
    service_fact_authority,
};
use crate::import::{NativeTraceExporter, NativeTraceHeader, NativeTraceRedactionReport};
use crate::release_store::ReleaseStore;
use evo_core::evidence::TraceCoverage;
use evo_core::{Context, Error, Result, Role, Run, fingerprint, identifier};
use evo_storage::Session;
use serde_json::json;

/// The actual Git identity recorded when this package was compiled. An unknown
/// or dirty source does not disable readers; it cannot create Native candidates.
pub fn compiled_exporter_identity() -> Result<NativeTraceExporter> {
    let commit = option_env!("RSIA_EXPORTER_COMMIT").unwrap_or("unknown");
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(
            "native exporter requires a clean known build commit".into(),
        ));
    }
    Ok(NativeTraceExporter {
        name: "rsia".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        commit: commit.into(),
    })
}

/// Read one authenticated captured source in the caller's current namespace and
/// transaction. No object, job, reservation, file or delivery receipt is written.
/// The return value is private preparation material, never a download success.
pub async fn prepare_native_candidate(
    ctx: &Context,
    session: &mut Session,
    source_id: &str,
) -> Result<Vec<u8>> {
    ctx.require(&[Role::Admin])?;
    identifier(source_id)?;
    let exporter = compiled_exporter_identity()?;
    let authority = load_stored_source(session, ctx, source_id).await?;
    let raw = std::str::from_utf8(&authority.record.body)
        .map_err(|_| Error::Invalid("invalid service fact source".into()))?;
    let source: ServiceFactSourceV1 = serde_json::from_value(
        evo_storage::lifecycle::parse_strict_json(raw)
            .map_err(|_| Error::Invalid("invalid service fact source".into()))?,
    )
    .map_err(|_| Error::Invalid("invalid service fact source".into()))?;
    if source.schema_version != "rsia.service_fact_source.v1"
        || source.license != SERVICE_FACTS_LICENSE
        || source.recipient_scope != SERVICE_FACTS_RECIPIENT
    {
        return Err(Error::Forbidden);
    }
    let row = session
        .object_row(ctx, "artifact", &source.service_run_id)
        .await?
        .ok_or(Error::NotFound)?;
    let run: Run = serde_json::from_str(&row.body)
        .map_err(|_| Error::Conflict("invalid service run".into()))?;
    if run.id != source.service_run_id
        || row.owner != run.owner
        || run.source != "trusted_host_snapshot"
    {
        return Err(Error::Conflict("service run binding differs".into()));
    }
    let snapshot =
        ReleaseStore::validate_run_snapshot_live_in_session(ctx, session, &source.service_run_id)
            .await?;
    if snapshot.id != run.snapshot_id {
        return Err(Error::Conflict("service run snapshot differs".into()));
    }
    let authenticated = session
        .service_success_facts(ctx, &source.service_run_id)
        .await?;
    if authenticated.iter().any(|(actor, _)| actor != &run.owner) {
        return Err(Error::Conflict(
            "service facts belong to another caller".into(),
        ));
    }
    let prefix = authenticated
        .into_iter()
        .take_while(|(_, fact)| fact.sequence <= source.captured_through_sequence)
        .map(|(_, fact)| fact)
        .collect();
    let expected = service_fact_authority(ctx.namespace(), &source.service_run_id, prefix)?;
    if expected.record.id != source_id || fingerprint(&expected)? != fingerprint(&authority)? {
        return Err(Error::Conflict(
            "service fact source differs from authenticated capture".into(),
        ));
    }
    crate::revocation_gate::ensure_dependencies_live(
        ctx,
        session,
        &[
            ("run".into(), source_id.into()),
            ("artifact".into(), source.service_run_id.clone()),
        ],
        "native trace candidate",
    )
    .await?;

    let events: Vec<_> = source
        .facts
        .iter()
        .map(|fact| {
            json!({
                "sequence": fact.sequence,
                "role": fact.role,
                "kind": fact.kind,
                "content": format!("RSIA committed {} for reference {}", fact.kind, fact.subject_id),
            })
        })
        .collect();
    let header = NativeTraceHeader {
        exporter,
        run_id: source.service_run_id,
        redaction_report: NativeTraceRedactionReport {
            schema_version: "rsia.trace.redaction.v1".into(),
            scan_status: "passed".into(),
            review_status: "reviewed".into(),
            removed_items: 0,
            redacted_items: 0,
        },
        // Only the captured service-fact prefix is known. This is not complete
        // coverage of the service's dialogue, model or Host execution history.
        coverage: TraceCoverage {
            bytes_read: authority.record.body.len() as u64,
            parsed_ok: 1,
            events_seen: events.len() as u64,
            events_exported: events.len() as u64,
            ..Default::default()
        },
    };
    let bytes = serde_json::to_vec(&json!({
        "schema_version": "rsia.trace.v1",
        "exporter": header.exporter,
        "run_id": header.run_id,
        "redaction_report": header.redaction_report,
        "coverage": header.coverage,
        "events": events,
    }))
    .map_err(|_| Error::Internal)?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error::Invalid(
            "native trace candidate exceeds single-file byte bound".into(),
        ));
    }
    Ok(bytes)
}

/// Files and exact tree identity for a future Admin decision. This is private
/// preparation material; it carries no delivery or completed-review receipt.
#[derive(Debug, Clone)]
pub struct NativeCandidateMembers {
    pub files: Vec<evo_storage::LocalExportFile>,
    pub candidate_digest: String,
    pub total_bytes: u64,
}

/// Only scanner rule names and counts leave this boundary. The package scanner's
/// raw snippets, finding details and input paths are never copied to this report.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePrivacyReport {
    pub schema_version: String,
    pub status: crate::packages::RedactionStatus,
    pub findings_by_rule: std::collections::BTreeMap<String, u64>,
    pub disclaimer: String,
}

/// Scan a bounded text-member set. This is policy inspection, not source
/// authentication, review, or publication; the producer below supplies authority.
pub fn scan_native_candidate_members(
    files: &[evo_storage::LocalExportFile],
) -> Result<NativePrivacyReport> {
    if files.iter().any(|file| file.bytes.is_empty()) {
        return Err(Error::Invalid(
            "native candidate member validation failed".into(),
        ));
    }
    let members: Vec<_> = files
        .iter()
        .map(|file| crate::packages::PackageMember {
            path: file.path.clone(),
            size: file.bytes.len() as u64,
            compressed: file.bytes.len() as u64,
        })
        .collect();
    crate::packages::reject_package(&members)
        .map_err(|_| Error::Invalid("native candidate member validation failed".into()))?;
    let mut findings_by_rule = std::collections::BTreeMap::new();
    let mut status = crate::packages::RedactionStatus::Clean;
    for file in files {
        let text = std::str::from_utf8(&file.bytes)
            .map_err(|_| Error::Invalid("native candidate member is not UTF-8".into()))?;
        for input in [file.path.as_str(), text] {
            let scanned = crate::packages::scan_privacy(input)?;
            for finding in scanned.findings {
                if finding.severity == crate::packages::FindingSeverity::Block {
                    status = crate::packages::RedactionStatus::Blocked;
                } else if status == crate::packages::RedactionStatus::Clean {
                    status = crate::packages::RedactionStatus::Warning;
                }
                *findings_by_rule.entry(finding.rule).or_insert(0) += 1;
            }
        }
    }
    Ok(NativePrivacyReport {
        schema_version: "rsia.trace_export.redaction_report.v1".into(),
        status,
        findings_by_rule,
        disclaimer: crate::packages::PRIVACY_DISCLAIMER.into(),
    })
}

/// Prepare multiple authenticated runs under one read transaction. The manifest
/// covers each trace and the report; the final tree digest also covers the
/// manifest itself, so the later decision can bind every exact candidate byte.
pub async fn prepare_native_members(
    ctx: &Context,
    session: &mut Session,
    source_ids: &[String],
) -> Result<NativeCandidateMembers> {
    ctx.require(&[Role::Admin])?;
    if source_ids.is_empty() || source_ids.len() > crate::packages::MAX_FILES - 2 {
        return Err(Error::Invalid(
            "native trace source count is invalid".into(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for id in source_ids {
        if !seen.insert(id) {
            return Err(Error::Invalid("duplicate native trace source".into()));
        }
    }
    let mut files = Vec::with_capacity(source_ids.len() + 2);
    let mut sources = Vec::with_capacity(source_ids.len());
    for (index, source_id) in source_ids.iter().enumerate() {
        let path = format!("traces/{index:04}.json");
        files.push(evo_storage::LocalExportFile {
            path: path.clone(),
            bytes: prepare_native_candidate(ctx, session, source_id).await?,
        });
        sources.push(json!({"source_id": source_id, "path": path}));
    }
    let report = scan_native_candidate_members(&files)?;
    if report.status != crate::packages::RedactionStatus::Clean {
        return Err(Error::Invalid(
            "native candidate privacy scan rejected".into(),
        ));
    }
    files.push(evo_storage::LocalExportFile {
        path: "redaction-report.json".into(),
        bytes: serde_json::to_vec(&report).map_err(|_| Error::Internal)?,
    });
    let entries: Vec<_> = files
        .iter()
        .map(|file| crate::packages::PackageEntry {
            path: file.path.clone(),
            digest_sha256: evo_core::hash(&file.bytes),
            size_bytes: file.bytes.len() as u64,
            compressed_bytes: file.bytes.len() as u64,
        })
        .collect();
    files.push(evo_storage::LocalExportFile {
        path: "manifest.json".into(),
        bytes: serde_json::to_vec(&json!({
            "schema_version": "rsia.trace_export.manifest.v1",
            "kind": "trace",
            "license": "RSIA-authored service success facts; origin instance and namespace only",
            "recipient_scope": SERVICE_FACTS_RECIPIENT,
            "sources": sources,
            "files": entries,
        }))
        .map_err(|_| Error::Internal)?,
    });
    if scan_native_candidate_members(&files)?.status != crate::packages::RedactionStatus::Clean {
        return Err(Error::Invalid(
            "native candidate privacy scan rejected".into(),
        ));
    }
    let (candidate_digest, total_bytes) = evo_storage::local_export_tree_digest(&files)?;
    Ok(NativeCandidateMembers {
        files,
        candidate_digest,
        total_bytes,
    })
}
