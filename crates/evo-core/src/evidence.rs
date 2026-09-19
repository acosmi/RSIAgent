//! Cross-run evidence, coverage, and routing records. Not a third publishable asset.
use crate::{Error, Result, fingerprint, identifier, text};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path};

pub const EVIDENCE_SCHEMA: &str = "rsia.evidence_set.v2";
pub const MAX_DISCOVERED_FILES: usize = 200;
pub const MAX_EXCERPTS: usize = 32;
pub const MAX_HEADER_PROBE_BYTES: usize = 64 * 1024;
pub const MAX_TOTAL_READ_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_EVENT_BYTES: usize = 256 * 1024;
pub const MAX_TOTAL_EXCERPT_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOrigin {
    TrustedRun,
    Synthetic,
    ImportedHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAttestation {
    TrustedHost,
    UnverifiedImport,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Development,
    Inspection,
    Generation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteClass {
    Procedural,
    ImprovementMethod,
    Preference,
    Environment,
    CapabilityRequest,
    OutOfScope,
    InsufficientOrUnsafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteStatus {
    ReadyForSkill,
    BlockedFeature,
    InspectPending,
    Diagnostic,
    AdminReview,
    Rejected,
    Quarantined,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSelection {
    pub roots: Vec<String>,
    pub run_ids: Vec<String>,
    pub purpose: Purpose,
    pub allow_model_excerpts: bool,
}

impl SourceSelection {
    pub fn validate(&self) -> Result<()> {
        if self.roots.is_empty() && self.run_ids.is_empty() {
            return Err(Error::Invalid("source selection is empty".into()));
        }
        for id in &self.run_ids {
            identifier(id)?;
        }
        for root in &self.roots {
            assert_authorized_path(root, &self.roots)?;
        }
        Ok(())
    }
}

pub fn assert_authorized_path(path: &str, roots: &[String]) -> Result<()> {
    if path.is_empty() || path.contains('\0') {
        return Err(Error::Invalid("invalid path".into()));
    }
    let p = Path::new(path);
    if p.is_absolute()
        && !roots
            .iter()
            .any(|r| path == r || path.starts_with(&format!("{r}/")))
    {
        return Err(Error::Forbidden);
    }
    for c in p.components() {
        if matches!(c, Component::ParentDir) {
            return Err(Error::Forbidden);
        }
    }
    if path.contains('\n') || path.contains("sudo ") {
        return Err(Error::Invalid(
            "log command is data, not a new source".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCoverage {
    pub discovery_exhausted: bool,
    pub files_known: bool,
    pub bytes_read: u64,
    pub parsed_ok: u64,
    pub parsed_fail: u64,
    pub permission_denied: u64,
    pub missing: u64,
    pub unsupported_format: u64,
    pub zero_records: u64,
    pub event_truncated: u64,
    pub char_truncated: u64,
    pub excerpt_truncated: u64,
    pub outbound_truncated: u64,
}

impl SourceCoverage {
    pub fn complete(&self) -> bool {
        self.discovery_exhausted
            && self.files_known
            && self.parsed_fail == 0
            && self.permission_denied == 0
            && self.missing == 0
            && self.unsupported_format == 0
            && self.event_truncated == 0
            && self.char_truncated == 0
            && self.excerpt_truncated == 0
            && self.outbound_truncated == 0
    }

    pub fn as_label(&self) -> &'static str {
        if self.complete() {
            "complete"
        } else {
            "partial"
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceMember {
    pub source_id: String,
    pub content_digest: String,
    pub task_origin: TaskOrigin,
    pub execution_attestation: ExecutionAttestation,
    pub purpose: Purpose,
}

impl EvidenceMember {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.source_id)?;
        identifier(&self.content_digest)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSet {
    pub schema_version: String,
    pub id: String,
    pub members: Vec<EvidenceMember>,
    pub coverage: SourceCoverage,
    pub independent_clusters: BTreeSet<String>,
    pub digest: String,
}

impl EvidenceSet {
    pub fn build(
        id: impl Into<String>,
        members: Vec<EvidenceMember>,
        coverage: SourceCoverage,
        clusters: BTreeSet<String>,
    ) -> Result<Self> {
        let id = id.into();
        identifier(&id)?;
        if members.is_empty() {
            return Err(Error::Invalid("evidence set needs members".into()));
        }
        for m in &members {
            m.validate()?;
        }
        let mut set = Self {
            schema_version: EVIDENCE_SCHEMA.into(),
            id,
            members,
            coverage,
            independent_clusters: clusters,
            digest: String::new(),
        };
        set.digest = fingerprint(&set)?;
        Ok(set)
    }

    pub fn contains_source(&self, source_id: &str) -> bool {
        self.members.iter().any(|m| m.source_id == source_id)
    }
}

/// Immutable locator binding source object digest, byte/event range, and excerpt digest (§5.6, §6.3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceLocator {
    pub source_id: String,
    pub source_digest: String,
    pub byte_start: usize,
    pub byte_end: usize,
    pub event_index: usize,
    pub excerpt_digest: String,
}

impl EvidenceLocator {
    pub fn build(
        source_id: impl Into<String>,
        source_digest: impl Into<String>,
        byte_start: usize,
        byte_end: usize,
        event_index: usize,
        excerpt_digest: impl Into<String>,
    ) -> Result<Self> {
        let source_id = source_id.into();
        let source_digest = source_digest.into();
        let excerpt_digest = excerpt_digest.into();
        identifier(&source_id)?;
        identifier(&source_digest)?;
        identifier(&excerpt_digest)?;
        if byte_start > byte_end {
            return Err(Error::Invalid("invalid byte range".into()));
        }
        Ok(Self {
            source_id,
            source_digest,
            byte_start,
            byte_end,
            event_index,
            excerpt_digest,
        })
    }

    /// Verifies source digest and extracts excerpt. Returns Error::Conflict("source_changed")
    /// if the source file changed between probe and read (§6.3, V056).
    pub fn verify_and_extract<'a>(&self, current_source_bytes: &'a [u8]) -> Result<&'a [u8]> {
        let current_digest = crate::hash(current_source_bytes);
        if current_digest != self.source_digest {
            return Err(Error::Conflict("source_changed".into()));
        }
        let excerpt = current_source_bytes
            .get(self.byte_start..self.byte_end)
            .ok_or_else(|| Error::Invalid("offset out of bounds".into()))?;
        let excerpt_digest = crate::hash(excerpt);
        if excerpt_digest != self.excerpt_digest {
            return Err(Error::Conflict("excerpt_digest_mismatch".into()));
        }
        Ok(excerpt)
    }
}

/// Aggregate summary of forensic history across sources (§5.6, §6.3).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AggregateSummary {
    pub total_sources: usize,
    pub total_events: usize,
    pub tool_calls_count: usize,
    pub failure_count: usize,
    pub unique_clusters: usize,
    pub counter_examples: Vec<String>,
    pub coverage: SourceCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub id: String,
    pub class: RouteClass,
    pub hypothesis: String,
    pub supporting: Vec<String>,
    pub counter_refs: Vec<String>,
    pub evidence_set_id: String,
}

impl Pattern {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.id)?;
        identifier(&self.evidence_set_id)?;
        text(&self.hypothesis, "hypothesis", 4096)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingDecision {
    pub pattern_id: String,
    pub class: RouteClass,
    pub consumer: String,
    pub status: RouteStatus,
    pub reason: String,
}

pub fn route(class: RouteClass, meta_enabled: bool) -> RoutingDecision {
    let (consumer, status, reason) = match class {
        RouteClass::Procedural => (
            "skill_generator",
            RouteStatus::ReadyForSkill,
            "reusable method",
        ),
        RouteClass::ImprovementMethod if meta_enabled => (
            "improver_job",
            RouteStatus::ReadyForSkill,
            "improvement method",
        ),
        RouteClass::ImprovementMethod => (
            "inspect",
            RouteStatus::BlockedFeature,
            "E14 consumer not enabled",
        ),
        RouteClass::Preference => (
            "inspect",
            RouteStatus::InspectPending,
            "preference is not auto-applied",
        ),
        RouteClass::Environment => (
            "inspect",
            RouteStatus::Diagnostic,
            "environment failure is not a skill",
        ),
        RouteClass::CapabilityRequest => (
            "inspect",
            RouteStatus::AdminReview,
            "capability request is not authorization",
        ),
        RouteClass::OutOfScope => (
            "inspect",
            RouteStatus::Rejected,
            "out of current authorized scope",
        ),
        RouteClass::InsufficientOrUnsafe => (
            "inspect",
            RouteStatus::Quarantined,
            "insufficient or unsafe evidence",
        ),
    };
    RoutingDecision {
        pattern_id: String::new(),
        class,
        consumer: consumer.into(),
        status,
        reason: reason.into(),
    }
}

pub fn cluster_retries(source_family: &str, retries: usize) -> String {
    format!("{source_family}:{retries}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEvidenceRequest {
    pub evidence_set_id: String,
    pub source_ids: Vec<String>,
    pub excerpts: Vec<String>,
}

impl ModelEvidenceRequest {
    pub fn from_set(set: &EvidenceSet, excerpts: Vec<String>) -> Result<Self> {
        if set.members.len() < 2 {
            return Err(Error::Invalid(
                "generator request needs at least two authorized independent sources".into(),
            ));
        }
        if excerpts.len() > MAX_EXCERPTS {
            return Err(Error::Invalid("too many excerpts".into()));
        }
        Ok(Self {
            evidence_set_id: set.id.clone(),
            source_ids: set.members.iter().map(|m| m.source_id.clone()).collect(),
            excerpts,
        })
    }
}

pub fn invalidate_jobs_if_source_revoked(
    set: &EvidenceSet,
    revoked: &str,
    pending: &[String],
) -> Result<Vec<String>> {
    if !set.contains_source(revoked) {
        return Ok(Vec::new());
    }
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    Ok(pending.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, origin: TaskOrigin, att: ExecutionAttestation) -> EvidenceMember {
        EvidenceMember {
            source_id: id.into(),
            content_digest: format!("{id}d"),
            task_origin: origin,
            execution_attestation: att,
            purpose: Purpose::Generation,
        }
    }

    #[test]
    fn two_runs_enter_generator_request() {
        let set = EvidenceSet::build(
            "es1",
            vec![
                member(
                    "run1",
                    TaskOrigin::TrustedRun,
                    ExecutionAttestation::TrustedHost,
                ),
                member(
                    "run2",
                    TaskOrigin::TrustedRun,
                    ExecutionAttestation::TrustedHost,
                ),
            ],
            SourceCoverage::default(),
            ["c1".into(), "c2".into()].into(),
        )
        .unwrap();
        let req = ModelEvidenceRequest::from_set(&set, vec!["e1".into()]).unwrap();
        assert_eq!(req.source_ids.len(), 2);
    }

    #[test]
    fn single_run_is_not_cross_task() {
        let set = EvidenceSet::build(
            "es1",
            vec![member(
                "run1",
                TaskOrigin::TrustedRun,
                ExecutionAttestation::TrustedHost,
            )],
            SourceCoverage::default(),
            ["c1".into()].into(),
        )
        .unwrap();
        assert!(ModelEvidenceRequest::from_set(&set, vec![]).is_err());
    }

    #[test]
    fn path_traversal_and_log_commands_rejected() {
        let roots = vec!["/authorized".into()];
        assert!(assert_authorized_path("../etc/passwd", &roots).is_err());
        assert!(assert_authorized_path("/etc/passwd", &roots).is_err());
        assert!(assert_authorized_path("sudo rm -rf /\n/authorized", &roots).is_err());
        assert!(assert_authorized_path("/authorized/run.jsonl", &roots).is_ok());
    }

    #[test]
    fn coverage_is_partial_when_any_dimension_truncates() {
        let mut c = SourceCoverage {
            discovery_exhausted: true,
            files_known: true,
            ..SourceCoverage::default()
        };
        assert_eq!(c.as_label(), "complete");
        c.char_truncated = 1;
        assert_eq!(c.as_label(), "partial");
        assert!(!c.complete());
    }

    #[test]
    fn import_attestation_is_not_trusted_run() {
        let m = member(
            "imp1",
            TaskOrigin::ImportedHistory,
            ExecutionAttestation::UnverifiedImport,
        );
        assert_ne!(m.execution_attestation, ExecutionAttestation::TrustedHost);
        assert_ne!(m.task_origin, TaskOrigin::TrustedRun);
    }

    #[test]
    fn preference_and_environment_do_not_become_skills() {
        assert_eq!(
            route(RouteClass::Preference, false).status,
            RouteStatus::InspectPending
        );
        assert_eq!(
            route(RouteClass::Environment, false).status,
            RouteStatus::Diagnostic
        );
        assert_eq!(
            route(RouteClass::ImprovementMethod, false).status,
            RouteStatus::BlockedFeature
        );
        assert_eq!(
            route(RouteClass::Procedural, false).status,
            RouteStatus::ReadyForSkill
        );
    }

    #[test]
    fn revoking_source_invalidates_pending_jobs() {
        let set = EvidenceSet::build(
            "es1",
            vec![
                member(
                    "run1",
                    TaskOrigin::TrustedRun,
                    ExecutionAttestation::TrustedHost,
                ),
                member(
                    "run2",
                    TaskOrigin::TrustedRun,
                    ExecutionAttestation::TrustedHost,
                ),
            ],
            SourceCoverage::default(),
            BTreeSet::new(),
        )
        .unwrap();
        let jobs = invalidate_jobs_if_source_revoked(&set, "run2", &["job-a".into()]).unwrap();
        assert_eq!(jobs, vec!["job-a"]);
    }

    #[test]
    fn evidence_locator_binds_digest_and_detects_source_changed() {
        let content = b"line1: safe code\nline2: failure observed\nline3: end";
        let digest = crate::hash(content);
        let excerpt = b"failure observed";
        let excerpt_digest = crate::hash(excerpt);
        let start = 24;
        let locator = EvidenceLocator::build(
            "src-1",
            &digest,
            start,
            start + excerpt.len(),
            1,
            &excerpt_digest,
        )
        .unwrap();

        let extracted = locator.verify_and_extract(content).unwrap();
        assert_eq!(extracted, excerpt);

        // If content changed (e.g. modified after probe), Conflict("source_changed") is returned.
        let modified = b"line1: safe code\nline2: modified text\nline3: end";
        let err = locator.verify_and_extract(modified).unwrap_err();
        assert!(matches!(err, Error::Conflict(msg) if msg == "source_changed"));
    }
}
