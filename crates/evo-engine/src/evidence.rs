//! Authorized forensic entry: aggregate, then locate. Never execute log commands.
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, MAX_DISCOVERED_FILES, Purpose,
    SourceCoverage, SourceSelection, TaskOrigin, assert_authorized_path,
};
use evo_core::{Error, Result, identifier};
use std::collections::BTreeSet;

pub struct ForensicLimits {
    pub max_files: usize,
    pub max_header_bytes: usize,
}

/// A run already loaded from the trusted run authority. The analyzer still
/// verifies every security-relevant field instead of trusting the caller's
/// selection or a model-provided run identifier.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredRunRecord {
    pub id: String,
    pub body: Vec<u8>,
    pub parent_family: String,
    pub task_origin: TaskOrigin,
    pub execution_attestation: ExecutionAttestation,
    pub purpose: Purpose,
}

impl Default for ForensicLimits {
    fn default() -> Self {
        Self {
            max_files: MAX_DISCOVERED_FILES,
            max_header_bytes: 64 * 1024,
        }
    }
}

pub fn ingest_trusted_runs(
    selection: &SourceSelection,
    bodies: &[(&str, &[u8])],
) -> Result<EvidenceSet> {
    selection.validate()?;
    let selected = selected_run_ids(selection)?;
    if bodies.len() > ForensicLimits::default().max_files {
        return Err(Error::Invalid("discovery limit reached".into()));
    }
    let mut coverage = SourceCoverage {
        discovery_exhausted: true,
        files_known: true,
        ..SourceCoverage::default()
    };
    let mut members = Vec::new();
    let mut clusters = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for (id, body) in bodies {
        identifier(id)?;
        if !selected.contains(*id) {
            return Err(Error::Forbidden);
        }
        if !seen.insert(*id) {
            return Err(Error::Conflict("duplicate selected run record".into()));
        }
        if body.len() > ForensicLimits::default().max_header_bytes && body.len() > 256 * 1024 {
            coverage.event_truncated += 1;
        }
        coverage.bytes_read += body.len() as u64;
        coverage.parsed_ok += 1;
        members.push(EvidenceMember {
            source_id: (*id).into(),
            content_digest: evo_core::hash(body),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: selection.purpose,
        });
        clusters.insert((*id).into());
    }
    require_all_selected_runs(&selected, &seen)?;
    EvidenceSet::build(format!("es-{}", members.len()), members, coverage, clusters)
}

/// Production E03 ingestion path. Unlike the legacy byte adapter above, this
/// requires a stored trusted-run record and derives independent clusters from
/// the trusted parent family rather than from run identifiers.
pub fn ingest_trusted_run_records(
    selection: &SourceSelection,
    records: &[StoredRunRecord],
) -> Result<EvidenceSet> {
    selection.validate()?;
    let selected = selected_run_ids(selection)?;
    if records.len() > ForensicLimits::default().max_files {
        return Err(Error::Invalid("discovery limit reached".into()));
    }

    let mut coverage = SourceCoverage {
        discovery_exhausted: true,
        files_known: true,
        ..SourceCoverage::default()
    };
    let mut members = Vec::with_capacity(records.len());
    let mut clusters = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for record in records {
        identifier(&record.id)?;
        identifier(&record.parent_family)?;
        if !selected.contains(record.id.as_str()) {
            return Err(Error::Forbidden);
        }
        if !seen.insert(record.id.as_str()) {
            return Err(Error::Conflict("duplicate selected run record".into()));
        }
        if record.task_origin != TaskOrigin::TrustedRun
            || record.execution_attestation != ExecutionAttestation::TrustedHost
        {
            return Err(Error::Forbidden);
        }
        if record.purpose != selection.purpose {
            return Err(Error::Conflict("trusted run purpose mismatch".into()));
        }
        if record.body.is_empty() {
            return Err(Error::Invalid(
                "trusted run record has no evidence body".into(),
            ));
        }
        if record.body.len() > ForensicLimits::default().max_header_bytes
            && record.body.len() > 256 * 1024
        {
            coverage.event_truncated += 1;
        }
        coverage.bytes_read += record.body.len() as u64;
        coverage.parsed_ok += 1;
        members.push(EvidenceMember {
            source_id: record.id.clone(),
            content_digest: evo_core::hash(&record.body),
            task_origin: record.task_origin,
            execution_attestation: record.execution_attestation,
            purpose: record.purpose,
        });
        clusters.insert(record.parent_family.clone());
    }
    require_all_selected_runs(&selected, &seen)?;
    EvidenceSet::build(format!("es-{}", members.len()), members, coverage, clusters)
}

fn selected_run_ids(selection: &SourceSelection) -> Result<BTreeSet<&str>> {
    if selection.run_ids.is_empty() {
        return Err(Error::Invalid("trusted run selection is empty".into()));
    }
    let selected: BTreeSet<&str> = selection.run_ids.iter().map(String::as_str).collect();
    if selected.len() != selection.run_ids.len() {
        return Err(Error::Conflict("duplicate selected run id".into()));
    }
    Ok(selected)
}

fn require_all_selected_runs(selected: &BTreeSet<&str>, seen: &BTreeSet<&str>) -> Result<()> {
    if selected != seen {
        return Err(Error::NotFound);
    }
    if seen.is_empty() {
        return Err(Error::Invalid(
            "zero records is not an empty-history success".into(),
        ));
    }
    Ok(())
}

pub fn reject_home_scan(path: &str, selection: &SourceSelection) -> Result<()> {
    if path == "/" || path == std::env::var("HOME").unwrap_or_default() {
        return Err(Error::Forbidden);
    }
    assert_authorized_path(path, &selection.roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> SourceSelection {
        SourceSelection {
            roots: vec!["/authorized".into()],
            run_ids: vec!["run1".into(), "run2".into()],
            purpose: Purpose::Generation,
            allow_model_excerpts: false,
        }
    }

    fn stored(id: &str, parent_family: &str) -> StoredRunRecord {
        StoredRunRecord {
            id: id.into(),
            body: format!("evidence-{id}").into_bytes(),
            parent_family: parent_family.into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Generation,
        }
    }

    #[test]
    fn two_authorized_runs_build_a_set() {
        let sel = selection();
        let set = ingest_trusted_runs(&sel, &[("run1", b"fail A"), ("run2", b"fail B")]).unwrap();
        assert_eq!(set.members.len(), 2);
        assert_eq!(set.coverage.as_label(), "complete");
    }

    #[test]
    fn unselected_or_missing_run_is_rejected() {
        let sel = selection();
        assert!(matches!(
            ingest_trusted_runs(&sel, &[("run1", b"ok"), ("run3", b"not selected")]),
            Err(Error::Forbidden)
        ));
        assert!(matches!(
            ingest_trusted_runs(&sel, &[("run1", b"only one")]),
            Err(Error::NotFound)
        ));
    }

    #[test]
    fn trusted_record_metadata_and_purpose_are_verified() {
        let sel = selection();
        let mut untrusted = stored("run1", "family-a");
        untrusted.execution_attestation = ExecutionAttestation::UnverifiedImport;
        assert!(matches!(
            ingest_trusted_run_records(&sel, &[untrusted, stored("run2", "family-b")]),
            Err(Error::Forbidden)
        ));

        let mut wrong_purpose = stored("run1", "family-a");
        wrong_purpose.purpose = Purpose::Inspection;
        assert!(matches!(
            ingest_trusted_run_records(&sel, &[wrong_purpose, stored("run2", "family-b")]),
            Err(Error::Conflict(message)) if message == "trusted run purpose mismatch"
        ));
    }

    #[test]
    fn independent_clusters_come_from_parent_family() {
        let sel = selection();
        let same_family = ingest_trusted_run_records(
            &sel,
            &[stored("run1", "family-a"), stored("run2", "family-a")],
        )
        .unwrap();
        assert_eq!(same_family.independent_clusters.len(), 1);
        assert!(same_family.independent_clusters.contains("family-a"));

        let independent = ingest_trusted_run_records(
            &sel,
            &[stored("run1", "family-a"), stored("run2", "family-b")],
        )
        .unwrap();
        assert_eq!(independent.independent_clusters.len(), 2);
    }
}

/// Immutable Host-issued source snapshot. Imported histories cannot use this entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredTraceAuthority {
    pub schema_version: String,
    pub record: StoredRunRecord,
    pub trace: evo_core::optimization::OptimizationTrace,
    pub excerpt_start: usize,
    pub excerpt_end: usize,
}

impl StoredTraceAuthority {
    pub fn validate(&self) -> Result<()> {
        let r = &self.record;
        let t = &self.trace;
        if self.schema_version != "rsia.optimization.source.v1"
            || r.task_origin != TaskOrigin::TrustedRun
            || r.execution_attestation != ExecutionAttestation::TrustedHost
            || r.purpose != Purpose::Development
            || t.purpose != r.purpose
            || t.run_id != r.id
            || t.parent_family != r.parent_family
            || t.source_digest != evo_core::hash(&r.body)
            || r.body.get(self.excerpt_start..self.excerpt_end) != Some(t.excerpt.as_bytes())
        {
            return Err(Error::Forbidden);
        }
        identifier(&r.id)?;
        identifier(&r.parent_family)?;
        Ok(())
    }
}

pub async fn store_trace_authority(
    store: &evo_storage::Store,
    context: &evo_core::Context,
    authority: &StoredTraceAuthority,
) -> Result<()> {
    context.require(&[evo_core::Role::Host])?;
    authority.validate()?;
    let mut session = store.session().await?;
    if session
        .get::<serde_json::Value>(context, "tombstone", &authority.record.id)
        .await?
        .is_some()
    {
        return Err(Error::Forbidden);
    }
    if session
        .get::<serde_json::Value>(context, "run", &authority.record.id)
        .await?
        .is_some()
    {
        let old = load_stored_source(&mut session, context, &authority.record.id).await?;
        if evo_core::fingerprint(&old)? != evo_core::fingerprint(authority)? {
            return Err(Error::Conflict("immutable source authority differs".into()));
        }
    } else {
        // Cache tombstones prevent a deleted source from being silently reintroduced.
        if session
            .cached::<StoredTraceAuthority, _>(
                context,
                "optimization.source",
                &authority.record.id,
                authority,
            )
            .await?
            .is_some()
        {
            return Err(Error::Conflict(
                "deleted source authority cannot be resurrected".into(),
            ));
        }
        session
            .put(
                context,
                "run",
                &authority.record.id,
                context.actor(),
                authority,
            )
            .await?;
        session
            .cache(
                context,
                "optimization.source",
                &authority.record.id,
                authority,
                &authority.record.id,
                authority,
            )
            .await?;
        session
            .audit(context, "optimization.source.host", &authority.record.id)
            .await?;
    }
    session.commit().await
}

pub async fn store_source_selection(
    store: &evo_storage::Store,
    context: &evo_core::Context,
    selection: &SourceSelection,
) -> Result<()> {
    context.require(&[evo_core::Role::Host, evo_core::Role::Admin])?;
    selection.validate()?;
    let id = format!("optgrant-{}", evo_core::fingerprint(selection)?);
    let mut session = store.session().await?;
    session
        .put(context, "artifact", &id, context.actor(), selection)
        .await?;
    for run in &selection.run_ids {
        session
            .put_edge(context, "artifact", &id, "run", run)
            .await?;
    }
    session
        .audit(context, "optimization.source.grant", &id)
        .await?;
    session.commit().await
}

/// Shared E03/E06 logical-revocation gate, usable inside the caller's transaction.
/// Missing sources, tombstones, and missing/changed watermarks all fail closed.
pub async fn validate_stored_sources(
    session: &mut evo_storage::Session,
    context: &evo_core::Context,
    source_ids: &[String],
    expected_watermark: u64,
) -> Result<()> {
    let actual = session
        .watermark(context)
        .await?
        .ok_or_else(|| Error::Conflict("missing source revoke watermark".into()))?;
    if u64::try_from(actual.0).ok() != Some(expected_watermark) {
        return Err(Error::Conflict("source revoke watermark changed".into()));
    }
    for id in source_ids {
        if session
            .get::<serde_json::Value>(context, "tombstone", id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
        load_stored_source(session, context, id).await?;
    }
    Ok(())
}

/// Legacy run documents and imports are never silently upgraded to Host authority.
pub async fn load_stored_source(
    session: &mut evo_storage::Session,
    context: &evo_core::Context,
    id: &str,
) -> Result<StoredTraceAuthority> {
    let value: serde_json::Value = session.need(context, "run", id).await?;
    if value.get("schema_version").and_then(|v| v.as_str()) != Some("rsia.optimization.source.v1") {
        return Err(Error::Invalid(
            "unsupported or unverified legacy run authority".into(),
        ));
    }
    let authority: StoredTraceAuthority = serde_json::from_value(value)
        .map_err(|_| Error::Invalid("invalid trusted source authority".into()))?;
    authority.validate()?;
    Ok(authority)
}
