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
