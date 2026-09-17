//! Cross-run evidence, coverage, and routing records. Not a third publishable asset.
use crate::{Error, Result, fingerprint, identifier, text};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path};

pub const EVIDENCE_SCHEMA: &str = "rsia.evidence_set.v2";
pub const MAX_DISCOVERED_FILES: usize = 200;
pub const MAX_EXCERPTS: usize = 32;

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
}
