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
use evo_core::{Context, Error, Result, Role, Run, fingerprint, hash, identifier, now};
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
    let (source, source_bytes) = authenticated_native_source(ctx, session, source_id).await?;

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
            bytes_read: source_bytes,
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
    pub source_snapshots: Vec<NativeSourceSnapshot>,
    pub source_watermark: Option<(i64, String)>,
}

/// Original storage provenance captured after the real Native source loader.
/// A snapshot is data, not permission to publish or a completed review.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSourceSnapshot {
    pub source_id: String,
    pub owner_actor: String,
    pub storage_body_sha256: String,
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
    let mut source_snapshots = Vec::with_capacity(source_ids.len());
    for (index, source_id) in source_ids.iter().enumerate() {
        let path = format!("traces/{index:04}.json");
        files.push(evo_storage::LocalExportFile {
            path: path.clone(),
            bytes: prepare_native_candidate(ctx, session, source_id).await?,
        });
        let row = session
            .object_row(ctx, "run", source_id)
            .await?
            .ok_or(Error::NotFound)?;
        source_snapshots.push(NativeSourceSnapshot {
            source_id: source_id.clone(),
            owner_actor: row.owner,
            storage_body_sha256: evo_core::hash(row.body.as_bytes()),
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
    let source_watermark = session.watermark(ctx).await?;
    Ok(NativeCandidateMembers {
        files,
        candidate_digest,
        total_bytes,
        source_snapshots,
        source_watermark,
    })
}

async fn authenticated_native_source(
    ctx: &Context,
    session: &mut Session,
    source_id: &str,
) -> Result<(ServiceFactSourceV1, u64)> {
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

    Ok((source, authority.record.body.len() as u64))
}

/// Recheck the real captured sources without regenerating any frozen member.
/// This reader grants neither review nor delivery and never reads a caller clock.
pub async fn verify_native_source_snapshots(
    ctx: &Context,
    session: &mut Session,
    snapshots: &[NativeSourceSnapshot],
    watermark: Option<&(i64, String)>,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    if snapshots.is_empty() || snapshots.len() > crate::packages::MAX_FILES - 2 {
        return Err(Error::Invalid(
            "native trace source count is invalid".into(),
        ));
    }
    for snapshot in snapshots {
        identifier(&snapshot.source_id)?;
        authenticated_native_source(ctx, session, &snapshot.source_id).await?;
        let row = session
            .object_row(ctx, "run", &snapshot.source_id)
            .await?
            .ok_or(Error::NotFound)?;
        if row.owner != snapshot.owner_actor
            || evo_core::hash(row.body.as_bytes()) != snapshot.storage_body_sha256
        {
            return Err(Error::Conflict(
                "native trace source snapshot changed".into(),
            ));
        }
    }
    if session.watermark(ctx).await?.as_ref() != watermark {
        return Err(Error::Conflict(
            "native trace source watermark changed".into(),
        ));
    }
    Ok(())
}

/// Phases the export record actually moves through. Every variant names a real
/// producer; `delivered`, `aborted`, `failed` and `revoked` have no writer in
/// this slice — the FS publication chain and the cleanup own them later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TraceExportPhase {
    Prepared,
    Reviewed,
    Delivered,
    Aborted,
    Failed,
    Revoked,
}

/// The wire word of a phase, shared by the record, the job result and the
/// status view so all three name the same closed vocabulary.
pub(crate) fn trace_phase_wire(phase: TraceExportPhase) -> Result<String> {
    serde_json::to_value(phase)
        .map_err(|_| Error::Internal)?
        .as_str()
        .map(str::to_owned)
        .ok_or(Error::Internal)
}

/// One exact member of the frozen candidate: its package path, its content
/// digest and its size. The bytes themselves never enter the record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TraceExportMember {
    pub path: String,
    pub digest_sha256: String,
    pub size_bytes: u64,
}

/// Where the frozen candidate bytes physically live. This slice records the
/// honest fact that no private staging exists yet; the publication slice
/// replaces it with the real staging state and its private paths.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TraceExportCandidateStorage {
    pub staged: bool,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TraceExportDeliveryStatus {
    NotDelivered,
    Delivered,
}

/// The delivery fact of the export. `not_delivered` is the only honest value
/// until the real publication chain lands a receipt digest.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TraceExportDelivery {
    pub status: TraceExportDeliveryStatus,
    pub receipt_digest: Option<String>,
}

/// Durability facts about the published tree. No tree exists in this slice,
/// so both facts stay at their honest unset values.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TraceExportDurability {
    pub tree_fsynced: bool,
    pub verified_tree_digest: Option<String>,
}

/// Only really observed local cost: `wall_seconds` is measured elapsed time
/// and `bytes` the measured candidate size. Unknown quantities are never
/// filled with zero, and no price, currency or budget stage is invented here.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TraceExportLocalCost {
    pub wall_seconds: i64,
    pub bytes: u64,
}

/// The `rsia.trace_export.v1` main record (physical kind `artifact`): one
/// logical export with its frozen candidate identity, the captured source
/// snapshots, the revocation watermark and the phases its real producers
/// advanced it through. It never carries the candidate bytes, private tokens
/// or absolute paths, and at most one prepare and one publish job may own it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TraceExportRecord {
    pub schema_version: String,
    pub id: String,
    pub export_id: String,
    pub phase: TraceExportPhase,
    pub prepare_job_id: String,
    pub publish_job_id: Option<String>,
    pub candidate_digest: String,
    pub members: Vec<TraceExportMember>,
    pub candidate_storage: TraceExportCandidateStorage,
    pub delivery: TraceExportDelivery,
    pub durability: TraceExportDurability,
    pub local_cost: TraceExportLocalCost,
    pub source_snapshots: Vec<NativeSourceSnapshot>,
    pub source_watermark_seq: Option<i64>,
    pub source_watermark_digest: Option<String>,
    pub owner_actor: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl TraceExportRecord {
    fn source_watermark(&self) -> Option<(i64, String)> {
        match (
            self.source_watermark_seq,
            self.source_watermark_digest.clone(),
        ) {
            (Some(seq), Some(digest)) => Some((seq, digest)),
            _ => None,
        }
    }
}

/// The physical record id of one logical export, shared by every writer and
/// reader of that export in the namespace.
pub(crate) fn trace_export_record_id(export_id: &str) -> Result<String> {
    identifier(export_id)?;
    if export_id.contains(['.', ':']) {
        return Err(Error::Invalid(
            "trace export identifiers are single segment".into(),
        ));
    }
    Ok(format!(
        "trace-export-{}",
        &fingerprint(&(evo_storage::TRACE_EXPORT_SCHEMA, export_id))?[..24]
    ))
}

/// Read the live record of one export in the caller's session. A record the
/// revocation cleanup already redacted is a `Conflict`, not a decode guess.
pub(crate) async fn load_live_record(
    ctx: &Context,
    session: &mut Session,
    export_id: &str,
) -> Result<Option<TraceExportRecord>> {
    ctx.require(&[Role::Admin])?;
    let record_id = trace_export_record_id(export_id)?;
    let Some(row) = session.object_row(ctx, "artifact", &record_id).await? else {
        return Ok(None);
    };
    let value = evo_storage::lifecycle::parse_strict_json(&row.body)?;
    if value
        .get("schema_version")
        .and_then(|schema| schema.as_str())
        == Some("rsia.redacted.v1")
    {
        return Err(Error::Conflict(
            "trace export record was redacted because its source was revoked".into(),
        ));
    }
    let record: TraceExportRecord = serde_json::from_value(value)
        .map_err(|_| Error::Conflict("trace export record shape differs".into()))?;
    if record.schema_version != evo_storage::TRACE_EXPORT_SCHEMA
        || record.id != record_id
        || record.export_id != export_id
    {
        return Err(Error::Conflict(
            "trace export record identity differs".into(),
        ));
    }
    Ok(Some(record))
}

/// First write of the main record inside the prepare job's own transaction.
/// The storage `put` gate carries the Admin role, the all-instance first-slot
/// quota and the immutable classification; this writer adds the one-prepare-
/// job-per-record rule: a record another prepare job owns is a `Conflict`
/// with zero writes, and this job's own record replays unchanged.
pub(crate) async fn write_prepared_record(
    ctx: &Context,
    session: &mut Session,
    export_id: &str,
    job_id: &str,
    members: &NativeCandidateMembers,
    started_at: i64,
) -> Result<TraceExportRecord> {
    ctx.require(&[Role::Admin])?;
    identifier(job_id)?;
    let record_id = trace_export_record_id(export_id)?;
    let record = TraceExportRecord {
        schema_version: evo_storage::TRACE_EXPORT_SCHEMA.into(),
        id: record_id.clone(),
        export_id: export_id.into(),
        phase: TraceExportPhase::Prepared,
        prepare_job_id: job_id.into(),
        publish_job_id: None,
        candidate_digest: members.candidate_digest.clone(),
        members: members
            .files
            .iter()
            .map(|file| TraceExportMember {
                path: file.path.clone(),
                digest_sha256: hash(&file.bytes),
                size_bytes: file.bytes.len() as u64,
            })
            .collect(),
        candidate_storage: TraceExportCandidateStorage {
            staged: false,
            paths: Vec::new(),
        },
        delivery: TraceExportDelivery {
            status: TraceExportDeliveryStatus::NotDelivered,
            receipt_digest: None,
        },
        durability: TraceExportDurability {
            tree_fsynced: false,
            verified_tree_digest: None,
        },
        local_cost: TraceExportLocalCost {
            wall_seconds: elapsed_seconds(started_at)?,
            bytes: members.total_bytes,
        },
        source_snapshots: members.source_snapshots.clone(),
        source_watermark_seq: members.source_watermark.as_ref().map(|(seq, _)| *seq),
        source_watermark_digest: members
            .source_watermark
            .as_ref()
            .map(|(_, digest)| digest.clone()),
        owner_actor: ctx.actor().into(),
        created_at: now()?,
        updated_at: now()?,
    };
    if let Some(row) = session.object_row(ctx, "artifact", &record_id).await? {
        let value = evo_storage::lifecycle::parse_strict_json(&row.body)?;
        if value
            .get("schema_version")
            .and_then(|schema| schema.as_str())
            != Some(evo_storage::TRACE_EXPORT_SCHEMA)
        {
            return Err(Error::Conflict(
                "trace export identity belongs to another record class".into(),
            ));
        }
        let old: TraceExportRecord = serde_json::from_value(value)
            .map_err(|_| Error::Conflict("trace export record shape differs".into()))?;
        if old.prepare_job_id != job_id {
            return Err(Error::Conflict(
                "trace export already has its one prepare job".into(),
            ));
        }
        if old.export_id != record.export_id
            || old.candidate_digest != record.candidate_digest
            || old.members != record.members
            || old.source_snapshots != record.source_snapshots
            || old.source_watermark_seq != record.source_watermark_seq
            || old.source_watermark_digest != record.source_watermark_digest
        {
            return Err(Error::Conflict(
                "trace export candidate changed under its prepare job".into(),
            ));
        }
        // Same job, same frozen content: the original object is the answer.
        return Ok(old);
    }
    session
        .put(ctx, "artifact", &record_id, ctx.actor(), &record)
        .await?;
    for snapshot in &record.source_snapshots {
        session
            .put_edge(ctx, "artifact", &record_id, "run", &snapshot.source_id)
            .await?;
    }
    Ok(record)
}

/// The publish decision point. The frozen candidate digest must equal the
/// record's frozen value, every captured source and the revocation watermark
/// are re-verified through the real closure (never by regenerating candidate
/// bytes), and only then does the record's one publish job advance it to
/// `reviewed`. The private FS staging and publication that follow are the
/// next slice's boundary ([`stage_reviewed_candidate_for_publication`]).
pub(crate) async fn advance_reviewed_record(
    ctx: &Context,
    session: &mut Session,
    export_id: &str,
    job_id: &str,
    candidate_digest: &str,
    started_at: i64,
) -> Result<TraceExportRecord> {
    ctx.require(&[Role::Admin])?;
    identifier(job_id)?;
    let mut record = load_live_record(ctx, session, export_id)?.ok_or(Error::NotFound)?;
    if record.candidate_digest != candidate_digest {
        return Err(Error::Conflict(
            "published candidate digest differs from the frozen candidate".into(),
        ));
    }
    match record.publish_job_id.as_deref() {
        Some(prior) if prior == job_id => {
            if record.phase == TraceExportPhase::Reviewed {
                // This job's own reviewed record replays unchanged.
                return Ok(record);
            }
            return Err(Error::Conflict(
                "trace export publish job cannot replay another phase".into(),
            ));
        }
        Some(_) => {
            return Err(Error::Conflict(
                "trace export already has its one publish job".into(),
            ));
        }
        None => {}
    }
    if record.phase != TraceExportPhase::Prepared {
        return Err(Error::Conflict(
            "trace export phase cannot be published".into(),
        ));
    }
    verify_native_source_snapshots(
        ctx,
        session,
        &record.source_snapshots,
        record.source_watermark().as_ref(),
    )
    .await?;
    record.phase = TraceExportPhase::Reviewed;
    record.publish_job_id = Some(job_id.into());
    record.local_cost.wall_seconds = record
        .local_cost
        .wall_seconds
        .checked_add(elapsed_seconds(started_at)?)
        .ok_or_else(|| Error::Conflict("trace export wall clock overflow".into()))?;
    record.updated_at = now()?;
    session
        .put(ctx, "artifact", &record.id, ctx.actor(), &record)
        .await?;
    Ok(record)
}

fn elapsed_seconds(started_at: i64) -> Result<i64> {
    now()?.checked_sub(started_at).ok_or(Error::ClockRegressed)
}

/// The exact `Conflict` the current publication boundary returns.
pub(crate) const PUBLICATION_UNWIRED: &str = "trace export publication is not wired in this slice";

/// Next-slice boundary between the reviewed decision above and the private FS
/// staging plus NOREPLACE rename publication of the frozen candidate. The
/// signature is the real one the publication chain implements; until that
/// slice it honestly refuses instead of pretending a delivery. `Conflict`
/// (not `Internal`) so a caller can tell the unwired boundary from a storage
/// fault and no supervisor mistakes it for a transient retry.
pub(crate) async fn stage_reviewed_candidate_for_publication(
    ctx: &Context,
    session: &mut Session,
    record: &TraceExportRecord,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    let _ = (session, record);
    Err(Error::Conflict(PUBLICATION_UNWIRED.into()))
}

/// Re-read the record a stored job result references, through the live source
/// closure, and prove the stored phase, record id and frozen candidate digest
/// still name it. A status view is never served from a drifted or revoked
/// export.
pub(crate) async fn verify_result_view(
    ctx: &Context,
    session: &mut Session,
    result: &evo_storage::ManagementTraceExportResultV2,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    let (export_id, record_id, candidate_digest, phase) = match result {
        evo_storage::ManagementTraceExportResultV2::TraceExportPrepared {
            export_id,
            record_id,
            candidate_digest,
            phase,
        }
        | evo_storage::ManagementTraceExportResultV2::TraceExportReviewed {
            export_id,
            record_id,
            candidate_digest,
            phase,
        } => (export_id, record_id, candidate_digest, phase),
    };
    let record = load_live_record(ctx, session, export_id)?.ok_or(Error::NotFound)?;
    if &record.id != record_id
        || record.candidate_digest != *candidate_digest
        || trace_phase_wire(record.phase)? != *phase
    {
        return Err(Error::Conflict(
            "trace export record view differs from the stored result".into(),
        ));
    }
    let mut dependencies = Vec::with_capacity(record.source_snapshots.len());
    for snapshot in &record.source_snapshots {
        dependencies.push(("run".into(), snapshot.source_id.clone()));
    }
    crate::revocation_gate::ensure_dependencies_live(
        ctx,
        session,
        &dependencies,
        "trace export result",
    )
    .await
}
