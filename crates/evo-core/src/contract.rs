//! Versioned v2 contracts: field ownership, typed patches, host surface, receipts.
//! v1 request types in lib.rs keep their JSON meaning; this module does not widen them.
use crate::{ArtifactKind, Error, Result, Strategy, Validate, fingerprint, identifier, text};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const BUNDLE_SCHEMA: &str = "rsia.resolved_bundle.v2";
pub const SURFACE_SCHEMA: &str = "rsia.host_surface.v1";
pub const MODEL_TOOLS: [&str; 4] = ["evo_prepare", "evo_feedback", "evo_propose", "evo_inspect"];
pub const V1_REQUEST_KINDS: [&str; 5] =
    ["prepare", "feedback", "propose", "inspect", "application"];
pub const V1_TOOL_DESCRIPTOR_MAX: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldOwner {
    ProtectedCore,
    SkillCandidate,
    ImproverCandidate,
    TrustedHost,
    Diagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldContract {
    pub path: &'static str,
    pub owner: FieldOwner,
    pub consumer: &'static str,
}

pub const FIELD_CONTRACTS: &[FieldContract] = &[
    FieldContract {
        path: "namespace",
        owner: FieldOwner::ProtectedCore,
        consumer: "dispatch",
    },
    FieldContract {
        path: "budget",
        owner: FieldOwner::ProtectedCore,
        consumer: "broker",
    },
    FieldContract {
        path: "approval",
        owner: FieldOwner::ProtectedCore,
        consumer: "publisher",
    },
    FieldContract {
        path: "skill.content",
        owner: FieldOwner::SkillCandidate,
        consumer: "compiler",
    },
    FieldContract {
        path: "skill.applicability",
        owner: FieldOwner::SkillCandidate,
        consumer: "compiler",
    },
    FieldContract {
        path: "skill.counterexample",
        owner: FieldOwner::SkillCandidate,
        consumer: "compiler",
    },
    FieldContract {
        path: "skill.required_capabilities",
        owner: FieldOwner::SkillCandidate,
        consumer: "compiler",
    },
    FieldContract {
        path: "skill.dependencies",
        owner: FieldOwner::SkillCandidate,
        consumer: "compiler",
    },
    FieldContract {
        path: "improver.instruction",
        owner: FieldOwner::ImproverCandidate,
        consumer: "worker",
    },
    FieldContract {
        path: "improver.max_candidates",
        owner: FieldOwner::ImproverCandidate,
        consumer: "worker",
    },
    FieldContract {
        path: "improver.max_rounds",
        owner: FieldOwner::ImproverCandidate,
        consumer: "worker",
    },
    FieldContract {
        path: "host.model",
        owner: FieldOwner::TrustedHost,
        consumer: "runner",
    },
    FieldContract {
        path: "host.tools",
        owner: FieldOwner::TrustedHost,
        consumer: "runner",
    },
];

pub fn field_contract(path: &str) -> Result<&'static FieldContract> {
    FIELD_CONTRACTS
        .iter()
        .find(|c| c.path == path)
        .ok_or_else(|| Error::Invalid(format!("no consumer for field {path}")))
}

pub fn assert_candidate_may_write(kind: ArtifactKind, path: &str) -> Result<()> {
    let c = field_contract(path)?;
    match (kind, c.owner) {
        (ArtifactKind::Skill, FieldOwner::SkillCandidate) => Ok(()),
        (ArtifactKind::Improver, FieldOwner::ImproverCandidate) => Ok(()),
        _ => Err(Error::Forbidden),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PatchOp<T> {
    #[default]
    Inherit,
    ResetToBaseline,
    Set {
        value: T,
    },
}

impl<T: Clone> PatchOp<T> {
    pub fn apply(&self, parent: &T, baseline: &T) -> T {
        match self {
            Self::Inherit => parent.clone(),
            Self::ResetToBaseline => baseline.clone(),
            Self::Set { value } => value.clone(),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::ResetToBaseline => "reset_to_baseline",
            Self::Set { .. } => "set",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginLayer {
    Parent,
    Baseline,
    Explicit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueOrigin {
    pub field: String,
    pub layer: OriginLayer,
    pub source_digest: String,
    pub op: String,
    pub value_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillSnapshot {
    pub content: String,
    pub applicability: String,
    pub counterexample: String,
    pub required_capabilities: Vec<String>,
    pub dependencies: Vec<String>,
}

impl SkillSnapshot {
    pub fn empty() -> Self {
        Self {
            content: String::new(),
            applicability: String::new(),
            counterexample: String::new(),
            required_capabilities: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if !self.content.is_empty() {
            text(&self.content, "content", 16384)?;
        }
        if !self.applicability.is_empty() {
            text(&self.applicability, "applicability", 4096)?;
        }
        if !self.counterexample.is_empty() {
            text(&self.counterexample, "counterexample", 4096)?;
        }
        if self.required_capabilities.len() > 32 || self.dependencies.len() > 32 {
            return Err(Error::Invalid("too many references".into()));
        }
        for id in self
            .required_capabilities
            .iter()
            .chain(self.dependencies.iter())
        {
            identifier(id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SkillPatch {
    #[serde(default)]
    pub content: PatchOp<String>,
    #[serde(default)]
    pub applicability: PatchOp<String>,
    #[serde(default)]
    pub counterexample: PatchOp<String>,
    #[serde(default)]
    pub required_capabilities: PatchOp<Vec<String>>,
    #[serde(default)]
    pub dependencies: PatchOp<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ImproverPatch {
    #[serde(default)]
    pub instruction: PatchOp<String>,
    #[serde(default)]
    pub max_candidates: PatchOp<u8>,
    #[serde(default)]
    pub max_rounds: PatchOp<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCapabilities {
    pub available: BTreeSet<String>,
    pub granted: BTreeSet<String>,
}

impl HostCapabilities {
    pub fn effective(&self) -> BTreeSet<String> {
        self.available
            .intersection(&self.granted)
            .cloned()
            .collect()
    }

    pub fn allows(&self, required: &[String]) -> bool {
        let have = self.effective();
        required.iter().all(|c| have.contains(c))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemSnapshot {
    pub schema_version: String,
    pub profile_id: String,
    pub host_id: String,
    pub host_version: String,
    pub model_id: String,
    pub tools: Vec<String>,
    pub mandatory_context_digest: String,
}

impl SystemSnapshot {
    pub fn digest(&self) -> Result<String> {
        fingerprint(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub evolution_enabled: bool,
    pub parent_digest: String,
    pub baseline_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedBundle {
    pub schema_version: String,
    pub profile_id: String,
    pub parent_digest: String,
    pub baseline_digest: String,
    pub skill: SkillSnapshot,
    pub improver: Strategy,
    pub origins: Vec<ValueOrigin>,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunProjection {
    pub bundle_digest: String,
    pub instructions: Vec<String>,
    pub tools: Vec<String>,
}

pub fn compile_skill(
    profile: &Profile,
    parent: &SkillSnapshot,
    baseline: &SkillSnapshot,
    patch: &SkillPatch,
    revoked: &BTreeSet<String>,
) -> Result<(SkillSnapshot, Vec<ValueOrigin>)> {
    parent.validate()?;
    baseline.validate()?;
    if profile.parent_digest.is_empty() || profile.baseline_digest.is_empty() {
        return Err(Error::Invalid(
            "parent and baseline digests are required".into(),
        ));
    }
    let mut origins = Vec::new();
    let mut out = SkillSnapshot::empty();
    out.content = apply_leaf(
        "skill.content",
        &patch.content,
        &parent.content,
        &baseline.content,
        &profile.parent_digest,
        &profile.baseline_digest,
        &mut origins,
    )?;
    out.applicability = apply_leaf(
        "skill.applicability",
        &patch.applicability,
        &parent.applicability,
        &baseline.applicability,
        &profile.parent_digest,
        &profile.baseline_digest,
        &mut origins,
    )?;
    out.counterexample = apply_leaf(
        "skill.counterexample",
        &patch.counterexample,
        &parent.counterexample,
        &baseline.counterexample,
        &profile.parent_digest,
        &profile.baseline_digest,
        &mut origins,
    )?;
    out.required_capabilities = apply_leaf(
        "skill.required_capabilities",
        &patch.required_capabilities,
        &parent.required_capabilities,
        &baseline.required_capabilities,
        &profile.parent_digest,
        &profile.baseline_digest,
        &mut origins,
    )?;
    out.dependencies = apply_leaf(
        "skill.dependencies",
        &patch.dependencies,
        &parent.dependencies,
        &baseline.dependencies,
        &profile.parent_digest,
        &profile.baseline_digest,
        &mut origins,
    )?;
    out.validate()?;
    for dep in &out.dependencies {
        if revoked.contains(dep) {
            return Err(Error::Conflict("dependency_revoked".into()));
        }
    }
    Ok((out, origins))
}

pub fn compile_improver(
    profile: &Profile,
    parent: &Strategy,
    baseline: &Strategy,
    patch: &ImproverPatch,
) -> Result<(Strategy, Vec<ValueOrigin>)> {
    parent.validate()?;
    baseline.validate()?;
    let mut origins = Vec::new();
    let instruction = apply_leaf(
        "improver.instruction",
        &patch.instruction,
        &parent.instruction,
        &baseline.instruction,
        &profile.parent_digest,
        &profile.baseline_digest,
        &mut origins,
    )?;
    let max_candidates = apply_leaf(
        "improver.max_candidates",
        &patch.max_candidates,
        &parent.max_candidates,
        &baseline.max_candidates,
        &profile.parent_digest,
        &profile.baseline_digest,
        &mut origins,
    )?;
    let max_rounds = apply_leaf(
        "improver.max_rounds",
        &patch.max_rounds,
        &parent.max_rounds,
        &baseline.max_rounds,
        &profile.parent_digest,
        &profile.baseline_digest,
        &mut origins,
    )?;
    let out = Strategy {
        schema_version: parent.schema_version.clone(),
        instruction,
        max_candidates,
        max_rounds,
        require_counterexample: parent.require_counterexample,
    };
    out.validate()?;
    Ok((out, origins))
}

pub struct CompileParts<'a> {
    pub profile: &'a Profile,
    pub parent: &'a SkillSnapshot,
    pub baseline: &'a SkillSnapshot,
    pub parent_strategy: &'a Strategy,
    pub baseline_strategy: &'a Strategy,
    pub skill_patch: &'a SkillPatch,
    pub improver_patch: &'a ImproverPatch,
    pub caps: &'a HostCapabilities,
    pub revoked: &'a BTreeSet<String>,
}

pub fn compile_bundle(parts: CompileParts<'_>) -> Result<ResolvedBundle> {
    identifier(&parts.profile.id)?;
    identifier(&parts.profile.parent_digest)?;
    identifier(&parts.profile.baseline_digest)?;
    let (skill, mut origins) = compile_skill(
        parts.profile,
        parts.parent,
        parts.baseline,
        parts.skill_patch,
        parts.revoked,
    )?;
    let (improver, mut i_origins) = compile_improver(
        parts.profile,
        parts.parent_strategy,
        parts.baseline_strategy,
        parts.improver_patch,
    )?;
    origins.append(&mut i_origins);
    if !parts.caps.allows(&skill.required_capabilities) {
        return Err(Error::Forbidden);
    }
    let mut bundle = ResolvedBundle {
        schema_version: BUNDLE_SCHEMA.into(),
        profile_id: parts.profile.id.clone(),
        parent_digest: parts.profile.parent_digest.clone(),
        baseline_digest: parts.profile.baseline_digest.clone(),
        skill,
        improver,
        origins,
        digest: String::new(),
    };
    bundle.digest = fingerprint(&bundle)?;
    Ok(bundle)
}

fn apply_leaf<T: Clone + Serialize>(
    field: &str,
    op: &PatchOp<T>,
    parent: &T,
    baseline: &T,
    parent_digest: &str,
    baseline_digest: &str,
    origins: &mut Vec<ValueOrigin>,
) -> Result<T> {
    field_contract(field)?;
    let value = op.apply(parent, baseline);
    let (layer, source) = match op {
        PatchOp::Inherit => (OriginLayer::Parent, parent_digest),
        PatchOp::ResetToBaseline => (OriginLayer::Baseline, baseline_digest),
        PatchOp::Set { .. } => (OriginLayer::Explicit, "set"),
    };
    origins.push(ValueOrigin {
        field: field.into(),
        layer,
        source_digest: source.into(),
        op: op.name().into(),
        value_digest: fingerprint(&value)?,
    });
    Ok(value)
}

pub fn project_run(
    bundle: &ResolvedBundle,
    snapshot: &SystemSnapshot,
    evolution_enabled: bool,
) -> Result<RunProjection> {
    if !evolution_enabled
        || (bundle.skill.content.is_empty() && bundle.skill.applicability.is_empty())
    {
        return Ok(RunProjection {
            bundle_digest: bundle.digest.clone(),
            instructions: Vec::new(),
            tools: snapshot.tools.clone(),
        });
    }
    Ok(RunProjection {
        bundle_digest: bundle.digest.clone(),
        instructions: vec![bundle.skill.content.clone()],
        tools: snapshot.tools.clone(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityLevel {
    ToolOnly,
    Attached,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedReceipt {
    pub offered: Vec<String>,
    pub attached: Vec<String>,
    pub used: Vec<String>,
    pub verified_benefit: Vec<String>,
    pub bundle_digest: String,
    pub request_digest: String,
    pub capability_level: CapabilityLevel,
    pub truncated: bool,
    pub attested_by: String,
}

impl AppliedReceipt {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.bundle_digest)?;
        identifier(&self.request_digest)?;
        identifier(&self.attested_by)?;
        fn subset(inner: &[String], outer: &[String], msg: &str) -> Result<()> {
            if inner.iter().all(|x| outer.contains(x)) {
                Ok(())
            } else {
                Err(Error::Invalid(msg.into()))
            }
        }
        subset(
            &self.attached,
            &self.offered,
            "attached must be subset of offered",
        )?;
        subset(
            &self.used,
            &self.attached,
            "used must be subset of attached",
        )?;
        subset(
            &self.verified_benefit,
            &self.used,
            "verified_benefit must be subset of used",
        )?;
        if self.capability_level == CapabilityLevel::ToolOnly
            && (!self.attached.is_empty()
                || !self.used.is_empty()
                || !self.verified_benefit.is_empty())
        {
            return Err(Error::Invalid(
                "tool-only receipts may only claim offered".into(),
            ));
        }
        if self.truncated && !self.used.is_empty() {
            return Err(Error::Invalid(
                "truncated projection cannot claim used".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceCoverage {
    Supported,
    RuntimeOwned,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceItem {
    pub name: String,
    pub coverage: SurfaceCoverage,
    pub mapped_field: Option<String>,
    pub consumer: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSurfaceManifest {
    pub schema_version: String,
    pub host: String,
    pub host_version: String,
    pub adapter_version: String,
    pub source_digest: String,
    pub items: Vec<SurfaceItem>,
}

impl HostSurfaceManifest {
    pub fn validate_against_extraction(&self, extracted: &[String]) -> Result<()> {
        if self.schema_version != SURFACE_SCHEMA {
            return Err(Error::Invalid("unsupported host surface schema".into()));
        }
        identifier(&self.host)?;
        text(&self.host_version, "host_version", 128)?;
        text(&self.adapter_version, "adapter_version", 128)?;
        identifier(&self.source_digest)?;
        if extracted.is_empty() {
            return Err(Error::Invalid(
                "empty extraction is not a successful cover".into(),
            ));
        }
        let classified: BTreeSet<&str> = self.items.iter().map(|i| i.name.as_str()).collect();
        for name in extracted {
            if !classified.contains(name.as_str()) {
                return Err(Error::Invalid(format!(
                    "extracted field {name} is unclassified"
                )));
            }
        }
        for item in &self.items {
            identifier(&item.name)?;
            text(&item.reason, "reason", 512)?;
            match item.coverage {
                SurfaceCoverage::Supported => {
                    let field = item.mapped_field.as_deref().ok_or_else(|| {
                        Error::Invalid("supported item needs mapped_field".into())
                    })?;
                    field_contract(field)?;
                    if item.consumer.as_deref().unwrap_or("").is_empty() {
                        return Err(Error::Invalid("supported item needs a consumer".into()));
                    }
                }
                SurfaceCoverage::RuntimeOwned | SurfaceCoverage::Unsupported => {
                    if item.mapped_field.is_some() {
                        return Err(Error::Invalid(
                            "runtime_owned/unsupported items cannot map candidate fields".into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn model_tool_names() -> &'static [&'static str] {
    &MODEL_TOOLS
}

pub fn admin_ops() -> &'static [&'static str] {
    &[
        "experiment.register",
        "evaluation.start",
        "evaluation.status",
        "exploration.start",
        "replay.run",
        "curriculum.step",
        "meta.start",
    ]
}

pub fn reject_admin_as_model_tool(name: &str) -> Result<()> {
    if admin_ops().contains(&name) {
        return Err(Error::Forbidden);
    }
    if MODEL_TOOLS.contains(&name) {
        return Ok(());
    }
    Err(Error::Invalid("unknown model tool".into()))
}

pub fn conflicting_writers(slots: &BTreeMap<String, Vec<String>>) -> Result<()> {
    for (slot, writers) in slots {
        if writers.len() > 1 {
            return Err(Error::Conflict(format!("conflicting_writers:{slot}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Application, Feedback, Inspect, Prepare, Proposal};

    fn profile() -> Profile {
        Profile {
            id: "p1".into(),
            evolution_enabled: true,
            parent_digest: "parent1".into(),
            baseline_digest: "base1".into(),
        }
    }

    fn caps_all() -> HostCapabilities {
        HostCapabilities {
            available: ["fs_read".into()].into(),
            granted: ["fs_read".into()].into(),
        }
    }

    fn sample_bundle() -> ResolvedBundle {
        let profile = profile();
        let parent = SkillSnapshot::empty();
        let baseline = SkillSnapshot::empty();
        let parent_strategy = Strategy::default();
        let baseline_strategy = Strategy::default();
        let skill_patch = SkillPatch::default();
        let improver_patch = ImproverPatch::default();
        let caps = caps_all();
        let revoked = BTreeSet::new();
        compile_bundle(CompileParts {
            profile: &profile,
            parent: &parent,
            baseline: &baseline,
            parent_strategy: &parent_strategy,
            baseline_strategy: &baseline_strategy,
            skill_patch: &skill_patch,
            improver_patch: &improver_patch,
            caps: &caps,
            revoked: &revoked,
        })
        .unwrap()
    }

    #[test]
    fn v1_five_requests_still_deny_unknown_fields() {
        assert!(
            serde_json::from_str::<Prepare>(r#"{"request_key":"k","goal":"x","actor":"a"}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<Feedback>(
            r#"{"request_key":"k","run_id":"r","outcome":"success","details":"d","role":"admin"}"#
        )
        .is_err());
        assert!(
            serde_json::from_str::<Inspect>(r#"{"kind":"run","id":"x","namespace":"n"}"#).is_err()
        );
        assert!(serde_json::from_str::<Application>(
            r#"{"request_key":"k","run_id":"r","candidate_id":"c","stage":"returned","evidence_ref":"e","actor":"a"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Proposal>(r#"{"request_key":"k"}"#).is_err());
        assert_eq!(V1_REQUEST_KINDS.len(), 5);
    }

    #[test]
    fn four_model_tools_and_no_admin() {
        assert_eq!(MODEL_TOOLS.len(), 4);
        assert!(reject_admin_as_model_tool("evo_prepare").is_ok());
        assert!(reject_admin_as_model_tool("evaluation.start").is_err());
        assert!(reject_admin_as_model_tool("budget.grant").is_err());
    }

    #[test]
    fn inherit_reset_and_set_differ() {
        let parent = SkillSnapshot {
            content: "P".into(),
            applicability: "ap".into(),
            counterexample: "cx".into(),
            required_capabilities: vec!["fs_read".into()],
            dependencies: Vec::new(),
        };
        let baseline = SkillSnapshot {
            content: "B".into(),
            ..parent.clone()
        };
        let mut patch = SkillPatch::default();
        let (inherited, _) =
            compile_skill(&profile(), &parent, &baseline, &patch, &BTreeSet::new()).unwrap();
        assert_eq!(inherited.content, "P");
        patch.content = PatchOp::ResetToBaseline;
        let (reset, _) =
            compile_skill(&profile(), &parent, &baseline, &patch, &BTreeSet::new()).unwrap();
        assert_eq!(reset.content, "B");
        patch.content = PatchOp::Set {
            value: String::new(),
        };
        let (explicit_empty, _) =
            compile_skill(&profile(), &parent, &baseline, &patch, &BTreeSet::new()).unwrap();
        assert_eq!(explicit_empty.content, "");
        assert_ne!(explicit_empty.content, inherited.content);
    }

    #[test]
    fn bare_null_rejected() {
        assert!(serde_json::from_str::<SkillPatch>(r#"{"content":null}"#).is_err());
    }

    #[test]
    fn unknown_and_unconsumed_fields_rejected() {
        assert!(serde_json::from_str::<SkillPatch>(r#"{"settings":{}}"#).is_err());
        assert!(field_contract("settings.theme").is_err());
        assert!(assert_candidate_may_write(ArtifactKind::Skill, "namespace").is_err());
        assert!(assert_candidate_may_write(ArtifactKind::Skill, "improver.instruction").is_err());
    }

    #[test]
    fn revoked_dependency_and_missing_capability_fail() {
        let parent = SkillSnapshot {
            content: "P".into(),
            applicability: "a".into(),
            counterexample: "c".into(),
            required_capabilities: vec!["fs_read".into()],
            dependencies: vec!["dep1".into()],
        };
        let patch = SkillPatch::default();
        let mut revoked = BTreeSet::new();
        revoked.insert("dep1".into());
        assert!(compile_skill(&profile(), &parent, &parent, &patch, &revoked).is_err());
        let caps = HostCapabilities {
            available: ["fs_read".into()].into(),
            granted: BTreeSet::new(),
        };
        assert!(!caps.allows(&["fs_read".into()]));
    }

    #[test]
    fn tool_only_cannot_claim_used() {
        let r = AppliedReceipt {
            offered: vec!["evo_prepare".into()],
            attached: vec!["evo_prepare".into()],
            used: vec!["evo_prepare".into()],
            verified_benefit: Vec::new(),
            bundle_digest: "b".into(),
            request_digest: "r".into(),
            capability_level: CapabilityLevel::ToolOnly,
            truncated: false,
            attested_by: "host".into(),
        };
        assert!(r.validate().is_err());
    }

    #[test]
    fn truncated_cannot_claim_used() {
        let r = AppliedReceipt {
            offered: vec!["s1".into()],
            attached: vec!["s1".into()],
            used: vec!["s1".into()],
            verified_benefit: Vec::new(),
            bundle_digest: "b".into(),
            request_digest: "r".into(),
            capability_level: CapabilityLevel::Attached,
            truncated: true,
            attested_by: "host".into(),
        };
        assert!(r.validate().is_err());
    }

    #[test]
    fn no_change_projection_keeps_host_tools() {
        let snapshot = SystemSnapshot {
            schema_version: "rsia.system_snapshot.v2".into(),
            profile_id: "p1".into(),
            host_id: "ref".into(),
            host_version: "0.1.0".into(),
            model_id: "m".into(),
            tools: vec!["read".into()],
            mandatory_context_digest: "ctx".into(),
        };
        let bundle = sample_bundle();
        let proj = project_run(&bundle, &snapshot, false).unwrap();
        assert!(proj.instructions.is_empty());
        assert_eq!(proj.tools, snapshot.tools);
    }

    #[test]
    fn empty_surface_extraction_fails() {
        let m = HostSurfaceManifest {
            schema_version: SURFACE_SCHEMA.into(),
            host: "reference".into(),
            host_version: "0.1.0".into(),
            adapter_version: "0.1.0".into(),
            source_digest: "src".into(),
            items: Vec::new(),
        };
        assert!(m.validate_against_extraction(&[]).is_err());
    }

    #[test]
    fn supported_without_consumer_fails() {
        let m = HostSurfaceManifest {
            schema_version: SURFACE_SCHEMA.into(),
            host: "reference".into(),
            host_version: "0.1.0".into(),
            adapter_version: "0.1.0".into(),
            source_digest: "src".into(),
            items: vec![SurfaceItem {
                name: "instruction".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("skill.content".into()),
                consumer: None,
                reason: "slot".into(),
            }],
        };
        assert!(
            m.validate_against_extraction(&["instruction".into()])
                .is_err()
        );
    }

    #[test]
    fn conflicting_writers_rejected() {
        let mut slots = BTreeMap::new();
        slots.insert("instruction".into(), vec!["s1".into(), "s2".into()]);
        assert!(conflicting_writers(&slots).is_err());
    }

    #[test]
    fn same_inputs_same_bundle_digest() {
        let a = sample_bundle();
        let b = sample_bundle();
        assert_eq!(a.digest, b.digest);
    }
}
