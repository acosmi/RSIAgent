//! Authorized forensic entry: aggregate, then locate. Never execute log commands.
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, MAX_DISCOVERED_FILES, Purpose,
    SourceCoverage, SourceSelection, TaskOrigin, assert_authorized_path,
    assert_not_whole_home_scan,
};
use evo_core::{Error, Result, identifier};
use std::collections::BTreeSet;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedEvidenceSummaryView {
    pub result_id: String,
    /// The stored result version; a legacy (v1) summary is historical metadata
    /// only and never a current import.
    pub result_schema: String,
    pub legacy: bool,
    pub evidence_set_id: String,
    pub evidence_digest: String,
    pub coverage: SourceCoverage,
    pub total_sources: usize,
    pub total_events: usize,
    pub independent_clusters: usize,
    pub task_origin: TaskOrigin,
    pub execution_attestation: ExecutionAttestation,
    pub generation_status: String,
    pub formal_evaluation_eligible: bool,
}

/// Reads a persisted E16 import through its live source/tombstone/watermark
/// gate. This is a summary consumer; it never upgrades imported history into
/// the E03 TrustedHost run authority or exposes private source bytes.
pub async fn read_persisted_imported_evidence(
    context: &evo_core::Context,
    store: &evo_storage::Store,
    result_id: &str,
) -> Result<ImportedEvidenceSummaryView> {
    context.require(&[evo_core::Role::Admin, evo_core::Role::Evaluator])?;
    let stored = crate::import::PersistentImportService::new(store.clone())
        .load_static_result(context, result_id)
        .await?;
    let (result_id, result_schema, legacy, evidence_set, summary, generation_status) = match stored
    {
        crate::import::StoredImportResult::V2(record) => (
            record.id,
            record.schema_version,
            false,
            record.payload.evidence_set,
            record.payload.aggregate_summary,
            record.payload.generation_status,
        ),
        crate::import::StoredImportResult::LegacyV1(record) => (
            record.id,
            record.schema_version,
            true,
            record.payload.evidence_set,
            record.payload.aggregate_summary,
            record.payload.generation_status,
        ),
    };
    let evidence_set =
        evidence_set.ok_or_else(|| Error::Invalid("import produced no usable evidence".into()))?;
    Ok(ImportedEvidenceSummaryView {
        result_id,
        result_schema,
        legacy,
        evidence_set_id: evidence_set.id,
        evidence_digest: evidence_set.digest,
        coverage: summary.coverage,
        total_sources: summary.total_sources,
        total_events: summary.total_events,
        independent_clusters: summary.unique_clusters,
        task_origin: TaskOrigin::ImportedHistory,
        execution_attestation: ExecutionAttestation::UnverifiedImport,
        generation_status,
        formal_evaluation_eligible: false,
    })
}

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
        let member = EvidenceMember {
            source_id: (*id).into(),
            content_digest: evo_core::hash(body),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: selection.purpose,
        };
        evo_core::evidence::verify_evidence_member_content_v3(&member, body)?;
        members.push(member);
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
        let member = EvidenceMember {
            source_id: record.id.clone(),
            content_digest: evo_core::hash(&record.body),
            task_origin: record.task_origin,
            execution_attestation: record.execution_attestation,
            purpose: record.purpose,
        };
        evo_core::evidence::verify_evidence_member_content_v3(&member, &record.body)?;
        members.push(member);
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

/// Refuses whole-filesystem and whole-home scans (including home ancestors and
/// `/Users/<name>` / `/home/<name>` shapes), then requires `path` to sit strictly
/// inside one of the selection's roots. The rule itself lives in
/// `evo_core::evidence::assert_not_whole_home_scan` so `SourceSelection::validate`
/// applies the same policy to roots.
pub fn reject_home_scan(path: &str, selection: &SourceSelection) -> Result<()> {
    assert_not_whole_home_scan(path)?;
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
    fn trusted_ingest_rebuilds_members_from_complete_record_bytes() {
        let selection = selection();
        let records = [stored("run1", "family-a"), stored("run2", "family-b")];
        let set = ingest_trusted_run_records(&selection, &records).unwrap();
        for (member, record) in set.members.iter().zip(&records) {
            assert_eq!(member.content_digest, evo_core::hash(&record.body));
            evo_core::evidence::verify_evidence_member_content_v3(member, &record.body).unwrap();
            assert!(matches!(
                evo_core::evidence::verify_evidence_member_content_v3(member, b"another actual body"),
                Err(Error::Conflict(message)) if message == "evidence_content_digest_mismatch"
            ));
        }
        let mut changed = records.to_vec();
        changed[0].body = b"another actual body".to_vec();
        let rebuilt = ingest_trusted_run_records(&selection, &changed).unwrap();
        assert_ne!(
            set.members[0].content_digest,
            rebuilt.members[0].content_digest
        );
        assert_ne!(set.digest, rebuilt.digest);
        assert_eq!(
            set.members[1].content_digest,
            rebuilt.members[1].content_digest
        );
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
        evo_core::evidence::verify_evidence_member_content_v3(
            &EvidenceMember {
                source_id: r.id.clone(),
                content_digest: t.source_digest.clone(),
                task_origin: r.task_origin,
                execution_attestation: r.execution_attestation,
                purpose: r.purpose,
            },
            &r.body,
        )
        .map_err(|_| Error::Forbidden)?;
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
    store_trace_authority_in_session(&mut session, context, authority).await?;
    session.commit().await
}

// Both actual Host entrypoints validate the complete authority before this shared writer.
pub(crate) async fn store_trace_authority_in_session(
    session: &mut evo_storage::Session,
    context: &evo_core::Context,
    authority: &StoredTraceAuthority,
) -> Result<()> {
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
        let old = load_stored_source(session, context, &authority.record.id).await?;
        if evo_core::fingerprint(&old)? != evo_core::fingerprint(authority)? {
            return Err(Error::Conflict("immutable source authority differs".into()));
        }
    } else {
        // A run and an E16 import_source artifact are the two revocation sources, and
        // a revocation tombstone is keyed by the id of its source alone: a run that
        // shares its id with an import source would share its tombstone, and
        // whichever was revoked first would make the other unrevocable. The id of an
        // import source is derived from its selection, the id of a run is chosen by
        // the caller, so refuse here, before anything is written. An object of any
        // other kind with this id is a different object and does not matter, and
        // neither does an artifact that only claims the import_source schema. A
        // stored conflicting pair isolates the namespace from new sources.
        if session
            .registered_revocable_source(
                context,
                evo_core::evidence::RevocableSourceKind::ImportSource,
                &authority.record.id,
            )
            .await?
        {
            return Err(Error::Conflict(format!(
                "run {} cannot be stored: an E16 import_source artifact already has this id \
                 (a revocation tombstone is keyed by id)",
                authority.record.id
            )));
        }
        session.ensure_revocation_namespace_live(context).await?;
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
    Ok(())
}

pub(crate) const SERVICE_FACTS_LICENSE: &str = "rsia.service_facts.summary.local.v1";
pub(crate) const SERVICE_FACTS_RECIPIENT: &str = "origin_instance_local_namespace_only";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceFactSourceV1 {
    pub schema_version: String,
    pub service_run_id: String,
    pub captured_through_sequence: u64,
    pub license: String,
    pub recipient_scope: String,
    pub facts: Vec<evo_storage::ServiceSuccessFactV1>,
}

// Inputs are the authenticated service facts selected by the Host entrypoint,
// or the same authenticated prefix re-read by the Admin exporter; never caller events.
pub(crate) fn service_fact_authority(
    namespace: &str,
    service_run_id: &str,
    facts: Vec<evo_storage::ServiceSuccessFactV1>,
) -> Result<StoredTraceAuthority> {
    let first = facts.first().ok_or(Error::NotFound)?;
    if first.kind != "service.prepare"
        || first.subject_id != service_run_id
        || facts.iter().any(|fact| fact.run_id != service_run_id)
    {
        return Err(Error::Forbidden);
    }
    let captured_through_sequence = facts.last().ok_or(Error::NotFound)?.sequence;
    let source = ServiceFactSourceV1 {
        schema_version: "rsia.service_fact_source.v1".into(),
        service_run_id: service_run_id.into(),
        captured_through_sequence,
        license: SERVICE_FACTS_LICENSE.into(),
        recipient_scope: SERVICE_FACTS_RECIPIENT.into(),
        facts,
    };
    let body = serde_json::to_string(&source).map_err(|_| Error::Internal)?;
    // One immutable source per (namespace, run): a run is captured once and is
    // revoked as that one source.
    let source_id = format!(
        "service-facts-{}",
        &evo_core::fingerprint(&(namespace, service_run_id))?[..32]
    );
    let family = "rsia-service-facts".to_string();
    let authority = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: source_id.clone(),
            body: body.as_bytes().to_vec(),
            parent_family: family.clone(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: evo_core::optimization::OptimizationTrace {
            run_id: source_id,
            parent_family: family,
            source_digest: evo_core::hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: evo_core::optimization::TraceOutcome::Incomplete,
            diagnosis: None,
            excerpt: body.clone(),
            seed: 0,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    };
    authority.validate()?;
    Ok(authority)
}

/// What the grant calls itself in the `Conflict` it refuses with.
const GRANT_SUBJECT: &str = "source grant";

/// Stores the grant of a source selection: the artifact `optgrant-<fingerprint>` and
/// an edge to every run of it.
///
/// A grant is a new dependency over its runs, so it is refused, with nothing written,
/// when one of them or anything above one of them was revoked (plan §11 and §11.3: a
/// revocation refuses new generation, export and use at once; E08: a deleted
/// non-primary source blocks every content-dependent successor). Each run is judged by
/// its own kind (a tombstone written for a run; one that cannot be read fails closed),
/// then the upstream closure of the runs, by the same rule as the submit check of a
/// management request (`revocation_gate`), and the `Conflict` names the node. The check
/// reads in the session that writes, before the first write, in whatever state the
/// cleanup of the revoked source is.
///
/// Whether a run exists, and whether it is a trusted one, is not asked here: the
/// consumer (`StoreOptimizationJournal::verify_sources`) decides that, as before.
pub async fn store_source_selection(
    store: &evo_storage::Store,
    context: &evo_core::Context,
    selection: &SourceSelection,
) -> Result<()> {
    context.require(&[evo_core::Role::Host, evo_core::Role::Admin])?;
    selection.validate()?;
    let id = format!("optgrant-{}", evo_core::fingerprint(selection)?);
    let mut session = store.session().await?;
    let runs: Vec<(String, String)> = selection
        .run_ids
        .iter()
        .map(|run| ("run".to_string(), run.clone()))
        .collect();
    crate::revocation_gate::ensure_dependencies_live(context, &mut session, &runs, GRANT_SUBJECT)
        .await?;
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
    // The closure walk starts at the sources, so their own tombstones are judged
    // before a missing (cleaned-up) run could answer NotFound.
    let roots: Vec<_> = source_ids
        .iter()
        .map(|id| ("run".to_string(), id.clone()))
        .collect();
    if crate::revocation_gate::judge_upstream(context, session, &roots)
        .await?
        .is_some()
    {
        return Err(Error::Forbidden);
    }
    for id in source_ids {
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

/// Owner-only local material from a fixed persisted E16 result. The snapshot is
/// unreviewed, cannot be dispatched to a model, and grants no future-use authority.
pub async fn read_persisted_imported_development_material(
    context: &evo_core::Context,
    store: &evo_storage::Store,
    request: &evo_core::evidence::ImportMaterialReadRequest,
) -> Result<evo_core::evidence::ImportedDevelopmentMaterial> {
    crate::import::PersistentImportService::new(store.clone())
        .read_local_development_material(context, request)
        .await
}

#[cfg(test)]
mod q03_tests {
    use super::*;

    #[tokio::test]
    async fn q03_evidence_validation_blocks_transitive_source() {
        let dir = tempfile::tempdir().unwrap();
        let store = evo_storage::Store::open(&dir.path().join("q03.sqlite3"))
            .await
            .unwrap();
        let host = evo_core::Context::new("n", "host", evo_core::Role::Host).unwrap();
        let admin = evo_core::Context::new("n", "admin", evo_core::Role::Admin).unwrap();
        for id in ["q03-root", "q03-upstream"] {
            let authority = StoredTraceAuthority {
                schema_version: "rsia.optimization.source.v1".into(),
                record: StoredRunRecord {
                    id: id.into(),
                    body: id.as_bytes().to_vec(),
                    parent_family: id.into(),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                trace: evo_core::optimization::OptimizationTrace {
                    run_id: id.into(),
                    parent_family: id.into(),
                    source_digest: evo_core::hash(id.as_bytes()),
                    purpose: Purpose::Development,
                    outcome: evo_core::optimization::TraceOutcome::Success,
                    diagnosis: None,
                    excerpt: id.into(),
                    seed: 1,
                },
                excerpt_start: 0,
                excerpt_end: id.len(),
            };
            store_trace_authority(&store, &host, &authority)
                .await
                .unwrap();
        }
        let mut session = store.session().await.unwrap();
        session.bump_watermark(&admin, "q03-initial").await.unwrap();
        session
            .put_edge(&admin, "run", "q03-root", "candidate", "q03-middle")
            .await
            .unwrap();
        session
            .put_edge(&admin, "candidate", "q03-middle", "run", "q03-upstream")
            .await
            .unwrap();
        validate_stored_sources(&mut session, &host, &["q03-root".into()], 1)
            .await
            .unwrap();
        session.commit().await.unwrap();
        let revoked = evo_storage::lifecycle::LifecycleStore::begin_revoke(
            &admin,
            &store,
            evo_storage::lifecycle::TypedObjectRef {
                kind: "run".into(),
                id: "q03-upstream".into(),
            },
            "q03 fixture",
            10,
        )
        .await
        .unwrap();
        let mut session = store.session().await.unwrap();
        let before: Vec<serde_json::Value> = session.list(&host, "run").await.unwrap();
        assert!(matches!(
            validate_stored_sources(
                &mut session,
                &host,
                &["q03-root".into()],
                revoked.watermark_seq
            )
            .await,
            Err(Error::Forbidden)
        ));
        let after: Vec<serde_json::Value> = session.list(&host, "run").await.unwrap();
        assert_eq!(before, after);
    }
}
