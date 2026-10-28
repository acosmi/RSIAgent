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
    // E14 increment 1 (plan §5.2, §9.1): the only open mechanism class is the
    // exploration policy; its five fields are read by `decide_elastic` and by
    // nothing else. Caps, simulation and width are control plane and have no
    // entry here, so they stay "no consumer" for a candidate.
    FieldContract {
        path: "improver.exploration_policy.significant_gain_micros",
        owner: FieldOwner::ImproverCandidate,
        consumer: "exploration.decide_elastic",
    },
    FieldContract {
        path: "improver.exploration_policy.stagnation_abs_gain_micros",
        owner: FieldOwner::ImproverCandidate,
        consumer: "exploration.decide_elastic",
    },
    FieldContract {
        path: "improver.exploration_policy.stagnation_window",
        owner: FieldOwner::ImproverCandidate,
        consumer: "exploration.decide_elastic",
    },
    FieldContract {
        path: "improver.exploration_policy.max_focus_actions",
        owner: FieldOwner::ImproverCandidate,
        consumer: "exploration.decide_elastic",
    },
    FieldContract {
        path: "improver.exploration_policy.fairness_wait_rounds",
        owner: FieldOwner::ImproverCandidate,
        consumer: "exploration.decide_elastic",
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
    // Check both supplied inputs before any leaf can replace or discard their references.
    for dep in parent.dependencies.iter().chain(&baseline.dependencies) {
        if revoked.contains(dep) {
            return Err(Error::Conflict("dependency_revoked".into()));
        }
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
    // Require context labels even for all-Set patches; this does not bind input content.
    if profile.parent_digest.is_empty() || profile.baseline_digest.is_empty() {
        return Err(Error::Invalid(
            "parent and baseline digests are required".into(),
        ));
    }
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

/// Replays a v2 bundle against the complete authenticated P/B values. The
/// caller supplies authority; this function proves the deterministic value
/// contract and never grants Store, release or model authority.
pub fn replay_resolved_bundle(
    bundle: &ResolvedBundle,
    parent: &SkillSnapshot,
    baseline: &SkillSnapshot,
    parent_strategy: &Strategy,
    baseline_strategy: &Strategy,
    caps: &HostCapabilities,
    revoked: &BTreeSet<String>,
) -> Result<()> {
    let mut origins = BTreeMap::new();
    for origin in &bundle.origins {
        if origins.insert(origin.field.as_str(), origin).is_some() {
            return Err(Error::Conflict("duplicate bundle origin".into()));
        }
    }
    const FIELDS: [&str; 8] = [
        "skill.content",
        "skill.applicability",
        "skill.counterexample",
        "skill.required_capabilities",
        "skill.dependencies",
        "improver.instruction",
        "improver.max_candidates",
        "improver.max_rounds",
    ];
    if origins.len() != FIELDS.len() || FIELDS.iter().any(|field| !origins.contains_key(field)) {
        return Err(Error::Invalid(
            "bundle requires exactly eight known origins".into(),
        ));
    }
    fn leaf<T: Clone>(
        origin: &ValueOrigin,
        value: &T,
        parent: &str,
        baseline: &str,
    ) -> Result<PatchOp<T>> {
        match (
            origin.layer,
            origin.op.as_str(),
            origin.source_digest.as_str(),
        ) {
            (OriginLayer::Parent, "inherit", source) if source == parent => Ok(PatchOp::Inherit),
            (OriginLayer::Baseline, "reset_to_baseline", source) if source == baseline => {
                Ok(PatchOp::ResetToBaseline)
            }
            (OriginLayer::Explicit, "set", "set") => Ok(PatchOp::Set {
                value: value.clone(),
            }),
            _ => Err(Error::Conflict(
                "origin operation/source/layer mismatch".into(),
            )),
        }
    }
    let p = bundle.parent_digest.as_str();
    let b = bundle.baseline_digest.as_str();
    let skill_patch = SkillPatch {
        content: leaf(origins["skill.content"], &bundle.skill.content, p, b)?,
        applicability: leaf(
            origins["skill.applicability"],
            &bundle.skill.applicability,
            p,
            b,
        )?,
        counterexample: leaf(
            origins["skill.counterexample"],
            &bundle.skill.counterexample,
            p,
            b,
        )?,
        required_capabilities: leaf(
            origins["skill.required_capabilities"],
            &bundle.skill.required_capabilities,
            p,
            b,
        )?,
        dependencies: leaf(
            origins["skill.dependencies"],
            &bundle.skill.dependencies,
            p,
            b,
        )?,
    };
    let improver_patch = ImproverPatch {
        instruction: leaf(
            origins["improver.instruction"],
            &bundle.improver.instruction,
            p,
            b,
        )?,
        max_candidates: leaf(
            origins["improver.max_candidates"],
            &bundle.improver.max_candidates,
            p,
            b,
        )?,
        max_rounds: leaf(
            origins["improver.max_rounds"],
            &bundle.improver.max_rounds,
            p,
            b,
        )?,
    };
    let rebuilt = compile_bundle(CompileParts {
        profile: &Profile {
            id: bundle.profile_id.clone(),
            evolution_enabled: true,
            parent_digest: p.into(),
            baseline_digest: b.into(),
        },
        parent,
        baseline,
        parent_strategy,
        baseline_strategy,
        skill_patch: &skill_patch,
        improver_patch: &improver_patch,
        caps,
        revoked,
    })?;
    if fingerprint(&rebuilt)? != fingerprint(bundle)? {
        return Err(Error::Conflict("complete bundle replay differs".into()));
    }
    Ok(())
}

/// Re-express a complete edit of a search parent relative to authenticated P.
/// Untouched search-parent fields are preserved, including references.
pub fn rebase_skill_patch(parent: &SkillSnapshot, desired: &SkillSnapshot) -> Result<SkillPatch> {
    parent.validate()?;
    desired.validate()?;
    fn op<T: Clone + PartialEq>(parent: &T, desired: &T) -> PatchOp<T> {
        if parent == desired {
            PatchOp::Inherit
        } else {
            PatchOp::Set {
                value: desired.clone(),
            }
        }
    }
    Ok(SkillPatch {
        content: op(&parent.content, &desired.content),
        applicability: op(&parent.applicability, &desired.applicability),
        counterexample: op(&parent.counterexample, &desired.counterexample),
        required_capabilities: op(
            &parent.required_capabilities,
            &desired.required_capabilities,
        ),
        dependencies: op(&parent.dependencies, &desired.dependencies),
    })
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        self.validate_frozen_v1_against_extraction(extracted)?;
        for item in &self.items {
            if item.coverage == SurfaceCoverage::Supported {
                let contract = field_contract(item.mapped_field.as_deref().unwrap_or(""))?;
                if item.consumer.as_deref() != Some(contract.consumer) {
                    return Err(Error::Invalid(
                        "supported item consumer differs from field contract".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Frozen v1 guards for historical reads and lineage, without current-use authority.
    pub fn validate_frozen_v1_against_extraction(&self, extracted: &[String]) -> Result<()> {
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
        // v1 keeps exact clones and byte identity, but one name cannot carry
        // different complete item contents into registration or live use.
        let mut items_by_name: BTreeMap<&str, &SurfaceItem> = BTreeMap::new();
        for item in &self.items {
            if let Some(previous) = items_by_name.insert(item.name.as_str(), item)
                && previous != item
            {
                return Err(Error::Invalid(format!(
                    "conflicting host surface items for name {}",
                    item.name
                )));
            }
        }
        // Check old names only after the existing schema, coverage, and item guards.
        let extracted_names: BTreeSet<&str> = extracted.iter().map(String::as_str).collect();
        for item in &self.items {
            if !extracted_names.contains(item.name.as_str()) {
                return Err(Error::Invalid(format!(
                    "host surface field {} is missing from extraction",
                    item.name
                )));
            }
        }
        Ok(())
    }
}

pub fn model_tool_names() -> &'static [&'static str] {
    &MODEL_TOOLS
}

pub fn admin_ops() -> &'static [&'static str] {
    // Single source: the dispatch-plane projection of the §6.1.2 registry
    // (58 original dispatch names + control.review). The formal-authority and
    // offline planes are deliberately not admin_ops, so neither classify nor
    // the CLI manage gate admits them.
    crate::management_registry::dispatch_names()
}

pub fn reject_admin_as_model_tool(name: &str) -> Result<()> {
    // Every registered management name on any plane (73) is rejected as a
    // model tool; only the four model tools are accepted.
    if crate::management_registry::lookup(name).is_some() {
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

/// Checks complete caller-supplied bytes only; this is neither receipt attestation
/// nor evidence that a bundle/request was loaded from an authorized producer.
/// The historical `AppliedReceipt::validate` contract remains unchanged.
pub fn verify_applied_receipt_content_v3(
    receipt: &AppliedReceipt,
    actual_bundle_bytes: &[u8],
    actual_request_bytes: &[u8],
) -> Result<()> {
    content_digest_format_v3(&receipt.bundle_digest)?;
    content_digest_format_v3(&receipt.request_digest)?;
    receipt.validate()?;
    if receipt.bundle_digest != crate::hash(actual_bundle_bytes)
        || receipt.request_digest != crate::hash(actual_request_bytes)
    {
        return Err(Error::Conflict("receipt content digest mismatch".into()));
    }
    Ok(())
}

/// Resource limits apply only to the new reviewed-source/v2 JSON entry points.
/// The default budget uses sorted object keys, compact JSON strings and exact
/// decimal tokens (zero is 0, exponent zero is omitted). This restricted form
/// is comparison data, not RFC8785 and not the original source digest bytes.
pub const REVIEWED_HOST_JSON_MAX_UTF8_BYTES: usize = 262_144;
pub const REVIEWED_HOST_JSON_MAX_NODES: usize = 32_768;
pub const REVIEWED_HOST_JSON_MAX_DEPTH: usize = 256;
pub const HOST_DEFAULT_NORMALIZED_MAX_BYTES: usize = 4096;

fn reviewed_resource(name: &str) -> Error {
    Error::Invalid(format!("reviewed host JSON resource limit: {name}"))
}

// Proposed reviewed_fixture-only content helpers. No registration or authority.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewedDecimalV1 {
    negative: bool,
    coefficient: String,
    exponent: i64,
}
fn reviewed_invalid() -> Error {
    Error::Invalid("invalid reviewed host source/default".into())
}
fn reviewed_number_end(raw: &str, start: usize) -> Result<(usize, bool)> {
    let b = raw.as_bytes();
    let mut p = start;
    if b.get(p) == Some(&b'-') {
        p += 1;
    }
    match b.get(p) {
        Some(b'0') => {
            p += 1;
            if b.get(p).is_some_and(u8::is_ascii_digit) {
                return Err(reviewed_invalid());
            }
        }
        Some(b'1'..=b'9') => {
            p += 1;
            while b.get(p).is_some_and(u8::is_ascii_digit) {
                p += 1;
            }
        }
        _ => return Err(reviewed_invalid()),
    }
    let mut integer = true;
    if b.get(p) == Some(&b'.') {
        integer = false;
        p += 1;
        let first = p;
        while b.get(p).is_some_and(u8::is_ascii_digit) {
            p += 1;
        }
        if p == first {
            return Err(reviewed_invalid());
        }
    }
    if matches!(b.get(p), Some(b'e' | b'E')) {
        integer = false;
        p += 1;
        if matches!(b.get(p), Some(b'+' | b'-')) {
            p += 1;
        }
        let first = p;
        while b.get(p).is_some_and(u8::is_ascii_digit) {
            p += 1;
        }
        if p == first {
            return Err(reviewed_invalid());
        }
    }
    Ok((p, integer))
}
fn reviewed_decimal(raw: &str) -> Result<ReviewedDecimalV1> {
    if reviewed_number_end(raw, 0)?.0 != raw.len() {
        return Err(reviewed_invalid());
    }
    let negative = raw.starts_with('-');
    let body = raw.strip_prefix('-').unwrap_or(raw);
    let (mantissa, exp) = match body.find(['e', 'E']) {
        Some(p) => (
            &body[..p],
            body[p + 1..]
                .parse::<i64>()
                .map_err(|_| reviewed_invalid())?,
        ),
        None => (body, 0),
    };
    let fraction = mantissa.find('.').map_or(0, |p| mantissa.len() - p - 1);
    let fraction = i64::try_from(fraction).map_err(|_| reviewed_invalid())?;
    let exp = exp.checked_sub(fraction).ok_or_else(reviewed_invalid)?;
    let digits = mantissa
        .bytes()
        .filter(|v| *v != b'.')
        .map(char::from)
        .collect::<String>();
    let digits = digits.trim_start_matches('0');
    // Grammar and checked exponent/fraction parsing precede the zero rule.
    if digits.is_empty() {
        return Ok(ReviewedDecimalV1 {
            negative: false,
            coefficient: "0".into(),
            exponent: 0,
        });
    }
    let coefficient = digits.trim_end_matches('0');
    let trailing =
        i64::try_from(digits.len() - coefficient.len()).map_err(|_| reviewed_invalid())?;
    let exponent = exp.checked_add(trailing).ok_or_else(reviewed_invalid)?;
    Ok(ReviewedDecimalV1 {
        negative,
        coefficient: coefficient.into(),
        exponent,
    })
}
impl ReviewedDecimalV1 {
    fn comparison_token(&self) -> String {
        if self.coefficient == "0" {
            "0".into()
        } else if self.exponent == 0 {
            format!(
                "{}{}",
                if self.negative { "-" } else { "" },
                self.coefficient
            )
        } else {
            format!(
                "{}{}e{}",
                if self.negative { "-" } else { "" },
                self.coefficient,
                self.exponent
            )
        }
    }
}
#[derive(Debug, Clone)]
enum ReviewedJsonNode {
    Null,
    Bool(bool),
    String(String),
    Number {
        number: serde_json::Number,
        decimal: ReviewedDecimalV1,
        integer_token: bool,
    },
    Array(Vec<usize>),
    Object(BTreeMap<String, usize>),
}
struct ReviewedJsonTree {
    nodes: Vec<ReviewedJsonNode>,
    root: usize,
}
#[derive(Clone, Copy)]
enum ReviewedReadState {
    ArrayFirst,
    ArrayValue,
    ArrayNext,
    ObjectFirst,
    ObjectKey,
    ObjectColon,
    ObjectValue,
    ObjectNext,
}
enum ReviewedMembers {
    Array(Vec<usize>),
    Object(BTreeMap<String, usize>),
}
struct ReviewedReadFrame {
    state: ReviewedReadState,
    members: ReviewedMembers,
    key: Option<String>,
}
struct ReviewedJsonCursor<'a> {
    raw: &'a str,
    pos: usize,
    nodes: Vec<ReviewedJsonNode>,
}
impl ReviewedJsonCursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.raw.as_bytes().get(self.pos).copied()
    }
    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }
    fn string(&mut self) -> Result<String> {
        if self.peek() != Some(b'"') {
            return Err(reviewed_invalid());
        }
        let start = self.pos;
        self.pos += 1;
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    self.pos = self.pos.checked_add(2).ok_or_else(reviewed_invalid)?;
                    if self.pos > self.raw.len() {
                        return Err(reviewed_invalid());
                    }
                }
                Some(_) => self.pos += 1,
                None => return Err(reviewed_invalid()),
            }
        }
        serde_json::from_str(&self.raw[start..self.pos]).map_err(|_| reviewed_invalid())
    }
    fn literal(&mut self, token: &str) -> Result<()> {
        if !self.raw[self.pos..].starts_with(token) {
            return Err(reviewed_invalid());
        }
        self.pos += token.len();
        Ok(())
    }
    fn node(&mut self, node: ReviewedJsonNode) -> usize {
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }
    fn attach(frames: &mut [ReviewedReadFrame], root: &mut Option<usize>, id: usize) -> Result<()> {
        match frames.last_mut() {
            Some(frame) => match &mut frame.members {
                ReviewedMembers::Array(items) => {
                    if !matches!(
                        frame.state,
                        ReviewedReadState::ArrayFirst | ReviewedReadState::ArrayValue
                    ) {
                        return Err(reviewed_invalid());
                    }
                    items.push(id);
                    frame.state = ReviewedReadState::ArrayNext;
                }
                ReviewedMembers::Object(items) => {
                    if !matches!(frame.state, ReviewedReadState::ObjectValue) {
                        return Err(reviewed_invalid());
                    }
                    let key = frame.key.take().ok_or_else(reviewed_invalid)?;
                    if items.insert(key, id).is_some() {
                        return Err(reviewed_invalid());
                    }
                    frame.state = ReviewedReadState::ObjectNext;
                }
            },
            None => {
                if root.replace(id).is_some() {
                    return Err(reviewed_invalid());
                }
            }
        }
        Ok(())
    }
    fn close(
        &mut self,
        frames: &mut Vec<ReviewedReadFrame>,
        root: &mut Option<usize>,
    ) -> Result<()> {
        let frame = frames.pop().ok_or_else(reviewed_invalid)?;
        self.pos += 1;
        let node = match frame.members {
            ReviewedMembers::Array(v) => ReviewedJsonNode::Array(v),
            ReviewedMembers::Object(v) => ReviewedJsonNode::Object(v),
        };
        let id = self.node(node);
        Self::attach(frames, root, id)
    }
    fn parse(raw: &str) -> Result<ReviewedJsonTree> {
        Self::parse_with_default_root(raw, false)
    }
    fn parse_with_default_root(raw: &str, default_root: bool) -> Result<ReviewedJsonTree> {
        reviewed_json_preflight(raw, default_root)?;
        let mut p = ReviewedJsonCursor {
            raw,
            pos: 0,
            nodes: Vec::new(),
        };
        let mut frames: Vec<ReviewedReadFrame> = Vec::new();
        let mut root = None;
        loop {
            p.whitespace();
            match frames.last().map(|v| v.state) {
                None if root.is_some() => {
                    if p.pos != raw.len() {
                        return Err(reviewed_invalid());
                    }
                    return Ok(ReviewedJsonTree {
                        nodes: p.nodes,
                        root: root.ok_or_else(reviewed_invalid)?,
                    });
                }
                Some(ReviewedReadState::ArrayFirst) if p.peek() == Some(b']') => {
                    p.close(&mut frames, &mut root)?;
                    continue;
                }
                Some(ReviewedReadState::ArrayNext) => {
                    match p.peek() {
                        Some(b',') => {
                            p.pos += 1;
                            frames.last_mut().ok_or_else(reviewed_invalid)?.state =
                                ReviewedReadState::ArrayValue;
                        }
                        Some(b']') => p.close(&mut frames, &mut root)?,
                        _ => return Err(reviewed_invalid()),
                    }
                    continue;
                }
                Some(ReviewedReadState::ObjectFirst | ReviewedReadState::ObjectKey) => {
                    if matches!(
                        frames.last().map(|v| v.state),
                        Some(ReviewedReadState::ObjectFirst)
                    ) && p.peek() == Some(b'}')
                    {
                        p.close(&mut frames, &mut root)?;
                        continue;
                    }
                    let key = p.string()?;
                    let frame = frames.last_mut().ok_or_else(reviewed_invalid)?;
                    let ReviewedMembers::Object(items) = &frame.members else {
                        return Err(reviewed_invalid());
                    };
                    if items.contains_key(&key) {
                        return Err(reviewed_invalid());
                    }
                    frame.key = Some(key);
                    frame.state = ReviewedReadState::ObjectColon;
                    continue;
                }
                Some(ReviewedReadState::ObjectColon) => {
                    if p.peek() != Some(b':') {
                        return Err(reviewed_invalid());
                    }
                    p.pos += 1;
                    frames.last_mut().ok_or_else(reviewed_invalid)?.state =
                        ReviewedReadState::ObjectValue;
                    continue;
                }
                Some(ReviewedReadState::ObjectNext) => {
                    match p.peek() {
                        Some(b',') => {
                            p.pos += 1;
                            frames.last_mut().ok_or_else(reviewed_invalid)?.state =
                                ReviewedReadState::ObjectKey;
                        }
                        Some(b'}') => p.close(&mut frames, &mut root)?,
                        _ => return Err(reviewed_invalid()),
                    }
                    continue;
                }
                _ => {}
            }
            let node = match p.peek() {
                Some(b'[') => {
                    p.pos += 1;
                    frames.push(ReviewedReadFrame {
                        state: ReviewedReadState::ArrayFirst,
                        members: ReviewedMembers::Array(Vec::new()),
                        key: None,
                    });
                    continue;
                }
                Some(b'{') => {
                    p.pos += 1;
                    frames.push(ReviewedReadFrame {
                        state: ReviewedReadState::ObjectFirst,
                        members: ReviewedMembers::Object(BTreeMap::new()),
                        key: None,
                    });
                    continue;
                }
                Some(b'"') => ReviewedJsonNode::String(p.string()?),
                Some(b't') => {
                    p.literal("true")?;
                    ReviewedJsonNode::Bool(true)
                }
                Some(b'f') => {
                    p.literal("false")?;
                    ReviewedJsonNode::Bool(false)
                }
                Some(b'n') => {
                    p.literal("null")?;
                    ReviewedJsonNode::Null
                }
                Some(b'-' | b'0'..=b'9') => {
                    let start = p.pos;
                    let (end, integer_token) = reviewed_number_end(raw, start)?;
                    let token = &raw[start..end];
                    let decimal = reviewed_decimal(token)?;
                    let number: serde_json::Number =
                        serde_json::from_str(token).map_err(|_| reviewed_invalid())?;
                    let roundtrip =
                        serde_json::to_string(&number).map_err(|_| reviewed_invalid())?;
                    if decimal != reviewed_decimal(&roundtrip)? {
                        return Err(reviewed_invalid());
                    }
                    p.pos = end;
                    ReviewedJsonNode::Number {
                        number,
                        decimal,
                        integer_token,
                    }
                }
                _ => return Err(reviewed_invalid()),
            };
            let id = p.node(node);
            Self::attach(&mut frames, &mut root, id)?;
        }
    }
}
fn reviewed_string_json_len(value: &str) -> Result<usize> {
    value.chars().try_fold(2_usize, |size, ch| {
        let extra = match ch {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            ch if ch <= '\u{1f}' => 6,
            ch => ch.len_utf8(),
        };
        size.checked_add(extra).ok_or_else(reviewed_invalid)
    })
}

// First pass retains bounded frame/key metadata only. It counts normalized
// default bytes before the second pass allocates any complete value/node tree.
struct ReviewedBudgetFrame {
    state: ReviewedReadState,
    object: bool,
    keys: BTreeSet<String>,
    key: Option<String>,
    default_object: bool,
    budgeted: bool,
    normalized_len: usize,
    items: usize,
}
fn reviewed_budget_add(frame: &mut ReviewedBudgetFrame, size: usize) -> Result<()> {
    frame.normalized_len = frame
        .normalized_len
        .checked_add(size)
        .ok_or_else(reviewed_invalid)?;
    if frame.budgeted && frame.normalized_len > HOST_DEFAULT_NORMALIZED_MAX_BYTES {
        return Err(reviewed_resource("normalized default bytes"));
    }
    Ok(())
}
fn reviewed_budget_attach(
    frames: &mut [ReviewedBudgetFrame],
    root: &mut bool,
    size: usize,
) -> Result<()> {
    if let Some(frame) = frames.last_mut() {
        if frame.object {
            if !matches!(frame.state, ReviewedReadState::ObjectValue) {
                return Err(reviewed_invalid());
            }
            frame.key.take().ok_or_else(reviewed_invalid)?;
            frame.state = ReviewedReadState::ObjectNext;
        } else {
            if !matches!(
                frame.state,
                ReviewedReadState::ArrayFirst | ReviewedReadState::ArrayValue
            ) {
                return Err(reviewed_invalid());
            }
            if frame.items > 0 {
                reviewed_budget_add(frame, 1)?;
            }
            frame.state = ReviewedReadState::ArrayNext;
        }
        reviewed_budget_add(frame, size)?;
        frame.items += 1;
    } else if std::mem::replace(root, true) {
        return Err(reviewed_invalid());
    }
    Ok(())
}
fn reviewed_json_preflight(raw: &str, default_root: bool) -> Result<()> {
    if raw.len() > REVIEWED_HOST_JSON_MAX_UTF8_BYTES {
        return Err(reviewed_resource("UTF-8 bytes"));
    }
    let mut cursor = ReviewedJsonCursor {
        raw,
        pos: 0,
        nodes: Vec::new(),
    };
    let mut frames: Vec<ReviewedBudgetFrame> = Vec::new();
    let mut root = false;
    let mut nodes = 0_usize;
    loop {
        cursor.whitespace();
        let mut closing = false;
        match frames.last().map(|frame| frame.state) {
            None if root => {
                return if cursor.pos == raw.len() {
                    Ok(())
                } else {
                    Err(reviewed_invalid())
                };
            }
            Some(ReviewedReadState::ArrayFirst) if cursor.peek() == Some(b']') => closing = true,
            Some(ReviewedReadState::ArrayNext) => match cursor.peek() {
                Some(b',') => {
                    cursor.pos += 1;
                    frames.last_mut().ok_or_else(reviewed_invalid)?.state =
                        ReviewedReadState::ArrayValue;
                    continue;
                }
                Some(b']') => closing = true,
                _ => return Err(reviewed_invalid()),
            },
            Some(ReviewedReadState::ObjectFirst | ReviewedReadState::ObjectKey) => {
                if matches!(
                    frames.last().map(|frame| frame.state),
                    Some(ReviewedReadState::ObjectFirst)
                ) && cursor.peek() == Some(b'}')
                {
                    closing = true;
                } else {
                    let key = cursor.string()?;
                    let frame = frames.last_mut().ok_or_else(reviewed_invalid)?;
                    if frame.keys.contains(&key) {
                        return Err(reviewed_invalid());
                    }
                    let key_len = reviewed_string_json_len(&key)?;
                    reviewed_budget_add(frame, key_len + 1 + usize::from(!frame.keys.is_empty()))?;
                    frame.keys.insert(key.clone());
                    frame.key = Some(key);
                    frame.state = ReviewedReadState::ObjectColon;
                    continue;
                }
            }
            Some(ReviewedReadState::ObjectColon) => {
                if cursor.peek() != Some(b':') {
                    return Err(reviewed_invalid());
                }
                cursor.pos += 1;
                frames.last_mut().ok_or_else(reviewed_invalid)?.state =
                    ReviewedReadState::ObjectValue;
                continue;
            }
            Some(ReviewedReadState::ObjectNext) => match cursor.peek() {
                Some(b',') => {
                    cursor.pos += 1;
                    frames.last_mut().ok_or_else(reviewed_invalid)?.state =
                        ReviewedReadState::ObjectKey;
                    continue;
                }
                Some(b'}') => closing = true,
                _ => return Err(reviewed_invalid()),
            },
            _ => {}
        }
        if closing {
            cursor.pos += 1;
            let frame = frames.pop().ok_or_else(reviewed_invalid)?;
            reviewed_budget_attach(&mut frames, &mut root, frame.normalized_len)?;
            continue;
        }
        nodes += 1;
        if nodes > REVIEWED_HOST_JSON_MAX_NODES {
            return Err(reviewed_resource("nodes"));
        }
        let inherited_budget = frames.last().is_some_and(|frame| {
            frame.budgeted || (frame.default_object && frame.key.as_deref() == Some("value"))
        });
        let size = match cursor.peek() {
            Some(b'[' | b'{') => {
                if frames.len() >= REVIEWED_HOST_JSON_MAX_DEPTH {
                    return Err(reviewed_resource("depth"));
                }
                let object = cursor.peek() == Some(b'{');
                let default_object = object
                    && (frames
                        .last()
                        .is_some_and(|frame| frame.key.as_deref() == Some("default"))
                        || (frames.is_empty() && default_root));
                frames.push(ReviewedBudgetFrame {
                    state: if object {
                        ReviewedReadState::ObjectFirst
                    } else {
                        ReviewedReadState::ArrayFirst
                    },
                    object,
                    keys: BTreeSet::new(),
                    key: None,
                    default_object,
                    budgeted: inherited_budget,
                    normalized_len: 2,
                    items: 0,
                });
                cursor.pos += 1;
                continue;
            }
            Some(b'"') => reviewed_string_json_len(&cursor.string()?)?,
            Some(b't') => {
                cursor.literal("true")?;
                4
            }
            Some(b'f') => {
                cursor.literal("false")?;
                5
            }
            Some(b'n') => {
                cursor.literal("null")?;
                4
            }
            Some(b'-' | b'0'..=b'9') => {
                let end = reviewed_number_end(raw, cursor.pos)?.0;
                let decimal = reviewed_decimal(&raw[cursor.pos..end])?;
                cursor.pos = end;
                decimal.comparison_token().len()
            }
            _ => return Err(reviewed_invalid()),
        };
        if inherited_budget && size > HOST_DEFAULT_NORMALIZED_MAX_BYTES {
            return Err(reviewed_resource("normalized default bytes"));
        }
        reviewed_budget_attach(&mut frames, &mut root, size)?;
    }
}

#[derive(Debug, Clone)]
enum ReviewedSemanticNodeV1 {
    Null,
    Bool(bool),
    String(String),
    Number(ReviewedDecimalV1),
    Array(Vec<usize>),
    Object(BTreeMap<String, usize>),
}
/// Comparison-only tree, with private indices produced by the finite extractor.
#[derive(Debug, Clone)]
pub struct ReviewedSemanticValueV1 {
    nodes: Vec<ReviewedSemanticNodeV1>,
    root: usize,
}
impl ReviewedSemanticValueV1 {
    pub fn same_value(&self, other: &Self) -> bool {
        let mut pending = vec![(self.root, other.root)];
        while let Some((a, b)) = pending.pop() {
            match (self.nodes.get(a), other.nodes.get(b)) {
                (Some(ReviewedSemanticNodeV1::Null), Some(ReviewedSemanticNodeV1::Null)) => {}
                (Some(ReviewedSemanticNodeV1::Bool(a)), Some(ReviewedSemanticNodeV1::Bool(b)))
                    if a == b => {}
                (
                    Some(ReviewedSemanticNodeV1::String(a)),
                    Some(ReviewedSemanticNodeV1::String(b)),
                ) if a == b => {}
                (
                    Some(ReviewedSemanticNodeV1::Number(a)),
                    Some(ReviewedSemanticNodeV1::Number(b)),
                ) if a == b => {}
                (
                    Some(ReviewedSemanticNodeV1::Array(a)),
                    Some(ReviewedSemanticNodeV1::Array(b)),
                ) => {
                    if a.len() != b.len() {
                        return false;
                    }
                    pending.extend(a.iter().copied().zip(b.iter().copied()));
                }
                (
                    Some(ReviewedSemanticNodeV1::Object(a)),
                    Some(ReviewedSemanticNodeV1::Object(b)),
                ) => {
                    if a.len() != b.len() {
                        return false;
                    }
                    for ((ak, av), (bk, bv)) in a.iter().zip(b.iter()) {
                        if ak != bk {
                            return false;
                        }
                        pending.push((*av, *bv));
                    }
                }
                _ => return false,
            }
        }
        true
    }
}
impl ReviewedSemanticValueV1 {
    fn comparison_utf8(&self) -> Result<String> {
        enum Job<'a> {
            Node(usize),
            Raw(&'static str),
            Key(&'a str),
        }
        let mut jobs = vec![Job::Node(self.root)];
        let mut out = String::new();
        let append = |out: &mut String, s: &str| -> Result<()> {
            if out
                .len()
                .checked_add(s.len())
                .ok_or_else(reviewed_invalid)?
                > HOST_DEFAULT_NORMALIZED_MAX_BYTES
            {
                return Err(reviewed_resource("normalized default bytes"));
            }
            out.push_str(s);
            Ok(())
        };
        while let Some(job) = jobs.pop() {
            match job {
                Job::Raw(v) => append(&mut out, v)?,
                Job::Key(v) => append(
                    &mut out,
                    &serde_json::to_string(v).map_err(|_| reviewed_invalid())?,
                )?,
                Job::Node(id) => match self.nodes.get(id).ok_or_else(reviewed_invalid)? {
                    ReviewedSemanticNodeV1::Null => append(&mut out, "null")?,
                    ReviewedSemanticNodeV1::Bool(v) => {
                        append(&mut out, if *v { "true" } else { "false" })?
                    }
                    ReviewedSemanticNodeV1::String(v) => append(
                        &mut out,
                        &serde_json::to_string(v).map_err(|_| reviewed_invalid())?,
                    )?,
                    ReviewedSemanticNodeV1::Number(v) => append(&mut out, &v.comparison_token())?,
                    ReviewedSemanticNodeV1::Array(items) => {
                        append(&mut out, "[")?;
                        jobs.push(Job::Raw("]"));
                        for (i, id) in items.iter().enumerate().rev() {
                            jobs.push(Job::Node(*id));
                            if i > 0 {
                                jobs.push(Job::Raw(","));
                            }
                        }
                    }
                    ReviewedSemanticNodeV1::Object(items) => {
                        append(&mut out, "{")?;
                        jobs.push(Job::Raw("}"));
                        for (i, (key, id)) in items.iter().enumerate().rev() {
                            jobs.push(Job::Node(*id));
                            jobs.push(Job::Raw(":"));
                            jobs.push(Job::Key(key));
                            if i > 0 {
                                jobs.push(Job::Raw(","));
                            }
                        }
                    }
                },
            }
        }
        Ok(out)
    }
}
impl ReviewedJsonTree {
    fn get(&self, id: usize) -> Result<&ReviewedJsonNode> {
        self.nodes.get(id).ok_or_else(reviewed_invalid)
    }
    fn semantic(&self, root: usize) -> Result<ReviewedSemanticValueV1> {
        let mut ids = BTreeMap::new();
        let mut work = vec![root];
        while let Some(id) = work.pop() {
            if ids.contains_key(&id) {
                continue;
            }
            let next = ids.len();
            ids.insert(id, next);
            match self.get(id)? {
                ReviewedJsonNode::Array(v) => work.extend(v.iter().copied()),
                ReviewedJsonNode::Object(v) => work.extend(v.values().copied()),
                _ => {}
            }
        }
        let lookup = |id: &usize| ids.get(id).copied().ok_or_else(reviewed_invalid);
        let mut nodes = vec![ReviewedSemanticNodeV1::Null; ids.len()];
        for (old, new) in &ids {
            nodes[*new] = match self.get(*old)? {
                ReviewedJsonNode::Null => ReviewedSemanticNodeV1::Null,
                ReviewedJsonNode::Bool(v) => ReviewedSemanticNodeV1::Bool(*v),
                ReviewedJsonNode::String(v) => ReviewedSemanticNodeV1::String(v.clone()),
                ReviewedJsonNode::Number { decimal, .. } => {
                    ReviewedSemanticNodeV1::Number(decimal.clone())
                }
                ReviewedJsonNode::Array(v) => {
                    ReviewedSemanticNodeV1::Array(v.iter().map(lookup).collect::<Result<Vec<_>>>()?)
                }
                ReviewedJsonNode::Object(v) => ReviewedSemanticNodeV1::Object(
                    v.iter()
                        .map(|(k, id)| Ok((k.clone(), lookup(id)?)))
                        .collect::<Result<BTreeMap<_, _>>>()?,
                ),
            };
        }
        Ok(ReviewedSemanticValueV1 {
            nodes,
            root: lookup(&root)?,
        })
    }
    fn object(&self, id: usize) -> Result<&BTreeMap<String, usize>> {
        match self.get(id)? {
            ReviewedJsonNode::Object(v) => Ok(v),
            _ => Err(reviewed_invalid()),
        }
    }
    fn string(&self, id: usize) -> Result<&str> {
        match self.get(id)? {
            ReviewedJsonNode::String(v) => Ok(v),
            _ => Err(reviewed_invalid()),
        }
    }
    fn array(&self, id: usize) -> Result<&[usize]> {
        match self.get(id)? {
            ReviewedJsonNode::Array(v) => Ok(v),
            _ => Err(reviewed_invalid()),
        }
    }
    fn exact_fields(&self, id: usize, keys: &[&str]) -> Result<&BTreeMap<String, usize>> {
        let m = self.object(id)?;
        if m.len() != keys.len() || keys.iter().any(|k| !m.contains_key(*k)) {
            return Err(reviewed_invalid());
        }
        Ok(m)
    }
    fn field(m: &BTreeMap<String, usize>, key: &str) -> Result<usize> {
        m.get(key).copied().ok_or_else(reviewed_invalid)
    }
    // Same JSON string serializer for keys and values. No f64 arithmetic.
    fn typed_wire_utf8(&self, root: usize) -> Result<String> {
        enum Job<'a> {
            Node(usize),
            Raw(&'static str),
            Key(&'a str),
        }
        let mut jobs = vec![Job::Node(root)];
        let mut out = String::new();
        let append = |out: &mut String, s: &str| -> Result<()> {
            out.len()
                .checked_add(s.len())
                .ok_or_else(reviewed_invalid)?;
            out.push_str(s);
            Ok(())
        };
        while let Some(job) = jobs.pop() {
            match job {
                Job::Raw(v) => append(&mut out, v)?,
                Job::Key(v) => append(
                    &mut out,
                    &serde_json::to_string(v).map_err(|_| reviewed_invalid())?,
                )?,
                Job::Node(id) => match self.get(id)? {
                    ReviewedJsonNode::Null => append(&mut out, "null")?,
                    ReviewedJsonNode::Bool(v) => {
                        append(&mut out, if *v { "true" } else { "false" })?
                    }
                    ReviewedJsonNode::String(v) => append(
                        &mut out,
                        &serde_json::to_string(v).map_err(|_| reviewed_invalid())?,
                    )?,
                    ReviewedJsonNode::Number { number, .. } => {
                        append(
                            &mut out,
                            &serde_json::to_string(number).map_err(|_| reviewed_invalid())?,
                        )?;
                    }
                    ReviewedJsonNode::Array(items) => {
                        append(&mut out, "[")?;
                        jobs.push(Job::Raw("]"));
                        for (i, id) in items.iter().enumerate().rev() {
                            jobs.push(Job::Node(*id));
                            if i > 0 {
                                jobs.push(Job::Raw(","));
                            }
                        }
                    }
                    ReviewedJsonNode::Object(items) => {
                        append(&mut out, "{")?;
                        jobs.push(Job::Raw("}"));
                        for (i, (key, id)) in items.iter().enumerate().rev() {
                            jobs.push(Job::Node(*id));
                            jobs.push(Job::Raw(":"));
                            jobs.push(Job::Key(key));
                            if i > 0 {
                                jobs.push(Job::Raw(","));
                            }
                        }
                    }
                },
            }
        }
        Ok(out)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedHostSourceEnvelopeV1 {
    pub schema_version: String,
    pub source_kind: String,
    pub source_schema: String,
    pub extractor_version: String,
    pub original_utf8: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewedValueTypeV1 {
    Bool,
    Integer,
    String,
    Enum,
    StringList,
    Object,
    Opaque,
}
/// Transient validated content; neither this nor its equality grants support.
#[derive(Debug, Clone)]
pub enum ReviewedDefaultV1 {
    None,
    Unknown,
    Value(ReviewedTypedDefaultV1),
}
#[derive(Debug, Clone)]
pub struct ReviewedTypedDefaultV1 {
    // Actual serde Number JSON, never the e-format comparison projection.
    typed_json_utf8: String,
    semantic: ReviewedSemanticValueV1,
    comparison_utf8: String,
}
impl ReviewedTypedDefaultV1 {
    pub fn typed_json_utf8(&self) -> &str {
        &self.typed_json_utf8
    }
    /// The normalized comparison and budget form; never original-source bytes.
    pub fn comparison_projection(&self) -> &str {
        &self.comparison_utf8
    }
}
pub fn reviewed_defaults_same_value(a: &ReviewedDefaultV1, b: &ReviewedDefaultV1) -> bool {
    match (a, b) {
        (ReviewedDefaultV1::None, ReviewedDefaultV1::None)
        | (ReviewedDefaultV1::Unknown, ReviewedDefaultV1::Unknown) => true,
        (ReviewedDefaultV1::Value(a), ReviewedDefaultV1::Value(b)) => {
            a.semantic.same_value(&b.semantic)
        }
        _ => false,
    }
}
#[derive(Debug, Clone)]
pub struct ReviewedHostDeclarationV1 {
    pub name: String,
    pub value_type: ReviewedValueTypeV1,
    pub enum_values: Vec<String>,
    pub default: ReviewedDefaultV1,
}
#[derive(Debug, Clone)]
pub struct ReviewedHostSourceProjectionV1 {
    pub source: ReviewedHostSourceEnvelopeV1,
    pub source_digest: String,
    pub host: String,
    pub host_version: String,
    pub declarations: Vec<ReviewedHostDeclarationV1>,
}
fn reviewed_default(
    tree: &ReviewedJsonTree,
    id: usize,
    ty: ReviewedValueTypeV1,
    choices: &[String],
) -> Result<ReviewedDefaultV1> {
    let m = tree.object(id)?;
    let kind = tree.string(ReviewedJsonTree::field(m, "kind")?)?;
    match kind {
        "none" => {
            tree.exact_fields(id, &["kind"])?;
            Ok(ReviewedDefaultV1::None)
        }
        "unknown" => {
            tree.exact_fields(id, &["kind"])?;
            Ok(ReviewedDefaultV1::Unknown)
        }
        "value" => {
            tree.exact_fields(id, &["kind", "value"])?;
            let value = ReviewedJsonTree::field(m, "value")?;
            let valid = match (ty, tree.get(value)?) {
                (ReviewedValueTypeV1::Bool, ReviewedJsonNode::Bool(_)) => true,
                (
                    ReviewedValueTypeV1::Integer,
                    ReviewedJsonNode::Number {
                        number,
                        integer_token,
                        ..
                    },
                ) => *integer_token && (number.is_i64() || number.is_u64()),
                (ReviewedValueTypeV1::String, ReviewedJsonNode::String(_)) => true,
                (ReviewedValueTypeV1::Enum, ReviewedJsonNode::String(v)) => {
                    choices.binary_search(v).is_ok()
                }
                (ReviewedValueTypeV1::StringList, ReviewedJsonNode::Array(items)) => items
                    .iter()
                    .all(|id| matches!(tree.nodes.get(*id), Some(ReviewedJsonNode::String(_)))),
                (ReviewedValueTypeV1::Object, ReviewedJsonNode::Object(_)) => true,
                _ => false,
            };
            if !valid {
                return Err(reviewed_invalid());
            }
            let semantic = tree.semantic(value)?;
            let comparison_utf8 = semantic.comparison_utf8()?;
            let typed_json_utf8 = tree.typed_wire_utf8(value)?;
            Ok(ReviewedDefaultV1::Value(ReviewedTypedDefaultV1 {
                typed_json_utf8,
                semantic,
                comparison_utf8,
            }))
        }
        _ => Err(reviewed_invalid()),
    }
}
/// Parse only the exact software reviewed-source envelope. All JSON keys,
/// including escaped spellings, pass the same duplicate-key decoder.
pub fn parse_reviewed_host_source_json(raw: &str) -> Result<ReviewedHostSourceProjectionV1> {
    let tree = ReviewedJsonCursor::parse(raw)?;
    let m = tree.exact_fields(
        tree.root,
        &[
            "schema_version",
            "source_kind",
            "source_schema",
            "extractor_version",
            "original_utf8",
        ],
    )?;
    let string = |key: &str| -> Result<String> {
        Ok(tree.string(ReviewedJsonTree::field(m, key)?)?.to_owned())
    };
    let source = ReviewedHostSourceEnvelopeV1 {
        schema_version: string("schema_version")?,
        source_kind: string("source_kind")?,
        source_schema: string("source_schema")?,
        extractor_version: string("extractor_version")?,
        original_utf8: string("original_utf8")?,
    };
    extract_reviewed_host_source(&source)
}
/// Typed envelope input is data only. This helper re-extracts original_utf8;
/// it performs no Store/FS/registration/permission or consumer operation.
pub fn extract_reviewed_host_source(
    source: &ReviewedHostSourceEnvelopeV1,
) -> Result<ReviewedHostSourceProjectionV1> {
    if source.schema_version != "rsia.host_source_envelope.v1"
        || source.source_kind != "reviewed_fixture"
        || source.source_schema != "rsia.host_reviewed_declarations.v1"
        || source.extractor_version != "rsia.host_reviewed_declarations.extractor.v1"
    {
        return Err(reviewed_invalid());
    }
    let tree = ReviewedJsonCursor::parse(&source.original_utf8)?;
    let m = tree.exact_fields(
        tree.root,
        &["schema_version", "host", "host_version", "declarations"],
    )?;
    if tree.string(ReviewedJsonTree::field(m, "schema_version")?)? != source.source_schema {
        return Err(reviewed_invalid());
    }
    let host = tree.string(ReviewedJsonTree::field(m, "host")?)?.to_owned();
    let host_version = tree
        .string(ReviewedJsonTree::field(m, "host_version")?)?
        .to_owned();
    identifier(&host)?;
    text(&host_version, "host_version", 128)?;
    let items = tree.array(ReviewedJsonTree::field(m, "declarations")?)?;
    if items.is_empty() {
        return Err(reviewed_invalid());
    }
    let mut declarations = Vec::with_capacity(items.len());
    let mut names = BTreeSet::new();
    for id in items {
        let d = tree.exact_fields(*id, &["name", "value_type", "enum_values", "default"])?;
        let name = tree.string(ReviewedJsonTree::field(d, "name")?)?.to_owned();
        identifier(&name)?;
        if !names.insert(name.clone()) {
            return Err(reviewed_invalid());
        }
        let value_type = match tree.string(ReviewedJsonTree::field(d, "value_type")?)? {
            "bool" => ReviewedValueTypeV1::Bool,
            "integer" => ReviewedValueTypeV1::Integer,
            "string" => ReviewedValueTypeV1::String,
            "enum" => ReviewedValueTypeV1::Enum,
            "string_list" => ReviewedValueTypeV1::StringList,
            "object" => ReviewedValueTypeV1::Object,
            "opaque" => ReviewedValueTypeV1::Opaque,
            _ => return Err(reviewed_invalid()),
        };
        let enum_values = tree
            .array(ReviewedJsonTree::field(d, "enum_values")?)?
            .iter()
            .map(|id| Ok(tree.string(*id)?.to_owned()))
            .collect::<Result<Vec<_>>>()?;
        if value_type == ReviewedValueTypeV1::Enum {
            if enum_values.is_empty() || enum_values.windows(2).any(|p| p[0] >= p[1]) {
                return Err(reviewed_invalid());
            }
        } else if !enum_values.is_empty() {
            return Err(reviewed_invalid());
        }
        let default = reviewed_default(
            &tree,
            ReviewedJsonTree::field(d, "default")?,
            value_type,
            &enum_values,
        )?;
        declarations.push(ReviewedHostDeclarationV1 {
            name,
            value_type,
            enum_values,
            default,
        });
    }
    Ok(ReviewedHostSourceProjectionV1 {
        source: source.clone(),
        source_digest: crate::hash(source.original_utf8.as_bytes()),
        host,
        host_version,
        declarations,
    })
}

/// Current typed surface data. This is content, never registration authority.
pub const SURFACE_SCHEMA_V2: &str = "rsia.host_surface.v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostSurfaceValueTypeV2 {
    Bool,
    Integer,
    String,
    Enum,
    StringList,
    Object,
    Opaque,
}

impl HostSurfaceValueTypeV2 {
    fn reviewed(self) -> ReviewedValueTypeV1 {
        match self {
            Self::Bool => ReviewedValueTypeV1::Bool,
            Self::Integer => ReviewedValueTypeV1::Integer,
            Self::String => ReviewedValueTypeV1::String,
            Self::Enum => ReviewedValueTypeV1::Enum,
            Self::StringList => ReviewedValueTypeV1::StringList,
            Self::Object => ReviewedValueTypeV1::Object,
            Self::Opaque => ReviewedValueTypeV1::Opaque,
        }
    }
    fn from_reviewed(value: ReviewedValueTypeV1) -> Self {
        match value {
            ReviewedValueTypeV1::Bool => Self::Bool,
            ReviewedValueTypeV1::Integer => Self::Integer,
            ReviewedValueTypeV1::String => Self::String,
            ReviewedValueTypeV1::Enum => Self::Enum,
            ReviewedValueTypeV1::StringList => Self::StringList,
            ReviewedValueTypeV1::Object => Self::Object,
            ReviewedValueTypeV1::Opaque => Self::Opaque,
        }
    }
}

/// Typed data only. Generic `Deserialize` cannot prove the lexical precision
/// of original JSON numbers: a deserializer can round a token before invoking
/// `visit_f64`. Production consumers must first use the strict raw JSON
/// decoder/preflight, then validate their complete DTO, identity and source.
#[derive(Debug, Clone)]
pub enum HostSurfaceDefaultV2 {
    None,
    Unknown,
    Value { value: serde_json::Value },
}

// Generic DTO deserialization rejects duplicate decoded keys at every level.
// Raw-JSON content consumers use the flat decoders below as well: a generic
// serde Deserializer may impose its own depth/number representation limits.
// In particular, visit_f64 cannot recover or authenticate the original token.
struct ReviewedValueSeed<'a> {
    depth: usize,
    budget: &'a mut usize,
}
fn reviewed_seed_charge<E: serde::de::Error>(
    budget: &mut usize,
    size: usize,
) -> std::result::Result<(), E> {
    *budget = budget
        .checked_add(size)
        .ok_or_else(|| E::custom("normalized default bytes overflow"))?;
    if *budget > HOST_DEFAULT_NORMALIZED_MAX_BYTES {
        return Err(E::custom(
            "reviewed host JSON resource limit: normalized default bytes",
        ));
    }
    Ok(())
}
impl<'de> serde::de::DeserializeSeed<'de> for ReviewedValueSeed<'_> {
    type Value = serde_json::Value;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> serde::de::Visitor<'de> for ReviewedValueSeed<'_> {
    type Value = serde_json::Value;
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a bounded JSON value with unique decoded keys")
    }
    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
        reviewed_seed_charge(self.budget, 4)?;
        Ok(serde_json::Value::Null)
    }
    fn visit_bool<E: serde::de::Error>(self, value: bool) -> std::result::Result<Self::Value, E> {
        reviewed_seed_charge(self.budget, if value { 4 } else { 5 })?;
        Ok(serde_json::Value::Bool(value))
    }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> std::result::Result<Self::Value, E> {
        self.number(serde_json::Number::from(value))
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> std::result::Result<Self::Value, E> {
        self.number(serde_json::Number::from(value))
    }
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> std::result::Result<Self::Value, E> {
        self.number(
            serde_json::Number::from_f64(value)
                .ok_or_else(|| E::custom("non-finite JSON number"))?,
        )
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
        reviewed_seed_charge(
            self.budget,
            reviewed_string_json_len(value).map_err(E::custom)?,
        )?;
        Ok(serde_json::Value::String(value.to_owned()))
    }
    fn visit_string<E: serde::de::Error>(
        self,
        value: String,
    ) -> std::result::Result<Self::Value, E> {
        reviewed_seed_charge(
            self.budget,
            reviewed_string_json_len(&value).map_err(E::custom)?,
        )?;
        Ok(serde_json::Value::String(value))
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        use serde::de::Error as _;
        if self.depth >= REVIEWED_HOST_JSON_MAX_DEPTH {
            return Err(A::Error::custom("reviewed host JSON resource limit: depth"));
        }
        reviewed_seed_charge(self.budget, 2)?;
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(ReviewedValueSeed {
            depth: self.depth + 1,
            budget: self.budget,
        })? {
            if !values.is_empty() {
                reviewed_seed_charge(self.budget, 1)?;
            }
            values.push(value);
        }
        Ok(serde_json::Value::Array(values))
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        use serde::de::Error as _;
        if self.depth >= REVIEWED_HOST_JSON_MAX_DEPTH {
            return Err(A::Error::custom("reviewed host JSON resource limit: depth"));
        }
        reviewed_seed_charge(self.budget, 2)?;
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom("duplicate decoded JSON key"));
            }
            reviewed_seed_charge(
                self.budget,
                reviewed_string_json_len(&key).map_err(A::Error::custom)?
                    + 1
                    + usize::from(!values.is_empty()),
            )?;
            let value = map.next_value_seed(ReviewedValueSeed {
                depth: self.depth + 1,
                budget: self.budget,
            })?;
            values.insert(key, value);
        }
        Ok(serde_json::Value::Object(values))
    }
}
impl ReviewedValueSeed<'_> {
    fn number<E: serde::de::Error>(
        self,
        value: serde_json::Number,
    ) -> std::result::Result<serde_json::Value, E> {
        let decimal = reviewed_decimal(&value.to_string()).map_err(E::custom)?;
        reviewed_seed_charge(self.budget, decimal.comparison_token().len())?;
        Ok(serde_json::Value::Number(value))
    }
}

// An externally constructed Value must be bounded before recursive serde/Clone
// is called. Parsed trees are already bounded by the streaming preflight.
fn reviewed_value_resources(value: &serde_json::Value) -> Result<()> {
    let mut pending = vec![(value, 0_usize)];
    let mut nodes = 0_usize;
    let mut size = 0_usize;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if nodes > REVIEWED_HOST_JSON_MAX_NODES {
            return Err(reviewed_resource("nodes"));
        }
        let extra = match value {
            serde_json::Value::Null => 4,
            serde_json::Value::Bool(value) => {
                if *value {
                    4
                } else {
                    5
                }
            }
            serde_json::Value::Number(value) => reviewed_decimal(&value.to_string())?
                .comparison_token()
                .len(),
            serde_json::Value::String(value) => reviewed_string_json_len(value)?,
            serde_json::Value::Array(items) => {
                if depth >= REVIEWED_HOST_JSON_MAX_DEPTH {
                    return Err(reviewed_resource("depth"));
                }
                // Check the structural lower bound before extending the work list.
                if items.len() > HOST_DEFAULT_NORMALIZED_MAX_BYTES {
                    return Err(reviewed_resource("normalized default bytes"));
                }
                pending.extend(items.iter().map(|item| (item, depth + 1)));
                2 + items.len().saturating_sub(1)
            }
            serde_json::Value::Object(items) => {
                if depth >= REVIEWED_HOST_JSON_MAX_DEPTH {
                    return Err(reviewed_resource("depth"));
                }
                if items.len() > HOST_DEFAULT_NORMALIZED_MAX_BYTES {
                    return Err(reviewed_resource("normalized default bytes"));
                }
                let mut fields = 2 + items.len().saturating_sub(1);
                for (key, value) in items {
                    fields = fields
                        .checked_add(reviewed_string_json_len(key)? + 1)
                        .ok_or_else(reviewed_invalid)?;
                    if fields > HOST_DEFAULT_NORMALIZED_MAX_BYTES {
                        return Err(reviewed_resource("normalized default bytes"));
                    }
                    pending.push((value, depth + 1));
                }
                fields
            }
        };
        size = size.checked_add(extra).ok_or_else(reviewed_invalid)?;
        if size > HOST_DEFAULT_NORMALIZED_MAX_BYTES {
            return Err(reviewed_resource("normalized default bytes"));
        }
    }
    Ok(())
}
impl Serialize for HostSurfaceDefaultV2 {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct(
            "HostSurfaceDefaultV2",
            if matches!(self, Self::Value { .. }) {
                2
            } else {
                1
            },
        )?;
        match self {
            Self::None => state.serialize_field("kind", "none")?,
            Self::Unknown => state.serialize_field("kind", "unknown")?,
            Self::Value { value } => {
                reviewed_value_resources(value).map_err(serde::ser::Error::custom)?;
                state.serialize_field("kind", "value")?;
                state.serialize_field("value", value)?;
            }
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for HostSurfaceDefaultV2 {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DefaultVisitor;

        impl<'de> serde::de::Visitor<'de> for DefaultVisitor {
            type Value = HostSurfaceDefaultV2;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a strict host default object")
            }

            fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let mut kind: Option<String> = None;
                // Some(Null) is a present value, distinct from a missing field.
                let mut value: Option<serde_json::Value> = None;
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "kind" => {
                            if kind.is_some() {
                                return Err(<M::Error as serde::de::Error>::duplicate_field(
                                    "kind",
                                ));
                            }
                            kind = Some(map.next_value()?);
                        }
                        "value" => {
                            if value.is_some() {
                                return Err(<M::Error as serde::de::Error>::duplicate_field(
                                    "value",
                                ));
                            }
                            let mut budget = 0;
                            value = Some(map.next_value_seed(ReviewedValueSeed {
                                depth: 0,
                                budget: &mut budget,
                            })?);
                        }
                        _ => {
                            return Err(<M::Error as serde::de::Error>::unknown_field(
                                &field,
                                &["kind", "value"],
                            ));
                        }
                    }
                }
                let kind =
                    kind.ok_or_else(|| <M::Error as serde::de::Error>::missing_field("kind"))?;
                match kind.as_str() {
                    "none" | "unknown" => {
                        if value.is_some() {
                            return Err(<M::Error as serde::de::Error>::unknown_field(
                                "value",
                                &["kind"],
                            ));
                        }
                        if kind == "none" {
                            Ok(HostSurfaceDefaultV2::None)
                        } else {
                            Ok(HostSurfaceDefaultV2::Unknown)
                        }
                    }
                    "value" => Ok(HostSurfaceDefaultV2::Value {
                        value: value.ok_or_else(|| {
                            <M::Error as serde::de::Error>::missing_field("value")
                        })?,
                    }),
                    _ => Err(<M::Error as serde::de::Error>::unknown_variant(
                        &kind,
                        &["none", "unknown", "value"],
                    )),
                }
            }
        }

        deserializer.deserialize_map(DefaultVisitor)
    }
}

/// Reuse the already checked flat decoder, without a second depth quota,
/// float conversion, string-valued default, or comparison-token persistence.
fn reviewed_tree_value(tree: &ReviewedJsonTree, root: usize) -> Result<serde_json::Value> {
    use serde_json::Value;
    let mut values: Vec<Option<Value>> = (0..tree.nodes.len()).map(|_| None).collect();
    let mut jobs = vec![(root, false)];
    while let Some((id, ready)) = jobs.pop() {
        let node = tree.get(id)?;
        if !ready {
            jobs.push((id, true));
            match node {
                ReviewedJsonNode::Array(items) => {
                    jobs.extend(items.iter().rev().map(|id| (*id, false)));
                }
                ReviewedJsonNode::Object(items) => {
                    jobs.extend(items.values().rev().map(|id| (*id, false)));
                }
                _ => {}
            }
            continue;
        }
        let value = match node {
            ReviewedJsonNode::Null => Value::Null,
            ReviewedJsonNode::Bool(value) => Value::Bool(*value),
            ReviewedJsonNode::String(value) => Value::String(value.clone()),
            ReviewedJsonNode::Number { number, .. } => Value::Number(number.clone()),
            ReviewedJsonNode::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|id| {
                        values
                            .get_mut(*id)
                            .and_then(Option::take)
                            .ok_or_else(reviewed_invalid)
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            ReviewedJsonNode::Object(items) => {
                let mut object = serde_json::Map::new();
                for (key, id) in items {
                    object.insert(
                        key.clone(),
                        values
                            .get_mut(*id)
                            .and_then(Option::take)
                            .ok_or_else(reviewed_invalid)?,
                    );
                }
                Value::Object(object)
            }
        };
        *values.get_mut(id).ok_or_else(reviewed_invalid)? = Some(value);
    }
    values
        .get_mut(root)
        .and_then(Option::take)
        .ok_or_else(reviewed_invalid)
}

/// Untrusted JSON preflight/parse with the v2 record schema label. This does
/// not validate record fields, identity, provenance or permissions. The consumer
/// must decode its complete versioned DTO and authenticate its actual source.
pub fn decode_host_surface_record_v2_json(raw: &str) -> Result<serde_json::Value> {
    let tree = ReviewedJsonCursor::parse(raw)?;
    let object = tree.object(tree.root)?;
    let schema = ReviewedJsonTree::field(object, "schema_version")?;
    if tree.string(schema)? != "rsia.host_surface_record.v2" {
        return Err(reviewed_invalid());
    }
    reviewed_tree_value(&tree, tree.root)
}

/// Raw default DTO parser. The same flat decoder rejects duplicate escaped
/// keys and inexact numbers and accepts legal depth 200 without serde's usual
/// string-deserializer recursion gate. Shape alone does not establish type.
pub fn decode_host_surface_default_v2_json(raw: &str) -> Result<HostSurfaceDefaultV2> {
    let tree = ReviewedJsonCursor::parse_with_default_root(raw, true)?;
    let object = tree.object(tree.root)?;
    let kind = tree.string(ReviewedJsonTree::field(object, "kind")?)?;
    match kind {
        "none" | "unknown" => {
            tree.exact_fields(tree.root, &["kind"])?;
            if kind == "none" {
                Ok(HostSurfaceDefaultV2::None)
            } else {
                Ok(HostSurfaceDefaultV2::Unknown)
            }
        }
        "value" => {
            tree.exact_fields(tree.root, &["kind", "value"])?;
            let value = reviewed_tree_value(&tree, ReviewedJsonTree::field(object, "value")?)?;
            reviewed_value_resources(&value)?;
            Ok(HostSurfaceDefaultV2::Value { value })
        }
        _ => Err(reviewed_invalid()),
    }
}

impl HostSurfaceDefaultV2 {
    fn from_reviewed(value: &ReviewedDefaultV1) -> Result<Self> {
        match value {
            ReviewedDefaultV1::None => Ok(Self::None),
            ReviewedDefaultV1::Unknown => Ok(Self::Unknown),
            ReviewedDefaultV1::Value(value) => {
                let tree = ReviewedJsonCursor::parse(value.typed_json_utf8())?;
                Ok(Self::Value {
                    value: reviewed_tree_value(&tree, tree.root)?,
                })
            }
        }
    }
    fn reviewed(
        &self,
        value_type: HostSurfaceValueTypeV2,
        choices: &[String],
    ) -> Result<ReviewedDefaultV1> {
        if let Self::Value { value } = self {
            reviewed_value_resources(value)?;
        }
        let raw = serde_json::to_string(self).map_err(|_| Error::Internal)?;
        let tree = ReviewedJsonCursor::parse(&raw)?;
        reviewed_default(&tree, tree.root, value_type.reviewed(), choices)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSurfaceDeclarationV2 {
    pub name: String,
    pub value_type: HostSurfaceValueTypeV2,
    pub enum_values: Vec<String>,
    pub default: HostSurfaceDefaultV2,
}

impl HostSurfaceDeclarationV2 {
    fn from_reviewed(value: &ReviewedHostDeclarationV1) -> Result<Self> {
        Ok(Self {
            name: value.name.clone(),
            value_type: HostSurfaceValueTypeV2::from_reviewed(value.value_type),
            enum_values: value.enum_values.clone(),
            default: HostSurfaceDefaultV2::from_reviewed(&value.default)?,
        })
    }
    fn matches_reviewed(&self, source: &ReviewedHostDeclarationV1) -> Result<bool> {
        Ok(self.name == source.name
            && self.value_type.reviewed() == source.value_type
            && self.enum_values == source.enum_values
            && reviewed_defaults_same_value(
                &self.default.reviewed(self.value_type, &self.enum_values)?,
                &source.default,
            ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSurfaceClassificationV2 {
    pub name: String,
    pub coverage: SurfaceCoverage,
    pub mapped_field: Option<String>,
    pub consumer: Option<String>,
    pub reason: String,
    pub renamed_from: Option<String>,
    pub test_ref: Option<String>,
    pub capability: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredHostSurfaceV2 {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSurfaceRegistrationV2 {
    pub adapter_version: String,
    pub classifications: Vec<HostSurfaceClassificationV2>,
    pub retired: Vec<RetiredHostSurfaceV2>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSurfaceItemV2 {
    pub name: String,
    pub coverage: SurfaceCoverage,
    pub mapped_field: Option<String>,
    pub consumer: Option<String>,
    pub reason: String,
    pub renamed_from: Option<String>,
    pub test_ref: Option<String>,
    pub capability: Option<String>,
    pub value_type: HostSurfaceValueTypeV2,
    pub enum_values: Vec<String>,
    pub default: HostSurfaceDefaultV2,
}

impl HostSurfaceItemV2 {
    fn declaration(&self) -> Result<HostSurfaceDeclarationV2> {
        if let HostSurfaceDefaultV2::Value { value } = &self.default {
            reviewed_value_resources(value)?;
        }
        Ok(HostSurfaceDeclarationV2 {
            name: self.name.clone(),
            value_type: self.value_type,
            enum_values: self.enum_values.clone(),
            default: self.default.clone(),
        })
    }
    fn validate_classification(&self) -> Result<()> {
        identifier(&self.name)?;
        text(&self.reason, "reason", 512)?;
        if let Some(name) = &self.renamed_from {
            identifier(name)?;
        }
        match self.coverage {
            SurfaceCoverage::Supported => {
                let field = self.mapped_field.as_deref().ok_or_else(reviewed_invalid)?;
                let contract = field_contract(field)?;
                if matches!(
                    contract.owner,
                    FieldOwner::ProtectedCore | FieldOwner::Diagnostics
                ) || self.consumer.as_deref() != Some(contract.consumer)
                    || self.value_type == HostSurfaceValueTypeV2::Opaque
                    || matches!(&self.default, HostSurfaceDefaultV2::Unknown)
                    || self
                        .test_ref
                        .as_deref()
                        .is_none_or(|value| value.trim().is_empty())
                {
                    return Err(reviewed_invalid());
                }
                identifier(self.capability.as_deref().ok_or_else(reviewed_invalid)?)?;
            }
            SurfaceCoverage::RuntimeOwned | SurfaceCoverage::Unsupported => {
                if self.mapped_field.is_some() {
                    return Err(reviewed_invalid());
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSurfaceManifestV2 {
    pub schema_version: String,
    pub host: String,
    pub host_version: String,
    pub adapter_version: String,
    pub source_kind: String,
    pub source_schema: String,
    pub extractor_version: String,
    pub source_envelope_digest: String,
    pub source_digest: String,
    pub items: Vec<HostSurfaceItemV2>,
    pub retired: Vec<RetiredHostSurfaceV2>,
}

/// Caller classifications carry no typed extraction, digest or computed diff.
pub fn host_surface_v2_from_source(
    source: &ReviewedHostSourceEnvelopeV1,
    registration: &HostSurfaceRegistrationV2,
) -> Result<(HostSurfaceManifestV2, Vec<HostSurfaceDeclarationV2>)> {
    let projection = extract_reviewed_host_source(source)?;
    text(&registration.adapter_version, "adapter_version", 128)?;
    let mut classifications = BTreeMap::new();
    for item in &registration.classifications {
        if classifications.insert(item.name.as_str(), item).is_some() {
            return Err(reviewed_invalid());
        }
    }
    if classifications.len() != projection.declarations.len() {
        return Err(reviewed_invalid());
    }
    let mut items = Vec::new();
    let mut extracted = Vec::new();
    for declaration in &projection.declarations {
        let typed = HostSurfaceDeclarationV2::from_reviewed(declaration)?;
        let class = classifications
            .get(typed.name.as_str())
            .ok_or_else(reviewed_invalid)?;
        let item = HostSurfaceItemV2 {
            name: typed.name.clone(),
            coverage: class.coverage,
            mapped_field: class.mapped_field.clone(),
            consumer: class.consumer.clone(),
            reason: class.reason.clone(),
            renamed_from: class.renamed_from.clone(),
            test_ref: class.test_ref.clone(),
            capability: class.capability.clone(),
            value_type: typed.value_type,
            enum_values: typed.enum_values.clone(),
            default: typed.default.clone(),
        };
        items.push(item);
        extracted.push(typed);
    }
    let manifest = HostSurfaceManifestV2 {
        schema_version: SURFACE_SCHEMA_V2.into(),
        host: projection.host,
        host_version: projection.host_version,
        adapter_version: registration.adapter_version.clone(),
        source_kind: source.source_kind.clone(),
        source_schema: source.source_schema.clone(),
        extractor_version: source.extractor_version.clone(),
        source_envelope_digest: fingerprint(source)?,
        source_digest: projection.source_digest,
        items,
        retired: registration.retired.clone(),
    };
    let names = extracted
        .iter()
        .map(|item| item.name.clone())
        .collect::<Vec<_>>();
    validate_host_surface_v2_source(&manifest, source, &names, &extracted)?;
    Ok((manifest, extracted))
}

/// Full original-source re-extraction is independent of all caller DTO fields.
pub fn validate_host_surface_v2_source(
    manifest: &HostSurfaceManifestV2,
    source: &ReviewedHostSourceEnvelopeV1,
    extracted: &[String],
    extracted_typed: &[HostSurfaceDeclarationV2],
) -> Result<()> {
    let actual = extract_reviewed_host_source(source)?;
    if manifest.schema_version != SURFACE_SCHEMA_V2
        || manifest.host != actual.host
        || manifest.host_version != actual.host_version
        || manifest.source_kind != source.source_kind
        || manifest.source_schema != source.source_schema
        || manifest.extractor_version != source.extractor_version
        || manifest.source_envelope_digest != fingerprint(source)?
        || manifest.source_digest != actual.source_digest
        || manifest.items.len() != actual.declarations.len()
        || extracted.len() != actual.declarations.len()
        || extracted_typed.len() != actual.declarations.len()
    {
        return Err(reviewed_invalid());
    }
    text(&manifest.adapter_version, "adapter_version", 128)?;
    let mut mapped_fields = BTreeSet::new();
    for (index, declaration) in actual.declarations.iter().enumerate() {
        let item = &manifest.items[index];
        if extracted[index] != declaration.name
            || !extracted_typed[index].matches_reviewed(declaration)?
            || !item.declaration()?.matches_reviewed(declaration)?
        {
            return Err(reviewed_invalid());
        }
        item.validate_classification()?;
        if item.coverage == SurfaceCoverage::Supported
            && !mapped_fields.insert(item.mapped_field.as_deref().ok_or_else(reviewed_invalid)?)
        {
            return Err(reviewed_invalid());
        }
    }
    let mut retired = BTreeSet::new();
    for item in &manifest.retired {
        identifier(&item.name)?;
        text(&item.reason, "retired reason", 512)?;
        if !retired.insert(item.name.as_str()) || extracted.iter().any(|name| name == &item.name) {
            return Err(reviewed_invalid());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSurfaceRenameV2 {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSurfaceUpgradeDiffV2 {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub renamed: Vec<HostSurfaceRenameV2>,
    pub type_changed: Vec<String>,
    pub enum_changed: Vec<String>,
    pub default_changed: Vec<String>,
    pub mapping_changed: Vec<String>,
    pub legacy_untyped: Vec<String>,
}

pub enum HostSurfacePredecessorV2<'a> {
    Legacy(&'a HostSurfaceManifest),
    Current(&'a HostSurfaceManifestV2),
}

/// Callers first authenticate/revalidate the actual predecessor in the same Session.
/// This computes data only; test_ref is registration metadata, not a smoke receipt.
pub fn host_surface_upgrade_diff_v2(
    current: &HostSurfaceManifestV2,
    predecessor: Option<HostSurfacePredecessorV2<'_>>,
) -> Result<HostSurfaceUpgradeDiffV2> {
    let mut legacy = BTreeMap::new();
    let mut typed = BTreeMap::new();
    match predecessor {
        None => {}
        Some(HostSurfacePredecessorV2::Legacy(previous)) => {
            for item in &previous.items {
                legacy.insert(item.name.as_str(), item);
            }
        }
        Some(HostSurfacePredecessorV2::Current(previous)) => {
            for item in &previous.items {
                typed.insert(item.name.as_str(), item);
            }
        }
    }
    let previous_names = legacy
        .keys()
        .chain(typed.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let current_names = current
        .items
        .iter()
        .map(|item| item.name.as_str())
        .collect::<BTreeSet<_>>();
    let retired = current
        .retired
        .iter()
        .map(|item| item.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut renamed = BTreeSet::new();
    let mut diff = HostSurfaceUpgradeDiffV2::default();
    for item in &current.items {
        let previous_name = match item.renamed_from.as_deref() {
            Some(name) => {
                if !previous_names.contains(name)
                    || current_names.contains(name)
                    || retired.contains(name)
                    || previous_names.contains(item.name.as_str())
                    || !renamed.insert(name)
                {
                    return Err(reviewed_invalid());
                }
                diff.renamed.push(HostSurfaceRenameV2 {
                    from: name.into(),
                    to: item.name.clone(),
                });
                name
            }
            None => item.name.as_str(),
        };
        if let Some(old) = typed.get(previous_name) {
            let type_changed = old.value_type != item.value_type;
            let enum_changed = old.enum_values != item.enum_values;
            let default_changed = !reviewed_defaults_same_value(
                &old.default.reviewed(old.value_type, &old.enum_values)?,
                &item.default.reviewed(item.value_type, &item.enum_values)?,
            );
            let mapping_changed = old.mapped_field != item.mapped_field
                || old.consumer != item.consumer
                || old.capability != item.capability;
            if type_changed {
                diff.type_changed.push(item.name.clone());
            }
            if enum_changed {
                diff.enum_changed.push(item.name.clone());
            }
            if default_changed {
                diff.default_changed.push(item.name.clone());
            }
            if mapping_changed {
                diff.mapping_changed.push(item.name.clone());
            }
            if old.coverage == SurfaceCoverage::Supported
                && item.coverage == SurfaceCoverage::Supported
                && (item.renamed_from.is_some()
                    || type_changed
                    || enum_changed
                    || default_changed
                    || mapping_changed)
                && old.test_ref == item.test_ref
            {
                return Err(Error::Invalid(
                    "changed supported surface needs a renewed test_ref".into(),
                ));
            }
        } else if let Some(old) = legacy.get(previous_name) {
            diff.legacy_untyped.push(item.name.clone());
            // v1 had no typed default or behavior-test field; never invent one.
            if old.coverage == SurfaceCoverage::Supported
                && item.coverage == SurfaceCoverage::Supported
                && item
                    .test_ref
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
            {
                return Err(reviewed_invalid());
            }
        } else {
            diff.added.push(item.name.clone());
        }
    }
    for name in &previous_names {
        if !current_names.contains(name) && !renamed.contains(name) {
            if !retired.contains(name) {
                return Err(reviewed_invalid());
            }
            diff.removed.push((*name).into());
        }
    }
    if retired.iter().any(|name| {
        !previous_names.contains(name) || current_names.contains(name) || renamed.contains(name)
    }) {
        return Err(reviewed_invalid());
    }
    diff.added.sort();
    diff.removed.sort();
    diff.renamed.sort();
    diff.type_changed.sort();
    diff.enum_changed.sort();
    diff.default_changed.sort();
    diff.mapping_changed.sort();
    diff.legacy_untyped.sort();
    Ok(diff)
}

/// Data-only membership predicate after source validation. Its boolean does
/// not grant a capability, attest a real host, or register support evidence.
pub fn host_surface_v2_allows_field(
    manifest: &HostSurfaceManifestV2,
    capabilities: &HostCapabilities,
    field: &str,
) -> bool {
    let effective = capabilities.effective();
    manifest.items.iter().any(|item| {
        item.coverage == SurfaceCoverage::Supported
            && item.mapped_field.as_deref() == Some(field)
            && item
                .capability
                .as_ref()
                .is_some_and(|capability| effective.contains(capability))
    })
}

#[cfg(test)]
mod digest_receipt_v3_controls {
    use super::*;

    #[test]
    fn old_receipt_identifier_domain_and_error_order_remain_unchanged() {
        let mut receipt = AppliedReceipt {
            offered: vec!["evo_prepare".into()],
            attached: vec![],
            used: vec![],
            verified_benefit: vec![],
            bundle_digest: "b".into(),
            request_digest: "r".into(),
            capability_level: CapabilityLevel::ToolOnly,
            truncated: false,
            attested_by: "host".into(),
        };
        receipt.validate().unwrap();
        let before = serde_json::to_vec(&receipt).unwrap();
        assert!(verify_applied_receipt_content_v3(&receipt, b"b", b"r").is_err());
        assert_eq!(serde_json::to_vec(&receipt).unwrap(), before);
        receipt.attached = receipt.offered.clone();
        receipt.used = receipt.offered.clone();
        assert!(matches!(receipt.validate(), Err(Error::Invalid(message))
            if message == "tool-only receipts may only claim offered"));
        receipt.bundle_digest = crate::hash(b"actual bundle");
        receipt.request_digest = crate::hash(b"actual request");
        receipt.attached.clear();
        receipt.used.clear();
        verify_applied_receipt_content_v3(&receipt, b"actual bundle", b"actual request").unwrap();
        assert!(
            matches!(verify_applied_receipt_content_v3(&receipt, b"other bundle", b"actual request"),
            Err(Error::Conflict(message)) if message == "receipt content digest mismatch")
        );
    }

    #[test]
    fn applied_receipt_v3_requires_digest_format_for_each_field() {
        let bundle_bytes = b"actual complete bundle bytes";
        let request_bytes = b"actual complete request bytes";
        let (bundle_digest, request_digest) =
            (crate::hash(bundle_bytes), crate::hash(request_bytes));
        let valid = AppliedReceipt {
            offered: Vec::new(),
            attached: Vec::new(),
            used: Vec::new(),
            verified_benefit: Vec::new(),
            bundle_digest,
            request_digest,
            capability_level: CapabilityLevel::ToolOnly,
            truncated: false,
            attested_by: "host".into(),
        };
        verify_applied_receipt_content_v3(&valid, bundle_bytes, request_bytes).unwrap();
        for request_field in [false, true] {
            let actual = if request_field {
                &valid.request_digest
            } else {
                &valid.bundle_digest
            };
            let uppercase = actual.to_ascii_uppercase();
            assert_ne!(&uppercase, actual);
            for bad in [
                "readable-name".to_string(),
                actual[..63].to_string(),
                uppercase,
                format!("sha256:{actual}"),
                "g".repeat(64),
            ] {
                let mut invalid = valid.clone();
                if request_field {
                    invalid.request_digest = bad;
                } else {
                    invalid.bundle_digest = bad;
                }
                assert!(matches!(
                    verify_applied_receipt_content_v3(&invalid, bundle_bytes, request_bytes),
                    Err(Error::Invalid(reason)) if reason == "invalid current content digest"
                ));
            }
        }
    }

    #[test]
    fn applied_receipt_old_non_digest_wire_roundtrips_without_upgrade() {
        // Parsing preserves the old b/r wire; strict new content checks are separate.
        const OLD: &str = r#"{"offered":["evo_prepare"],"attached":["evo_prepare"],"used":["evo_prepare"],"verified_benefit":[],"bundle_digest":"b","request_digest":"r","capability_level":"tool_only","truncated":false,"attested_by":"host"}"#;
        let old: AppliedReceipt = serde_json::from_str(OLD).unwrap();
        assert_eq!(serde_json::to_string(&old).unwrap(), OLD);
        assert_eq!(old.bundle_digest, "b");
        assert_eq!(old.request_digest, "r");
        assert!(matches!(
            verify_applied_receipt_content_v3(&old, b"bundle", b"request"),
            Err(Error::Invalid(reason)) if reason == "invalid current content digest"
        ));
    }
}

// Versioned explicit-leaf content identity only.
// The v2 compiler, wire and literal "set" remain unchanged for historical reads.
// A future consumer must independently resolve its actual P/B source;
// this pure value digest grants no storage, approval or model authority.
pub const BUNDLE_SCHEMA_V3: &str = "rsia.resolved_bundle.v3";
pub const BUNDLE_COMPILER_VERSION_V3: &str = "rsia.bundle_compiler.v3";

#[derive(Serialize)]
struct ExplicitFieldSourceV1<'a> {
    domain: &'static str,
    schema_version: &'static str,
    bundle_schema_version: &'static str,
    compiler_version: &'static str,
    field: &'a str,
    op: &'static str,
    value: &'a serde_json::Value,
}

fn validate_explicit_field_value_v3(field: &str, value: &serde_json::Value) -> Result<()> {
    let contract = field_contract(field)?;
    if !matches!(
        contract.owner,
        FieldOwner::SkillCandidate | FieldOwner::ImproverCandidate
    ) {
        return Err(Error::Forbidden);
    }
    let bad = || Error::Invalid("invalid explicit leaf source value".into());
    let string = || value.as_str().map(str::to_owned).ok_or_else(bad);
    let strings = || {
        value
            .as_array()
            .ok_or_else(bad)?
            .iter()
            .map(|item| item.as_str().map(str::to_owned).ok_or_else(bad))
            .collect::<Result<Vec<String>>>()
    };
    let byte = || {
        value
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(bad)
    };
    let mut skill = SkillSnapshot::empty();
    let mut strategy = Strategy::default();
    match field {
        "skill.content" => skill.content = string()?,
        "skill.applicability" => skill.applicability = string()?,
        "skill.counterexample" => skill.counterexample = string()?,
        "skill.required_capabilities" => skill.required_capabilities = strings()?,
        "skill.dependencies" => skill.dependencies = strings()?,
        "improver.instruction" => strategy.instruction = string()?,
        "improver.max_candidates" => strategy.max_candidates = byte()?,
        "improver.max_rounds" => strategy.max_rounds = byte()?,
        _ => {
            return Err(Error::Invalid(
                "field is not a v3 explicit bundle leaf".into(),
            ));
        }
    }
    skill.validate()?;
    strategy.validate()?;
    Ok(())
}

/// Recompute the chosen seven-field current Set source from the actual typed
/// value. Its version/domain/op are fixed here, never read as caller authority.
/// Value serialization preserves the registered integer/string/array domains;
/// no Unicode, line-ending or list-order normalization is performed.
pub fn explicit_field_source_v3<T: Serialize>(field: &str, actual_value: &T) -> Result<String> {
    let value = serde_json::to_value(actual_value).map_err(|_| Error::Internal)?;
    validate_explicit_field_value_v3(field, &value)?;
    crate::canonical_fingerprint(&ExplicitFieldSourceV1 {
        domain: "rsia.bundle.explicit_leaf",
        schema_version: "rsia.explicit_field_source.v1",
        bundle_schema_version: BUNDLE_SCHEMA_V3,
        compiler_version: BUNDLE_COMPILER_VERSION_V3,
        field,
        op: "set",
        value: &value,
    })
}

#[cfg(test)]
mod explicit_field_source_v3_controls {
    use super::*;
    use serde_json::{Value, json};

    fn material(field: &str, value: Value) -> Value {
        json!({
            "domain": "rsia.bundle.explicit_leaf",
            "schema_version": "rsia.explicit_field_source.v1",
            "bundle_schema_version": "rsia.resolved_bundle.v3",
            "compiler_version": "rsia.bundle_compiler.v3",
            "field": field,
            "op": "set",
            "value": value
        })
    }

    #[test]
    fn all_eight_actual_leaf_domains_use_the_complete_material() {
        for (field, value) in [
            ("skill.content", json!("body\r\n读写")),
            ("skill.applicability", json!("when needed")),
            ("skill.counterexample", json!("counterexample")),
            ("skill.required_capabilities", json!(["cap-a", "cap-b"])),
            ("skill.dependencies", json!(["source-a", "source-b"])),
            ("improver.instruction", json!("strategy")),
            ("improver.max_candidates", json!(4)),
            ("improver.max_rounds", json!(3)),
        ] {
            let digest = explicit_field_source_v3(field, &value).unwrap();
            assert_eq!(
                digest,
                crate::canonical_fingerprint(&material(field, value)).unwrap()
            );
            assert_eq!(digest.len(), 64);
            assert!(
                digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
        }
        assert_eq!(BUNDLE_SCHEMA, "rsia.resolved_bundle.v2");
    }

    #[test]
    fn actual_types_and_existing_value_bounds_refuse_single_faults() {
        explicit_field_source_v3("improver.max_candidates", &2_u8).unwrap();
        for value in [json!(null), json!("2"), json!(2.0), json!(-1), json!(256)] {
            assert!(matches!(
                explicit_field_source_v3("improver.max_candidates", &value),
                Err(Error::Invalid(message)) if message == "invalid explicit leaf source value"
            ));
        }
        for (field, value) in [
            ("improver.max_candidates", json!(0)),
            ("improver.max_candidates", json!(5)),
            ("improver.max_rounds", json!(0)),
            ("improver.max_rounds", json!(4)),
        ] {
            explicit_field_source_v3(field, &json!(1)).unwrap();
            assert!(matches!(
                explicit_field_source_v3(field, &value),
                Err(Error::Invalid(message)) if message == "unsupported or unbounded strategy"
            ));
        }
        explicit_field_source_v3("skill.dependencies", &vec!["source-a"]).unwrap();
        for value in [json!([null]), json!([1]), json!("source-a")] {
            assert!(matches!(
                explicit_field_source_v3("skill.dependencies", &value),
                Err(Error::Invalid(message)) if message == "invalid explicit leaf source value"
            ));
        }
        for field in [
            "namespace",
            "budget",
            "approval",
            "host.model",
            "host.tools",
        ] {
            assert!(matches!(
                explicit_field_source_v3(field, &json!("x")),
                Err(Error::Forbidden)
            ));
        }
    }

    #[test]
    fn actual_utf8_line_endings_and_reference_order_remain_distinct() {
        for (a, b) in [("line\r\nnext", "line\nnext"), ("é", "e\u{301}")] {
            assert_ne!(
                explicit_field_source_v3("skill.content", &a).unwrap(),
                explicit_field_source_v3("skill.content", &b).unwrap()
            );
        }
        for field in ["skill.dependencies", "skill.required_capabilities"] {
            assert_ne!(
                explicit_field_source_v3(field, &vec!["a", "b"]).unwrap(),
                explicit_field_source_v3(field, &vec!["b", "a"]).unwrap()
            );
        }
        assert_ne!(
            explicit_field_source_v3("skill.content", &"x").unwrap(),
            explicit_field_source_v3("skill.applicability", &"x").unwrap()
        );
    }

    #[test]
    fn each_of_the_seven_material_fields_participates_in_identity() {
        let original = material("skill.content", json!("x"));
        let digest = explicit_field_source_v3("skill.content", &"x").unwrap();
        assert_eq!(digest, crate::canonical_fingerprint(&original).unwrap());
        for (key, replacement) in [
            ("domain", json!("other.bundle.explicit_leaf")),
            ("schema_version", json!("rsia.explicit_field_source.v2")),
            ("bundle_schema_version", json!("rsia.resolved_bundle.v2")),
            ("compiler_version", json!("rsia.bundle_compiler.v2")),
            ("field", json!("skill.applicability")),
            ("op", json!("inherit")),
            ("value", json!("y")),
        ] {
            let mut changed = original.clone();
            changed[key] = replacement;
            assert_ne!(
                digest,
                crate::canonical_fingerprint(&changed).unwrap(),
                "{key}"
            );
        }
        // Arbitrary material hashes above are data, never accepted proof or
        // evidence that an external operation/domain/version is authorized.
    }
}

// Pure content only. Resolver, approval and owner checks remain consumer duties.
// These two exact, non-Deserialize content mirrors are transient, never registries.
// Their Serialize field order is the existing typed engine order; the engine
// adapter must compare to_vec(loaded original) with to_vec(complete mirror).
#[derive(Debug, Clone, Serialize)]
pub struct CompilerDependencyContentV1 {
    pub kind: String,
    pub id: String,
    pub schema_version: String,
    pub content_digest: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct FrozenCompilerProfileContentV1 {
    pub id: String,
    pub schema_version: String,
    pub namespace: String,
    pub profile_id: String,
    pub version: u64,
    pub host_registration_id: String,
    pub provider_registration_id: String,
    pub resource_instrumentation_id: Option<String>,
    pub system_snapshot: SystemSnapshot,
    pub capabilities: HostCapabilities,
    pub environment_digest: String,
    pub baseline_skill: SkillSnapshot,
    pub baseline_strategy: Strategy,
    pub dependencies: Vec<CompilerDependencyContentV1>,
    pub purpose: crate::evidence::Purpose,
    pub application_order: Vec<String>,
    pub authorized_by: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct EmptyRootContentV1 {
    pub id: String,
    pub schema_version: String,
    pub namespace: String,
    pub profile_registration_id: String,
    pub profile_digest: String,
    pub host_registration_id: String,
    pub skill: SkillSnapshot,
    pub strategy: Strategy,
    pub authorized_by: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedBundleV3 {
    pub schema_version: String,
    pub compiler_version: String,
    pub profile_id: String,
    pub parent_digest: String,
    pub baseline_digest: String,
    pub skill: SkillSnapshot,
    pub improver: Strategy,
    pub origins: Vec<ValueOrigin>,
    pub digest: String,
}
#[derive(Debug, Clone)]
pub enum ParentContentV3 {
    EmptyRoot(EmptyRootContentV1),
    CurrentBundle(ResolvedBundleV3),
}
// No Deserialize/Serialize, public fields, mutable getter, generic serializer
// hook, arbitrary body bytes, independent Skill/caps, or authority constructor.
#[derive(Debug, Clone)]
pub struct ProfileContentProofV3 {
    profile: Profile,
    parent: ParentContentV3,
    baseline: FrozenCompilerProfileContentV1,
}
fn content_digest_format_v3(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Invalid("invalid current content digest".into()));
    }
    Ok(())
}
fn empty_root_strategy_content_v3() -> Strategy {
    Strategy {
        schema_version: "evo.strategy.v1".into(),
        instruction: "Preserve the unchanged host baseline; no evolution instruction is applied."
            .into(),
        max_candidates: 1,
        max_rounds: 1,
        require_counterexample: true,
    }
}
impl ProfileContentProofV3 {
    /// Content integrity, never evidence that a body was registered/approved.
    /// Engine uses this only after its existing same-Session resolver and exact
    /// loaded-original/mirror typed-byte comparison. Caller JSON mints no gate.
    pub fn from_content(
        profile: Profile,
        parent: ParentContentV3,
        baseline: FrozenCompilerProfileContentV1,
    ) -> Result<Self> {
        identifier(&profile.id)?;
        content_digest_format_v3(&profile.parent_digest)?;
        content_digest_format_v3(&profile.baseline_digest)?;
        if baseline.schema_version != "rsia.frozen_compiler_profile.v1"
            || baseline.profile_id != profile.id
            || baseline.system_snapshot.profile_id != baseline.profile_id
            || baseline.version == 0
            || baseline.application_order != ["mandatory_context", "skill", "task"]
        {
            return Err(Error::Conflict(
                "full frozen profile identity/order mismatch".into(),
            ));
        }
        identifier(&baseline.id)?;
        identifier(&baseline.profile_id)?;
        content_digest_format_v3(&baseline.environment_digest)?;
        baseline.baseline_skill.validate()?;
        baseline.baseline_strategy.validate()?;
        if fingerprint(&baseline.baseline_strategy)? != fingerprint(&Strategy::default())? {
            return Err(Error::Forbidden);
        }
        if !baseline
            .capabilities
            .allows(&baseline.baseline_skill.required_capabilities)
        {
            return Err(Error::Forbidden);
        }
        // Exact typed serialization of all 17 fields, not to_value/canonical.
        let baseline_digest = fingerprint(&baseline)?;
        if baseline_digest != profile.baseline_digest {
            return Err(Error::Conflict("baseline content digest mismatch".into()));
        }
        let parent_digest = match &parent {
            ParentContentV3::EmptyRoot(root) => {
                if root.schema_version != "rsia.empty_root_authorization.v1"
                    || root.namespace != baseline.namespace
                    || root.profile_registration_id != baseline.id
                    || root.profile_digest != baseline_digest
                    || root.host_registration_id != baseline.host_registration_id
                    || fingerprint(&root.skill)? != fingerprint(&SkillSnapshot::empty())?
                    || fingerprint(&root.strategy)?
                        != fingerprint(&empty_root_strategy_content_v3())?
                {
                    return Err(Error::Conflict(
                        "empty root complete content mismatch".into(),
                    ));
                }
                // Exact original nine fields, including namespace/authorized_by.
                fingerprint(root)?
            }
            ParentContentV3::CurrentBundle(bundle) => {
                validate_current_bundle_content_v3(bundle)?;
                if bundle.profile_id != profile.id {
                    return Err(Error::Conflict("parent bundle profile mismatch".into()));
                }
                // Its B may be a previous frozen profile: do not invent a new
                // parent.baseline_digest == current B restriction.
                bundle.digest.clone()
            }
        };
        if parent_digest != profile.parent_digest {
            return Err(Error::Conflict("parent content digest mismatch".into()));
        }
        Ok(Self {
            profile,
            parent,
            baseline,
        })
    }
    pub fn profile(&self) -> &Profile {
        &self.profile
    }
    pub fn baseline_content(&self) -> &FrozenCompilerProfileContentV1 {
        &self.baseline
    }
    fn parent_values(&self) -> (&SkillSnapshot, &Strategy) {
        match &self.parent {
            ParentContentV3::EmptyRoot(root) => (&root.skill, &root.strategy),
            ParentContentV3::CurrentBundle(bundle) => (&bundle.skill, &bundle.improver),
        }
    }
}
const BUNDLE_FIELDS_V3: [&str; 8] = [
    "skill.content",
    "skill.applicability",
    "skill.counterexample",
    "skill.required_capabilities",
    "skill.dependencies",
    "improver.instruction",
    "improver.max_candidates",
    "improver.max_rounds",
];
/// Existing typed fingerprint convention: digest field remains present but empty.
pub fn resolved_bundle_digest_v3(bundle: &ResolvedBundleV3) -> Result<String> {
    let mut unsigned = bundle.clone();
    unsigned.digest.clear();
    fingerprint(&unsigned)
}
fn origin_map_v3(bundle: &ResolvedBundleV3) -> Result<BTreeMap<&str, &ValueOrigin>> {
    let mut origins = BTreeMap::new();
    for origin in &bundle.origins {
        if origins.insert(origin.field.as_str(), origin).is_some() {
            return Err(Error::Conflict("duplicate bundle origin".into()));
        }
    }
    if origins.len() != BUNDLE_FIELDS_V3.len()
        || BUNDLE_FIELDS_V3
            .iter()
            .any(|field| !origins.contains_key(field))
    {
        return Err(Error::Invalid(
            "bundle requires exactly eight known origins".into(),
        ));
    }
    Ok(origins)
}
fn replay_leaf_v3<T: Clone + Serialize>(
    field: &str,
    origin: &ValueOrigin,
    value: &T,
    parent: &str,
    baseline: &str,
) -> Result<PatchOp<T>> {
    let value_json = serde_json::to_value(value).map_err(|_| Error::Internal)?;
    validate_explicit_field_value_v3(field, &value_json)?;
    if origin.field != field || origin.value_digest != fingerprint(value)? {
        return Err(Error::Conflict("origin value digest differs".into()));
    }
    match (origin.layer, origin.op.as_str()) {
        (OriginLayer::Parent, "inherit") if origin.source_digest == parent => Ok(PatchOp::Inherit),
        (OriginLayer::Baseline, "reset_to_baseline") if origin.source_digest == baseline => {
            Ok(PatchOp::ResetToBaseline)
        }
        (OriginLayer::Explicit, "set")
            if origin.source_digest == explicit_field_source_v3(field, value)? =>
        {
            Ok(PatchOp::Set {
                value: value.clone(),
            })
        }
        _ => Err(Error::Conflict(
            "origin operation/source/layer mismatch".into(),
        )),
    }
}
fn replay_patches_v3(bundle: &ResolvedBundleV3) -> Result<(SkillPatch, ImproverPatch)> {
    let o = origin_map_v3(bundle)?;
    let p = bundle.parent_digest.as_str();
    let b = bundle.baseline_digest.as_str();
    let skill = SkillPatch {
        content: replay_leaf_v3(
            "skill.content",
            o["skill.content"],
            &bundle.skill.content,
            p,
            b,
        )?,
        applicability: replay_leaf_v3(
            "skill.applicability",
            o["skill.applicability"],
            &bundle.skill.applicability,
            p,
            b,
        )?,
        counterexample: replay_leaf_v3(
            "skill.counterexample",
            o["skill.counterexample"],
            &bundle.skill.counterexample,
            p,
            b,
        )?,
        required_capabilities: replay_leaf_v3(
            "skill.required_capabilities",
            o["skill.required_capabilities"],
            &bundle.skill.required_capabilities,
            p,
            b,
        )?,
        dependencies: replay_leaf_v3(
            "skill.dependencies",
            o["skill.dependencies"],
            &bundle.skill.dependencies,
            p,
            b,
        )?,
    };
    let improver = ImproverPatch {
        instruction: replay_leaf_v3(
            "improver.instruction",
            o["improver.instruction"],
            &bundle.improver.instruction,
            p,
            b,
        )?,
        max_candidates: replay_leaf_v3(
            "improver.max_candidates",
            o["improver.max_candidates"],
            &bundle.improver.max_candidates,
            p,
            b,
        )?,
        max_rounds: replay_leaf_v3(
            "improver.max_rounds",
            o["improver.max_rounds"],
            &bundle.improver.max_rounds,
            p,
            b,
        )?,
    };
    Ok((skill, improver))
}
fn validate_current_bundle_content_v3(bundle: &ResolvedBundleV3) -> Result<()> {
    if bundle.schema_version != BUNDLE_SCHEMA_V3
        || bundle.compiler_version != BUNDLE_COMPILER_VERSION_V3
    {
        return Err(Error::Conflict(
            "current bundle requires v3 compiler/schema".into(),
        ));
    }
    identifier(&bundle.profile_id)?;
    content_digest_format_v3(&bundle.parent_digest)?;
    content_digest_format_v3(&bundle.baseline_digest)?;
    content_digest_format_v3(&bundle.digest)?;
    bundle.skill.validate()?;
    bundle.improver.validate()?;
    if resolved_bundle_digest_v3(bundle)? != bundle.digest {
        return Err(Error::Conflict("bundle content digest mismatch".into()));
    }
    replay_patches_v3(bundle)?;
    // This checks complete content, not prior P/B replay or release approval.
    Ok(())
}
fn compile_leaf_v3<T: Clone + Serialize>(
    field: &str,
    op: &PatchOp<T>,
    parent: &T,
    baseline: &T,
    proof: &ProfileContentProofV3,
    origins: &mut Vec<ValueOrigin>,
) -> Result<T> {
    let value = op.apply(parent, baseline);
    let value_json = serde_json::to_value(&value).map_err(|_| Error::Internal)?;
    validate_explicit_field_value_v3(field, &value_json)?;
    let (layer, source_digest) = match op {
        PatchOp::Inherit => (OriginLayer::Parent, proof.profile.parent_digest.clone()),
        PatchOp::ResetToBaseline => (OriginLayer::Baseline, proof.profile.baseline_digest.clone()),
        PatchOp::Set { .. } => (
            OriginLayer::Explicit,
            explicit_field_source_v3(field, &value)?,
        ),
    };
    origins.push(ValueOrigin {
        field: field.into(),
        layer,
        source_digest,
        op: op.name().into(),
        value_digest: fingerprint(&value)?,
    });
    Ok(value)
}
pub struct CompilePartsV3<'a> {
    pub content: &'a ProfileContentProofV3,
    pub skill_patch: &'a SkillPatch,
    pub improver_patch: &'a ImproverPatch,
    pub revoked: &'a BTreeSet<String>,
}
pub fn compile_bundle_v3(parts: CompilePartsV3<'_>) -> Result<ResolvedBundleV3> {
    let proof = parts.content;
    let (p, ps) = proof.parent_values();
    let b = &proof.baseline.baseline_skill;
    let bs = &proof.baseline.baseline_strategy;
    p.validate()?;
    ps.validate()?;
    b.validate()?;
    bs.validate()?;
    // Both original inputs are checked before any Set/Reset discards a reference.
    for dep in p.dependencies.iter().chain(&b.dependencies) {
        if parts.revoked.contains(dep) {
            return Err(Error::Conflict("dependency_revoked".into()));
        }
    }
    let mut origins = Vec::new();
    let skill = SkillSnapshot {
        content: compile_leaf_v3(
            "skill.content",
            &parts.skill_patch.content,
            &p.content,
            &b.content,
            proof,
            &mut origins,
        )?,
        applicability: compile_leaf_v3(
            "skill.applicability",
            &parts.skill_patch.applicability,
            &p.applicability,
            &b.applicability,
            proof,
            &mut origins,
        )?,
        counterexample: compile_leaf_v3(
            "skill.counterexample",
            &parts.skill_patch.counterexample,
            &p.counterexample,
            &b.counterexample,
            proof,
            &mut origins,
        )?,
        required_capabilities: compile_leaf_v3(
            "skill.required_capabilities",
            &parts.skill_patch.required_capabilities,
            &p.required_capabilities,
            &b.required_capabilities,
            proof,
            &mut origins,
        )?,
        dependencies: compile_leaf_v3(
            "skill.dependencies",
            &parts.skill_patch.dependencies,
            &p.dependencies,
            &b.dependencies,
            proof,
            &mut origins,
        )?,
    };
    let improver = Strategy {
        schema_version: ps.schema_version.clone(),
        instruction: compile_leaf_v3(
            "improver.instruction",
            &parts.improver_patch.instruction,
            &ps.instruction,
            &bs.instruction,
            proof,
            &mut origins,
        )?,
        max_candidates: compile_leaf_v3(
            "improver.max_candidates",
            &parts.improver_patch.max_candidates,
            &ps.max_candidates,
            &bs.max_candidates,
            proof,
            &mut origins,
        )?,
        max_rounds: compile_leaf_v3(
            "improver.max_rounds",
            &parts.improver_patch.max_rounds,
            &ps.max_rounds,
            &bs.max_rounds,
            proof,
            &mut origins,
        )?,
        require_counterexample: ps.require_counterexample,
    };
    skill.validate()?;
    improver.validate()?;
    for dep in &skill.dependencies {
        if parts.revoked.contains(dep) {
            return Err(Error::Conflict("dependency_revoked".into()));
        }
    }
    if !proof
        .baseline
        .capabilities
        .allows(&skill.required_capabilities)
    {
        return Err(Error::Forbidden);
    }
    let mut bundle = ResolvedBundleV3 {
        schema_version: BUNDLE_SCHEMA_V3.into(),
        compiler_version: BUNDLE_COMPILER_VERSION_V3.into(),
        profile_id: proof.profile.id.clone(),
        parent_digest: proof.profile.parent_digest.clone(),
        baseline_digest: proof.profile.baseline_digest.clone(),
        skill,
        improver,
        origins,
        digest: String::new(),
    };
    bundle.digest = resolved_bundle_digest_v3(&bundle)?;
    Ok(bundle)
}
pub fn replay_resolved_bundle_v3(
    bundle: &ResolvedBundleV3,
    proof: &ProfileContentProofV3,
    revoked: &BTreeSet<String>,
) -> Result<()> {
    validate_current_bundle_content_v3(bundle)?;
    if bundle.profile_id != proof.profile.id
        || bundle.parent_digest != proof.profile.parent_digest
        || bundle.baseline_digest != proof.profile.baseline_digest
    {
        return Err(Error::Conflict("bundle P/B identity mismatch".into()));
    }
    let (skill_patch, improver_patch) = replay_patches_v3(bundle)?;
    let rebuilt = compile_bundle_v3(CompilePartsV3 {
        content: proof,
        skill_patch: &skill_patch,
        improver_patch: &improver_patch,
        revoked,
    })?;
    if fingerprint(&rebuilt)? != fingerprint(bundle)? {
        return Err(Error::Conflict("complete bundle replay differs".into()));
    }
    Ok(())
}
#[cfg(test)]
mod complete_profile_bundle_v3_content_controls {
    use super::*;

    fn baseline() -> FrozenCompilerProfileContentV1 {
        let snapshot = SystemSnapshot {
            schema_version: "rsia.system_snapshot.v1".into(),
            profile_id: "profile".into(),
            host_id: "reference".into(),
            host_version: "0.1.0".into(),
            model_id: "development-model".into(),
            tools: vec!["reference-tool".into()],
            mandatory_context_digest: crate::hash(b"Complete mandatory-context fixture body."),
        };
        FrozenCompilerProfileContentV1 {
            id: "profile-registration".into(),
            schema_version: "rsia.frozen_compiler_profile.v1".into(),
            namespace: "source-content-test".into(),
            profile_id: "profile".into(),
            version: 1,
            host_registration_id: "host-registration".into(),
            provider_registration_id: "provider-registration".into(),
            resource_instrumentation_id: None,
            environment_digest: snapshot.digest().unwrap(),
            system_snapshot: snapshot,
            capabilities: HostCapabilities {
                available: ["cap-a".into()].into_iter().collect(),
                granted: ["cap-a".into()].into_iter().collect(),
            },
            baseline_skill: SkillSnapshot {
                content: "Complete actual baseline A.".into(),
                required_capabilities: vec!["cap-a".into()],
                ..SkillSnapshot::empty()
            },
            baseline_strategy: Strategy::default(),
            dependencies: vec![],
            purpose: crate::evidence::Purpose::Development,
            application_order: vec!["mandatory_context".into(), "skill".into(), "task".into()],
            authorized_by: "operator".into(),
        }
    }
    fn root(b: &FrozenCompilerProfileContentV1) -> EmptyRootContentV1 {
        EmptyRootContentV1 {
            id: "empty-root".into(),
            schema_version: "rsia.empty_root_authorization.v1".into(),
            namespace: b.namespace.clone(),
            profile_registration_id: b.id.clone(),
            profile_digest: fingerprint(b).unwrap(),
            host_registration_id: b.host_registration_id.clone(),
            skill: SkillSnapshot::empty(),
            strategy: empty_root_strategy_content_v3(),
            authorized_by: "operator".into(),
        }
    }
    fn profile(b: &FrozenCompilerProfileContentV1, parent_digest: String) -> Profile {
        Profile {
            id: b.profile_id.clone(),
            evolution_enabled: true,
            parent_digest,
            baseline_digest: fingerprint(b).unwrap(),
        }
    }
    fn root_proof(b: FrozenCompilerProfileContentV1) -> ProfileContentProofV3 {
        let r = root(&b);
        ProfileContentProofV3::from_content(
            profile(&b, fingerprint(&r).unwrap()),
            ParentContentV3::EmptyRoot(r),
            b,
        )
        .unwrap()
    }
    fn compiled(proof: &ProfileContentProofV3, body: &str) -> ResolvedBundleV3 {
        let patch = SkillPatch {
            content: PatchOp::Set { value: body.into() },
            required_capabilities: PatchOp::ResetToBaseline,
            ..SkillPatch::default()
        };
        let bundle = compile_bundle_v3(CompilePartsV3 {
            content: proof,
            skill_patch: &patch,
            improver_patch: &ImproverPatch::default(),
            revoked: &BTreeSet::new(),
        })
        .unwrap();
        replay_resolved_bundle_v3(&bundle, proof, &BTreeSet::new()).unwrap();
        bundle
    }
    fn exact_conflict<T: std::fmt::Debug>(result: Result<T>, expected: &str) {
        assert!(
            matches!(result, Err(Error::Conflict(ref message)) if message == expected),
            "expected Conflict({expected:?}), got {result:?}"
        );
    }
    #[test]
    fn complete_empty_root_and_real_compiled_current_parent_are_healthy_content() {
        let b = baseline();
        let first_proof = root_proof(b.clone());
        let first = compiled(&first_proof, "Actual first explicit body.");
        assert_eq!(first.origins.len(), 8);
        let current_proof = ProfileContentProofV3::from_content(
            profile(&b, first.digest.clone()),
            ParentContentV3::CurrentBundle(first.clone()),
            b,
        )
        .unwrap();
        let inherited = compile_bundle_v3(CompilePartsV3 {
            content: &current_proof,
            skill_patch: &SkillPatch::default(),
            improver_patch: &ImproverPatch::default(),
            revoked: &BTreeSet::new(),
        })
        .unwrap();
        replay_resolved_bundle_v3(&inherited, &current_proof, &BTreeSet::new()).unwrap();
        assert_eq!(inherited.skill.content, first.skill.content);
        assert_eq!(inherited.compiler_version, BUNDLE_COMPILER_VERSION_V3);
        assert_ne!(inherited.digest, first.digest);
    }
    #[test]
    fn another_valid_complete_parent_cannot_reuse_the_first_parent_digest() {
        let b = baseline();
        let proof = root_proof(b.clone());
        let a = compiled(&proof, "Actual current-parent content A.");
        let other = compiled(&proof, "Actual current-parent content B.");
        assert_ne!(a.digest, other.digest);
        exact_conflict(
            ProfileContentProofV3::from_content(
                profile(&b, a.digest),
                ParentContentV3::CurrentBundle(other),
                b,
            ),
            "parent content digest mismatch",
        );
    }
    #[test]
    fn another_valid_complete_baseline_cannot_reuse_the_first_baseline_digest() {
        let a = baseline();
        let old_digest = fingerprint(&a).unwrap();
        compiled(&root_proof(a), "Healthy original baseline input.");
        let mut other = baseline();
        other.baseline_skill.content = "Complete actual baseline B.".into();
        let r = root(&other);
        let healthy = root_proof(other.clone());
        compiled(&healthy, "Healthy changed baseline input.");
        let mut stale = profile(&other, fingerprint(&r).unwrap());
        assert_ne!(stale.baseline_digest, old_digest);
        stale.baseline_digest = old_digest;
        exact_conflict(
            ProfileContentProofV3::from_content(stale, ParentContentV3::EmptyRoot(r), other),
            "baseline content digest mismatch",
        );
    }
    #[test]
    fn another_valid_set_value_cannot_reuse_the_first_values_source() {
        let proof = root_proof(baseline());
        let a = compiled(&proof, "Actual explicit leaf A.");
        let mut other = compiled(&proof, "Actual explicit leaf B.");
        let source = a
            .origins
            .iter()
            .find(|o| o.field == "skill.content")
            .unwrap()
            .source_digest
            .clone();
        let origin = other
            .origins
            .iter_mut()
            .find(|o| o.field == "skill.content")
            .unwrap();
        assert_ne!(origin.source_digest, source);
        origin.source_digest = source;
        // Recompute the outer identity to isolate semantic source replay,
        // retaining B's actual value_digest rather than hitting a stale hash.
        other.digest = resolved_bundle_digest_v3(&other).unwrap();
        exact_conflict(
            replay_resolved_bundle_v3(&other, &proof, &BTreeSet::new()),
            "origin operation/source/layer mismatch",
        );
    }
    // These are content-data controls, not registered/approved/Host/Owner
    // producer controls and not Parent CPP. Actual resolver/carrier tests owe
    // those separately. No new wire may consume legacy v2 results.
}
