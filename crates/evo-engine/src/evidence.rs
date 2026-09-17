//! Authorized forensic entry: aggregate, then locate. Never execute log commands.
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, MAX_DISCOVERED_FILES, SourceCoverage,
    SourceSelection, TaskOrigin, assert_authorized_path,
};
use evo_core::{Error, Result, identifier};

pub struct ForensicLimits {
    pub max_files: usize,
    pub max_header_bytes: usize,
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
    if bodies.len() > ForensicLimits::default().max_files {
        return Err(Error::Invalid("discovery limit reached".into()));
    }
    let mut coverage = SourceCoverage {
        discovery_exhausted: true,
        files_known: true,
        ..SourceCoverage::default()
    };
    let mut members = Vec::new();
    let mut clusters = std::collections::BTreeSet::new();
    for (id, body) in bodies {
        identifier(id)?;
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
    if members.is_empty() {
        return Err(Error::Invalid(
            "zero records is not an empty-history success".into(),
        ));
    }
    EvidenceSet::build(format!("es-{}", members.len()), members, coverage, clusters)
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
    use evo_core::evidence::Purpose;

    #[test]
    fn two_authorized_runs_build_a_set() {
        let sel = SourceSelection {
            roots: vec!["/authorized".into()],
            run_ids: vec!["run1".into(), "run2".into()],
            purpose: Purpose::Generation,
            allow_model_excerpts: false,
        };
        let set = ingest_trusted_runs(&sel, &[("run1", b"fail A"), ("run2", b"fail B")]).unwrap();
        assert_eq!(set.members.len(), 2);
        assert_eq!(set.coverage.as_label(), "complete");
    }
}
