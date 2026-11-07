//! Native trace export from authenticated local service facts: private
//! in-memory candidates, and the export record that binds the Admin publish
//! decision to those exact frozen bytes. Candidate bytes carry prospective
//! scan/review declarations; they are neither a review receipt nor a
//! deliverable until the fenced publication records its delivery.

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
    let commit = env!("RSIA_EXPORTER_COMMIT");
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
    native_candidate_bytes(ctx, session, source_id).await
}

async fn native_candidate_bytes(
    ctx: &Context,
    session: &mut Session,
    source_id: &str,
) -> Result<Vec<u8>> {
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
    native_members(ctx, session, source_ids).await
}

/// The candidate builder behind the Admin entries; the source list was
/// validated once at the request boundary.
pub(crate) async fn native_members(
    ctx: &Context,
    session: &mut Session,
    source_ids: &[String],
) -> Result<NativeCandidateMembers> {
    let mut files = Vec::with_capacity(source_ids.len() + 2);
    let mut sources = Vec::with_capacity(source_ids.len());
    let mut source_snapshots = Vec::with_capacity(source_ids.len());
    for (index, source_id) in source_ids.iter().enumerate() {
        let path = format!("traces/{index:04}.json");
        files.push(evo_storage::LocalExportFile {
            path: path.clone(),
            bytes: native_candidate_bytes(ctx, session, source_id).await?,
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
    Ok(NativeCandidateMembers {
        files,
        candidate_digest,
        total_bytes,
        source_snapshots,
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

/// Phases of the export record, each with one real writer: the prepare job
/// writes `prepared`; its one publish job's decision transaction `reviewed`;
/// the fenced publication's completion (or crash recovery that proves the
/// exact published tree) `delivered`; a publication refused before its formal
/// rename `aborted`. A revoked source turns the record into the lifecycle
/// redaction (`rsia.redacted.v1`, state `source_revoked`): that cleanup removes
/// the published tree, keeps the delivery facts, and is never served as live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TraceExportPhase {
    Prepared,
    Reviewed,
    Delivered,
    Aborted,
}

/// The delivery fact, written only after the formal tree was renamed under
/// the publication fence: the receipt those exact bytes earned, the local
/// scope they were delivered to, and the residual statement that a later
/// local revocation cannot erase copies already taken from that directory.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TraceExportDelivery {
    pub receipt_digest: String,
    pub local_export_ref: String,
    pub remote_erasure_disclaimer: String,
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
/// snapshots and the phases its real producers advanced it through. It never
/// carries the candidate bytes, private tokens or absolute paths, and at most
/// one prepare and one publish job may own it.
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
    pub members: Vec<evo_storage::TraceExportMember>,
    pub delivery: Option<TraceExportDelivery>,
    pub local_cost: TraceExportLocalCost,
    pub source_snapshots: Vec<NativeSourceSnapshot>,
    pub owner_actor: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// The physical record id of one logical export (its id was validated at the
/// request boundary), shared by every writer and reader of that export.
pub(crate) fn trace_export_record_id(export_id: &str) -> Result<String> {
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
/// The storage `put` gate carries the all-instance quota and the immutable
/// classification; this writer adds the one-prepare-job-per-record rule: a
/// record another prepare job owns is a `Conflict` with zero writes, and this
/// job's own record replays unchanged.
pub(crate) async fn write_prepared_record(
    ctx: &Context,
    session: &mut Session,
    export_id: &str,
    job_id: &str,
    members: &NativeCandidateMembers,
    started_at: i64,
) -> Result<TraceExportRecord> {
    let record = TraceExportRecord {
        schema_version: evo_storage::TRACE_EXPORT_SCHEMA.into(),
        id: trace_export_record_id(export_id)?,
        export_id: export_id.into(),
        phase: TraceExportPhase::Prepared,
        prepare_job_id: job_id.into(),
        publish_job_id: None,
        candidate_digest: members.candidate_digest.clone(),
        members: members
            .files
            .iter()
            .map(|file| evo_storage::TraceExportMember {
                path: file.path.clone(),
                digest_sha256: hash(&file.bytes),
                size_bytes: file.bytes.len() as u64,
            })
            .collect(),
        delivery: None,
        local_cost: TraceExportLocalCost {
            wall_seconds: elapsed_seconds(started_at)?,
            bytes: members.total_bytes,
        },
        source_snapshots: members.source_snapshots.clone(),
        owner_actor: ctx.actor().into(),
        created_at: now()?,
        updated_at: now()?,
    };
    if let Some(old) = load_live_record(ctx, session, export_id).await? {
        if old.prepare_job_id != job_id {
            return Err(Error::Conflict(
                "trace export already has its one prepare job".into(),
            ));
        }
        if old.candidate_digest != record.candidate_digest
            || old.source_snapshots != record.source_snapshots
        {
            return Err(Error::Conflict(
                "trace export candidate changed under its prepare job".into(),
            ));
        }
        // Same job, same frozen content: the original object is the answer.
        return Ok(old);
    }
    session
        .put(ctx, "artifact", &record.id, ctx.actor(), &record)
        .await?;
    for snapshot in &record.source_snapshots {
        session
            .put_edge(ctx, "artifact", &record.id, "run", &snapshot.source_id)
            .await?;
    }
    Ok(record)
}

/// The publish decision (TX1 of the publish job). The request must name the
/// record's frozen digest and this job must be the record's one publish job.
/// The frozen members are re-derived through the same authenticated loader
/// that froze them — source closure, license, recipient scope and the exact
/// fact prefix, read once — and must reproduce the frozen tree digest (which
/// covers every member's path, digest and size) from the same source
/// snapshots; re-derived bytes are publication material, never a substitute
/// candidate. Only then does the record move to `reviewed`; this job's own
/// replay re-derives the same bytes without a second write.
pub(crate) async fn review_frozen_candidate(
    ctx: &Context,
    session: &mut Session,
    export_id: &str,
    job_id: &str,
    candidate_digest: &str,
    started_at: i64,
) -> Result<(TraceExportRecord, Vec<evo_storage::LocalExportFile>)> {
    let mut record = load_live_record(ctx, session, export_id)
        .await?
        .ok_or(Error::NotFound)?;
    if record.candidate_digest != candidate_digest {
        return Err(Error::Conflict(
            "published candidate digest differs from the frozen candidate".into(),
        ));
    }
    let replay = match (record.phase, record.publish_job_id.as_deref()) {
        (TraceExportPhase::Prepared, None) => false,
        (TraceExportPhase::Reviewed, Some(prior)) if prior == job_id => true,
        (_, Some(prior)) if prior != job_id => {
            return Err(Error::Conflict(
                "trace export already has its one publish job".into(),
            ));
        }
        _ => {
            return Err(Error::Conflict(
                "trace export phase cannot be published".into(),
            ));
        }
    };
    let source_ids: Vec<String> = record
        .source_snapshots
        .iter()
        .map(|snapshot| snapshot.source_id.clone())
        .collect();
    let members = native_members(ctx, session, &source_ids).await?;
    if members.candidate_digest != record.candidate_digest
        || members.source_snapshots != record.source_snapshots
    {
        return Err(Error::Conflict(
            "re-derived trace export candidate differs from the frozen candidate".into(),
        ));
    }
    if !replay {
        record.phase = TraceExportPhase::Reviewed;
        record.publish_job_id = Some(job_id.into());
        add_elapsed(&mut record, started_at)?;
        session
            .put(ctx, "artifact", &record.id, ctx.actor(), &record)
            .await?;
    }
    Ok((record, members.files))
}

/// The one terminal write of a `reviewed` publication: `delivered` with the
/// delivery fact when the formal tree was renamed under the publication fence
/// (the receipt those bytes earned), else `aborted` — refused or failed before
/// the rename, nothing delivered. Only the really observed cost is added.
pub(crate) async fn finish_publication(
    ctx: &Context,
    session: &mut Session,
    record: &TraceExportRecord,
    receipt: Option<&evo_storage::LocalExportReceipt>,
    started_at: i64,
) -> Result<TraceExportRecord> {
    let mut live = load_live_record(ctx, session, &record.export_id)
        .await?
        .ok_or(Error::NotFound)?;
    if live.phase != TraceExportPhase::Reviewed
        || live.publish_job_id != record.publish_job_id
        || live.candidate_digest != record.candidate_digest
    {
        return Err(Error::Conflict(
            "trace export record is not this reviewed publication".into(),
        ));
    }
    match receipt {
        Some(receipt) => {
            live.phase = TraceExportPhase::Delivered;
            live.delivery = Some(TraceExportDelivery {
                receipt_digest: receipt.completion_receipt_digest.clone(),
                local_export_ref: receipt.local_export_ref.clone(),
                remote_erasure_disclaimer: crate::packages::REMOTE_ERASURE_DISCLAIMER.into(),
            });
        }
        None => live.phase = TraceExportPhase::Aborted,
    }
    add_elapsed(&mut live, started_at)?;
    session
        .put(ctx, "artifact", &live.id, ctx.actor(), &live)
        .await?;
    Ok(live)
}

fn elapsed_seconds(started_at: i64) -> Result<i64> {
    now()?.checked_sub(started_at).ok_or(Error::ClockRegressed)
}

fn add_elapsed(record: &mut TraceExportRecord, started_at: i64) -> Result<()> {
    record.local_cost.wall_seconds = record
        .local_cost
        .wall_seconds
        .checked_add(elapsed_seconds(started_at)?)
        .ok_or_else(|| Error::Conflict("trace export wall clock overflow".into()))?;
    record.updated_at = now()?;
    Ok(())
}

/// Crash recovery of one publish job, anchored on the record alone: no
/// candidate is regenerated and no on-disk manifest proves anything by itself.
/// A `prepared` record without a publish job proves the decision never
/// committed; a `delivered` one is final. For this job's `reviewed` record the
/// formal tree must be exactly the frozen members and digest — the rename
/// happened under the publication fence — and only then is `delivered`
/// completed with the receipt those bytes earn, after making the rename
/// durable. A missing or differing tree proves no publication: `None`, and
/// nothing is completed or republished here.
pub(crate) async fn reconcile_publication(
    ctx: &Context,
    store: &evo_storage::Store,
    export_id: &str,
    publish_job_id: &str,
) -> Result<Option<TraceExportRecord>> {
    let started_at = now()?;
    let live = {
        let mut session = store.session().await?;
        let live = load_live_record(ctx, &mut session, export_id)
            .await?
            .ok_or(Error::NotFound)?;
        session.commit().await?;
        live
    };
    match (live.phase, live.publish_job_id.as_deref()) {
        (TraceExportPhase::Prepared, None) => return Ok(None),
        (TraceExportPhase::Delivered, Some(job)) if job == publish_job_id => return Ok(Some(live)),
        (TraceExportPhase::Reviewed, Some(job)) if job == publish_job_id => {}
        _ => {
            return Err(Error::Conflict(
                "trace export record is not this publish job's".into(),
            ));
        }
    }
    let receipt = match store
        .verify_trace_export_tree(ctx, export_id, &live.members, &live.candidate_digest)
        .await
    {
        Ok(receipt) => receipt,
        Err(Error::Conflict(reason)) if reason == evo_storage::TRACE_EXPORT_TREE_UNAVAILABLE => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    store.sync_trace_export_tree(ctx, export_id).await?;
    let mut session = store.session().await?;
    let record = finish_publication(ctx, &mut session, &live, Some(&receipt), started_at).await?;
    session.commit().await?;
    Ok(Some(record))
}

/// The status view of a succeeded trace export job. The record its result
/// names is re-read live (never a redacted one) with the same id and frozen
/// digest and a live source closure; the phase it has reached since is not
/// compared, so a prepare result stays servable after its publication. A
/// delivered result must still be the record's delivery receipt, and the
/// formal tree must still be exactly the frozen members — checked against the
/// recorded digests, never by regenerating bytes; a missing or tampered tree
/// is the honest `TRACE_EXPORT_TREE_UNAVAILABLE` conflict, never a resend.
pub(crate) async fn verify_result_view(
    ctx: &Context,
    store: &evo_storage::Store,
    result: &evo_storage::ManagementTraceExportResultV2,
) -> Result<()> {
    use evo_storage::ManagementTraceExportResultV2 as R;
    let (export_id, record_id, candidate_digest, receipt_digest) = match result {
        R::TraceExportPrepared {
            export_id,
            record_id,
            candidate_digest,
        } => (export_id, record_id, candidate_digest, None),
        R::TraceExportDelivered {
            export_id,
            record_id,
            candidate_digest,
            receipt_digest,
        } => (export_id, record_id, candidate_digest, Some(receipt_digest)),
    };
    let record = {
        let mut session = store.session().await?;
        let record = load_live_record(ctx, &mut session, export_id)
            .await?
            .ok_or(Error::NotFound)?;
        let dependencies: Vec<(String, String)> = record
            .source_snapshots
            .iter()
            .map(|snapshot| ("run".to_string(), snapshot.source_id.clone()))
            .collect();
        crate::revocation_gate::ensure_dependencies_live(
            ctx,
            &mut session,
            &dependencies,
            "trace export result",
        )
        .await?;
        session.commit().await?;
        record
    };
    if &record.id != record_id || &record.candidate_digest != candidate_digest {
        return Err(Error::Conflict(
            "trace export record view differs from the stored result".into(),
        ));
    }
    if let Some(receipt_digest) = receipt_digest {
        if record
            .delivery
            .as_ref()
            .map(|delivery| &delivery.receipt_digest)
            != Some(receipt_digest)
        {
            return Err(Error::Conflict(
                "trace export delivery view differs from the stored result".into(),
            ));
        }
        store
            .verify_trace_export_tree(ctx, export_id, &record.members, &record.candidate_digest)
            .await?;
    }
    Ok(())
}
