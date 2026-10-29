//! Run-level projection. Does not write host global config.
use crate::formal_authority::{
    CompilerInputRequest, FormalCompilerInputBinding, VerifiedCompilerInputs,
    resolve_compiler_inputs,
};
use evo_core::contract::{
    CompileParts, ResolvedBundle, RunProjection, SystemSnapshot, compile_bundle, project_run,
};
use evo_core::skill_edit::{
    CompiledSkillEdit, ProtectedTextRange, SkillEditBatch, TrustedEditContext,
    compile_skill_edit_batch,
};
use evo_core::{Context, Error, Result, fingerprint};
use evo_storage::Session;
use serde::{Deserialize, Serialize};

pub fn compile(parts: CompileParts<'_>) -> Result<ResolvedBundle> {
    compile_bundle(parts)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedBundleRecord {
    pub id: String,
    pub schema_version: String,
    pub bundle: ResolvedBundle,
    pub input: CompilerInputRequest,
    pub binding: FormalCompilerInputBinding,
}

/// Resolves all original inputs before patch application, then resolves every
/// output read. The caller retains the transaction and owns its single commit.
pub async fn compile_in_session(
    s: &mut Session,
    ctx: &Context,
    input: &CompilerInputRequest,
    skill: &evo_core::contract::SkillPatch,
    improver: &evo_core::contract::ImproverPatch,
) -> Result<ResolvedBundle> {
    let verified = resolve_compiler_inputs(s, ctx, input).await?;
    let bundle = verified.compile(skill, improver)?;
    persist_authenticated_bundle(s, ctx, input, &bundle).await?;
    Ok(bundle)
}

pub async fn persist_authenticated_bundle(
    s: &mut Session,
    ctx: &Context,
    input: &CompilerInputRequest,
    bundle: &ResolvedBundle,
) -> Result<VerifiedCompilerInputs> {
    let verified = resolve_compiler_inputs(s, ctx, input).await?;
    verified.replay(bundle)?;
    // Persist one canonical output request, independent of whether the
    // original caller supplied the optional P-strategy assertion. Its value
    // comes from the verified frozen authority, after the original replay.
    let mut output = verified.request_for_search_parent(bundle.skill.clone());
    output.search_parent_strategy = bundle.improver.clone();
    let output_inputs = resolve_compiler_inputs(s, ctx, &output).await?;
    output_inputs.replay(bundle)?;
    let record = AuthenticatedBundleRecord {
        id: format!("formal-bundle-{}", bundle.digest),
        schema_version: "rsia.authenticated_bundle.v1".into(),
        bundle: bundle.clone(),
        input: output,
        binding: output_inputs.binding().clone(),
    };
    if let Some(old) = s
        .get::<AuthenticatedBundleRecord>(ctx, "artifact", &record.id)
        .await?
    {
        if fingerprint(&old)? != fingerprint(&record)? {
            return Err(Error::Conflict("compiled bundle identity reused".into()));
        }
    } else {
        s.put(ctx, "artifact", &record.id, ctx.actor(), &record)
            .await?;
        for node in &record.binding.closure.nodes {
            s.put_edge(ctx, "artifact", &record.id, &node.kind, &node.id)
                .await?;
        }
        s.audit(ctx, "compiler.bundle.persist", &record.id).await?;
    }
    Ok(output_inputs)
}

pub async fn verify_bundle_in_session(
    s: &mut Session,
    ctx: &Context,
    bundle_digest: &str,
    environment: &str,
    watermark: u64,
) -> Result<AuthenticatedBundleRecord> {
    let record: AuthenticatedBundleRecord = s
        .need(ctx, "artifact", &format!("formal-bundle-{bundle_digest}"))
        .await?;
    if record.schema_version != "rsia.authenticated_bundle.v1"
        || record.bundle.digest != bundle_digest
        || record.id != format!("formal-bundle-{bundle_digest}")
        || record.input.environment_digest != environment
        || record.input.revoke_watermark != watermark
    {
        return Err(Error::Conflict(
            "compiled bundle record identity mismatch".into(),
        ));
    }
    let verified = resolve_compiler_inputs(s, ctx, &record.input).await?;
    verified.replay(&record.bundle)?;
    if fingerprint(verified.binding())? != fingerprint(&record.binding)? {
        return Err(Error::Conflict(
            "compiled bundle restored with stale inputs".into(),
        ));
    }
    Ok(record)
}

/// Pure compilation only. Evaluation and runtime attachment remain separate gates.
pub fn compile_skill_edits(
    input: &evo_core::contract::SkillSnapshot,
    context: &TrustedEditContext,
    batch: &SkillEditBatch,
    protected_ranges: &[ProtectedTextRange],
) -> Result<CompiledSkillEdit> {
    compile_skill_edit_batch(input, context, batch, protected_ranges)
}

pub fn projection_for_new_run(
    bundle: &ResolvedBundle,
    snapshot: &SystemSnapshot,
    evolution_enabled: bool,
) -> Result<RunProjection> {
    project_run(bundle, snapshot, evolution_enabled)
}

#[derive(Debug, Clone)]
enum ReadBundleWire {
    Current(evo_core::contract::ResolvedBundleV3),
    Legacy(ResolvedBundle),
}
/// Immutable view only; it has no Serialize or authority constructor.
#[derive(Debug, Clone)]
pub struct ReadBundleView {
    pub schema_version: String,
    pub profile_id: String,
    pub parent_digest: String,
    pub baseline_digest: String,
    pub skill: evo_core::contract::SkillSnapshot,
    pub improver: evo_core::Strategy,
    pub origins: Vec<evo_core::contract::ValueOrigin>,
    pub digest: String,
}
#[derive(Debug, Clone)]
pub struct ReadBundle {
    wire: Box<ReadBundleWire>,
    view: Box<ReadBundleView>,
}
impl ReadBundle {
    fn from_wire(wire: ReadBundleWire) -> Result<Self> {
        let view = match &wire {
            ReadBundleWire::Current(record) => {
                if record.schema_version != evo_core::contract::BUNDLE_SCHEMA_V3
                    || record.compiler_version != evo_core::contract::BUNDLE_COMPILER_VERSION_V3
                {
                    return Err(Error::Invalid(
                        "unsupported current ReadBundle schema".into(),
                    ));
                }
                ReadBundleView {
                    schema_version: record.schema_version.clone(),
                    profile_id: record.profile_id.clone(),
                    parent_digest: record.parent_digest.clone(),
                    baseline_digest: record.baseline_digest.clone(),
                    skill: record.skill.clone(),
                    improver: record.improver.clone(),
                    origins: record.origins.clone(),
                    digest: record.digest.clone(),
                }
            }
            ReadBundleWire::Legacy(record) => {
                if record.schema_version != evo_core::contract::BUNDLE_SCHEMA {
                    return Err(Error::Invalid(
                        "unsupported legacy ReadBundle schema".into(),
                    ));
                }
                ReadBundleView {
                    schema_version: record.schema_version.clone(),
                    profile_id: record.profile_id.clone(),
                    parent_digest: record.parent_digest.clone(),
                    baseline_digest: record.baseline_digest.clone(),
                    skill: record.skill.clone(),
                    improver: record.improver.clone(),
                    origins: record.origins.clone(),
                    digest: record.digest.clone(),
                }
            }
        };
        let value = Self {
            wire: Box::new(wire),
            view: Box::new(view),
        };

        Ok(value)
    }
    pub(crate) fn from_current(record: evo_core::contract::ResolvedBundleV3) -> Result<Self> {
        Self::from_wire(ReadBundleWire::Current(record))
    }
    pub(crate) fn from_legacy(record: ResolvedBundle) -> Result<Self> {
        Self::from_wire(ReadBundleWire::Legacy(record))
    }
    pub(crate) fn is_current(&self) -> bool {
        matches!(self.wire.as_ref(), ReadBundleWire::Current(_))
    }
}
impl std::ops::Deref for ReadBundle {
    type Target = ReadBundleView;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}
impl Serialize for ReadBundle {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self.wire.as_ref() {
            ReadBundleWire::Current(record) => record.serialize(serializer),
            ReadBundleWire::Legacy(record) => record.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for ReadBundle {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = evo_core::contract::StrictJsonValue::deserialize(deserializer)?.0;
        let wire = match value
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
        {
            Some(evo_core::contract::BUNDLE_SCHEMA_V3) => {
                serde_json::from_value(value).map(ReadBundleWire::Current)
            }
            Some(evo_core::contract::BUNDLE_SCHEMA) => {
                serde_json::from_value(value).map(ReadBundleWire::Legacy)
            }
            _ => return Err(serde::de::Error::custom("unsupported bundle schema")),
        }
        .map_err(serde::de::Error::custom)?;
        Self::from_wire(wire).map_err(serde::de::Error::custom)
    }
}

impl ReadBundle {
    pub(crate) fn validate_identity(&self) -> Result<()> {
        match self.wire.as_ref() {
            ReadBundleWire::Legacy(bundle) => {
                crate::releases::validate_resolved_bundle_identity(bundle)
            }
            ReadBundleWire::Current(bundle) => {
                if evo_core::contract::resolved_bundle_digest_v3(bundle)? != bundle.digest {
                    return Err(Error::Conflict(
                        "Current resolved bundle identity differs".into(),
                    ));
                }
                evo_core::identifier(&bundle.profile_id)?;
                bundle.skill.validate()?;
                evo_core::Validate::validate(&bundle.improver)
            }
        }
    }
    pub(crate) fn current(&self) -> Result<&evo_core::contract::ResolvedBundleV3> {
        self.validate_identity()?;
        match self.wire.as_ref() {
            ReadBundleWire::Current(bundle) => Ok(bundle),
            ReadBundleWire::Legacy(_) => {
                Err(Error::Conflict("legacy bundle is not Current input".into()))
            }
        }
    }
}
pub(crate) fn projection_for_read_bundle(
    bundle: &ReadBundle,
    snapshot: &SystemSnapshot,
    evolution_enabled: bool,
) -> Result<RunProjection> {
    // Same application semantics as project_run; no version cast or DTO truncation.
    Ok(RunProjection {
        bundle_digest: bundle.digest.clone(),
        instructions: if !evolution_enabled
            || (bundle.skill.content.is_empty() && bundle.skill.applicability.is_empty())
        {
            Vec::new()
        } else {
            vec![bundle.skill.content.clone()]
        },
        tools: snapshot.tools.clone(),
    })
}

pub const AUTHENTICATED_BUNDLE_SCHEMA_V2: &str = "rsia.authenticated_bundle.v2";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentAuthenticatedBundleRecord {
    pub id: String,
    pub schema_version: String,
    pub bundle: evo_core::contract::ResolvedBundleV3,
    pub input: CompilerInputRequest,
    pub binding: crate::formal_authority::CurrentFormalCompilerInputBinding,
    pub original_baseline: crate::formal_authority::FrozenCompilerProfile,
    pub original_parent: crate::formal_authority::CurrentParentMaterial,
}
pub const AUTHENTICATED_BUNDLE_SCHEMA_V3: &str = "rsia.authenticated_bundle.v3";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedBundleRecordV3 {
    pub id: String,
    pub schema_version: String,
    pub bundle: evo_core::contract::ResolvedBundleV3,
    pub input: CompilerInputRequest,
    pub binding: crate::formal_authority::CurrentFormalCompilerInputBinding,
    pub original_baseline: crate::formal_authority::FrozenCompilerProfileV2,
    pub original_parent: crate::formal_authority::CurrentParentMaterial,
}
#[derive(Debug, Clone)]
pub struct AuthenticatedBundleView {
    pub id: String,
    pub schema_version: String,
    pub bundle: evo_core::contract::ResolvedBundleV3,
    pub input: CompilerInputRequest,
    pub binding: crate::formal_authority::CurrentFormalCompilerInputBinding,
    pub original_baseline: crate::formal_authority::LoadedFrozenCompilerProfile,
    pub original_parent: crate::formal_authority::CurrentParentMaterial,
}

#[derive(Debug, Clone)]
enum AuthenticatedBundleOriginal {
    V2(CurrentAuthenticatedBundleRecord),
    V3(AuthenticatedBundleRecordV3),
}
#[derive(Debug, Clone)]
pub struct LoadedAuthenticatedBundleRecord {
    original: AuthenticatedBundleOriginal,
    view: AuthenticatedBundleView,
}
impl LoadedAuthenticatedBundleRecord {
    fn from_original(original: AuthenticatedBundleOriginal) -> Self {
        macro_rules! view {
            ($value:expr) => {{
                let value = $value;
                AuthenticatedBundleView {
                    id: value.id.clone(),
                    schema_version: value.schema_version.clone(),
                    bundle: value.bundle.clone(),
                    input: value.input.clone(),
                    binding: value.binding.clone(),
                    original_baseline: value.original_baseline.clone().into(),
                    original_parent: value.original_parent.clone(),
                }
            }};
        }
        let view = match &original {
            AuthenticatedBundleOriginal::V2(v) => view!(v),
            AuthenticatedBundleOriginal::V3(v) => view!(v),
        };
        Self { original, view }
    }
    pub fn v2(&self) -> Option<&CurrentAuthenticatedBundleRecord> {
        match &self.original {
            AuthenticatedBundleOriginal::V2(v) => Some(v),
            _ => None,
        }
    }
    pub fn v3(&self) -> Option<&AuthenticatedBundleRecordV3> {
        match &self.original {
            AuthenticatedBundleOriginal::V3(v) => Some(v),
            _ => None,
        }
    }
    pub fn validate_content_version(&self) -> Result<()> {
        let expected = if self.v3().is_some() {
            AUTHENTICATED_BUNDLE_SCHEMA_V3
        } else {
            AUTHENTICATED_BUNDLE_SCHEMA_V2
        };
        if self.schema_version != expected
            || self.id != format!("formal-bundle-{}", self.bundle.digest)
            || (self.v3().is_some() != self.original_baseline.complete().is_some())
        {
            return Err(Error::Conflict(
                "authenticated original variant/version differs".into(),
            ));
        }
        if let Some(v) = self.v3() {
            let parent = match &v.original_parent {
                crate::formal_authority::CurrentParentMaterial::EmptyRoot { root } => {
                    evo_core::contract::ParentContentV3::EmptyRoot(
                        crate::formal_authority::project_empty_root(root)?,
                    )
                }
                crate::formal_authority::CurrentParentMaterial::ApprovedRelease {
                    bundle, ..
                } => evo_core::contract::ParentContentV3::CurrentBundle(bundle.clone()),
            };
            let baseline =
                crate::formal_authority::project_complete_frozen_profile(&v.original_baseline)?;
            let proof = evo_core::contract::CompleteProfileContentProofV3::from_content(
                v.input.profile.clone(),
                parent,
                baseline,
            )?;
            evo_core::contract::replay_complete_resolved_bundle_v3(
                &v.bundle,
                &proof,
                &std::collections::BTreeSet::new(),
            )?;
            if v.binding.schema_version != crate::formal_authority::CURRENT_BINDING_SCHEMA
                || v.input.profile.baseline_digest != v.bundle.baseline_digest
                || v.input.profile.parent_digest != v.bundle.parent_digest
                || v.input.environment_digest != v.original_baseline.environment_digest
                || fingerprint(&v.input.capabilities)?
                    != fingerprint(&v.original_baseline.capabilities)?
            {
                return Err(Error::Conflict(
                    "complete authenticated original input differs".into(),
                ));
            }
        }
        Ok(())
    }
}
impl From<CurrentAuthenticatedBundleRecord> for LoadedAuthenticatedBundleRecord {
    fn from(v: CurrentAuthenticatedBundleRecord) -> Self {
        Self::from_original(AuthenticatedBundleOriginal::V2(v))
    }
}
impl From<AuthenticatedBundleRecordV3> for LoadedAuthenticatedBundleRecord {
    fn from(v: AuthenticatedBundleRecordV3) -> Self {
        Self::from_original(AuthenticatedBundleOriginal::V3(v))
    }
}
impl std::ops::Deref for LoadedAuthenticatedBundleRecord {
    type Target = AuthenticatedBundleView;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}
impl Serialize for LoadedAuthenticatedBundleRecord {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match &self.original {
            AuthenticatedBundleOriginal::V2(v) => v.serialize(s),
            AuthenticatedBundleOriginal::V3(v) => v.serialize(s),
        }
    }
}
impl<'de> Deserialize<'de> for LoadedAuthenticatedBundleRecord {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = evo_core::contract::StrictJsonValue::deserialize(d)?.0;
        let record = match value
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
        {
            Some(AUTHENTICATED_BUNDLE_SCHEMA_V2) => {
                serde_json::from_value::<CurrentAuthenticatedBundleRecord>(value).map(Self::from)
            }
            Some(AUTHENTICATED_BUNDLE_SCHEMA_V3) => {
                serde_json::from_value::<AuthenticatedBundleRecordV3>(value).map(Self::from)
            }
            _ => {
                return Err(serde::de::Error::custom(
                    "unsupported authenticated bundle schema",
                ));
            }
        }
        .map_err(serde::de::Error::custom)?;
        record
            .validate_content_version()
            .map_err(serde::de::Error::custom)?;
        Ok(record)
    }
}
pub async fn compile_current_in_session(
    s: &mut Session,
    ctx: &Context,
    input: &CompilerInputRequest,
    skill: &evo_core::contract::SkillPatch,
    improver: &evo_core::contract::ImproverPatch,
) -> Result<evo_core::contract::ResolvedBundleV3> {
    let verified = crate::formal_authority::resolve_current_compiler_inputs(s, ctx, input).await?;
    let bundle = verified.compile(skill, improver)?;
    persist_current_authenticated_bundle(s, ctx, input, &bundle).await?;
    Ok(bundle)
}
pub async fn persist_current_authenticated_bundle(
    s: &mut Session,
    ctx: &Context,
    input: &CompilerInputRequest,
    bundle: &evo_core::contract::ResolvedBundleV3,
) -> Result<crate::formal_authority::CurrentVerifiedCompilerInputs> {
    prepare_or_persist_current_bundle(s, ctx, input, bundle, true).await
}
pub(crate) async fn preflight_current_authenticated_bundle(
    s: &mut Session,
    ctx: &Context,
    input: &CompilerInputRequest,
    bundle: &evo_core::contract::ResolvedBundleV3,
) -> Result<()> {
    prepare_or_persist_current_bundle(s, ctx, input, bundle, false)
        .await
        .map(|_| ())
}
async fn prepare_or_persist_current_bundle(
    s: &mut Session,
    ctx: &Context,
    input: &CompilerInputRequest,
    bundle: &evo_core::contract::ResolvedBundleV3,
    persist: bool,
) -> Result<crate::formal_authority::CurrentVerifiedCompilerInputs> {
    let verified = crate::formal_authority::resolve_current_compiler_inputs(s, ctx, input).await?;
    verified.replay(bundle)?;
    let mut output = verified.request_for_search_parent(bundle.skill.clone());
    output.search_parent_strategy = bundle.improver.clone();
    let output_inputs =
        crate::formal_authority::resolve_current_compiler_inputs(s, ctx, &output).await?;
    output_inputs.replay(bundle)?;
    let (original_baseline, original_parent) = output_inputs.original_material();
    let record = if original_baseline.complete().is_some() {
        LoadedAuthenticatedBundleRecord::from(AuthenticatedBundleRecordV3 {
            id: format!("formal-bundle-{}", bundle.digest),
            schema_version: AUTHENTICATED_BUNDLE_SCHEMA_V3.into(),
            bundle: bundle.clone(),
            input: output,
            binding: output_inputs.binding().clone(),
            original_baseline: original_baseline.complete().ok_or(Error::Internal)?.clone(),
            original_parent,
        })
    } else {
        LoadedAuthenticatedBundleRecord::from(CurrentAuthenticatedBundleRecord {
            id: format!("formal-bundle-{}", bundle.digest),
            schema_version: AUTHENTICATED_BUNDLE_SCHEMA_V2.into(),
            bundle: bundle.clone(),
            input: output,
            binding: output_inputs.binding().clone(),
            original_baseline: original_baseline.legacy().ok_or(Error::Internal)?.clone(),
            original_parent,
        })
    };
    record.validate_content_version()?;
    if let Some(old) = s
        .get::<LoadedAuthenticatedBundleRecord>(ctx, "artifact", &record.id)
        .await?
    {
        if serde_json::to_vec(&old).map_err(|_| Error::Internal)?
            != serde_json::to_vec(&record).map_err(|_| Error::Internal)?
        {
            return Err(Error::Conflict("compiled bundle identity reused".into()));
        }
    } else if persist {
        s.put(ctx, "artifact", &record.id, ctx.actor(), &record)
            .await?;
        for node in &record.binding.closure.nodes {
            s.put_edge(ctx, "artifact", &record.id, &node.kind, &node.id)
                .await?;
        }
        s.audit(ctx, "compiler.bundle.persist", &record.id).await?;
    }
    Ok(output_inputs)
}
pub async fn verify_current_bundle_in_session(
    s: &mut Session,
    ctx: &Context,
    bundle_digest: &str,
    environment: &str,
    watermark: u64,
) -> Result<LoadedAuthenticatedBundleRecord> {
    verify_current_bundle_in_session_inner(
        s,
        ctx,
        bundle_digest,
        environment,
        watermark,
        &mut std::collections::BTreeSet::new(),
    )
    .await
}
pub(crate) async fn verify_current_bundle_in_session_inner(
    s: &mut Session,
    ctx: &Context,
    bundle_digest: &str,
    environment: &str,
    watermark: u64,
    visiting: &mut std::collections::BTreeSet<String>,
) -> Result<LoadedAuthenticatedBundleRecord> {
    let record =
        need_current_authenticated_record(s, ctx, &format!("formal-bundle-{bundle_digest}"))
            .await?;
    if record.validate_content_version().is_err()
        || record.bundle.schema_version != evo_core::contract::BUNDLE_SCHEMA_V3
        || record.bundle.compiler_version != evo_core::contract::BUNDLE_COMPILER_VERSION_V3
        || record.binding.schema_version != crate::formal_authority::CURRENT_BINDING_SCHEMA
        || record.bundle.digest != bundle_digest
        || record.id != format!("formal-bundle-{bundle_digest}")
        || record.input.environment_digest != environment
        || record.input.revoke_watermark != watermark
    {
        return Err(Error::Conflict(
            "compiled bundle record identity mismatch".into(),
        ));
    }
    let verified = Box::pin(
        crate::formal_authority::resolve_current_compiler_inputs_inner(
            s,
            ctx,
            &record.input,
            visiting,
        ),
    )
    .await?;
    verified.replay(&record.bundle)?;
    if serde_json::to_vec(verified.binding()).map_err(|_| Error::Internal)?
        != serde_json::to_vec(&record.binding).map_err(|_| Error::Internal)?
    {
        return Err(Error::Conflict(
            "compiled bundle restored with stale inputs".into(),
        ));
    }
    Box::pin(crate::formal_authority::replay_current_record_material(
        s, ctx, &record, visiting,
    ))
    .await?;
    Ok(record)
}

pub(crate) async fn read_current_parent_bundle_in_session(
    s: &mut Session,
    ctx: &Context,
    digest: &str,
    environment: &str,
    watermark: u64,
    visiting: &mut std::collections::BTreeSet<String>,
) -> Result<LoadedAuthenticatedBundleRecord> {
    let key = format!("authenticated:{digest}");
    if !visiting.insert(key.clone()) {
        return Err(Error::Conflict("cyclic compiler parent lineage".into()));
    }
    let result = async {
        let record =
            need_current_authenticated_record(s, ctx, &format!("formal-bundle-{digest}")).await?;
        if record.validate_content_version().is_err()
            || record.bundle.schema_version != evo_core::contract::BUNDLE_SCHEMA_V3
            || record.bundle.compiler_version != evo_core::contract::BUNDLE_COMPILER_VERSION_V3
            || record.binding.schema_version != crate::formal_authority::CURRENT_BINDING_SCHEMA
            || record.id != format!("formal-bundle-{digest}")
            || record.bundle.digest != digest
            || record.input.environment_digest != environment
            || record.input.revoke_watermark != watermark
        {
            return Err(Error::Conflict(
                "compiled bundle record identity mismatch".into(),
            ));
        }
        Box::pin(crate::formal_authority::replay_current_record_material(
            s, ctx, &record, visiting,
        ))
        .await?;
        Ok(record)
    }
    .await;
    visiting.remove(&key);
    result
}

enum AuthenticatedRecordRead {
    Current(Box<LoadedAuthenticatedBundleRecord>),
    Legacy(Box<AuthenticatedBundleRecord>),
}
impl<'de> Deserialize<'de> for AuthenticatedRecordRead {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = evo_core::contract::StrictJsonValue::deserialize(d)?.0;
        match value
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
        {
            Some(AUTHENTICATED_BUNDLE_SCHEMA_V2 | AUTHENTICATED_BUNDLE_SCHEMA_V3) => {
                serde_json::from_value(value)
                    .map(Self::Current)
                    .map_err(serde::de::Error::custom)
            }
            Some("rsia.authenticated_bundle.v1") => serde_json::from_value(value)
                .map(Self::Legacy)
                .map_err(serde::de::Error::custom),
            _ => Err(serde::de::Error::custom(
                "unsupported authenticated bundle schema",
            )),
        }
    }
}
async fn need_current_authenticated_record(
    s: &mut Session,
    ctx: &Context,
    id: &str,
) -> Result<LoadedAuthenticatedBundleRecord> {
    match s
        .need::<AuthenticatedRecordRead>(ctx, "artifact", id)
        .await?
    {
        AuthenticatedRecordRead::Current(record) => {
            if record.schema_version != AUTHENTICATED_BUNDLE_SCHEMA_V2 || record.id != id {
                return Err(Error::Invalid(
                    "unsupported current authenticated record identity/schema".into(),
                ));
            }
            Ok(*record)
        }
        AuthenticatedRecordRead::Legacy(record) => {
            if record.schema_version != "rsia.authenticated_bundle.v1"
                || record.id != id
                || record.id != format!("formal-bundle-{}", record.bundle.digest)
            {
                return Err(Error::Invalid(
                    "invalid historical authenticated record identity/schema".into(),
                ));
            }
            crate::releases::validate_resolved_bundle_identity(&record.bundle)?;
            validate_historical_authenticated_input(&record, ctx)?;
            Err(Error::Conflict(
                "legacy authenticated bundle is not Current input".into(),
            ))
        }
    }
}

// Data-only classification before the legacy-only new-use refusal. This does
// not refresh historical authority, traverse live sources or dispatch a model.
fn validate_historical_authenticated_input(
    record: &AuthenticatedBundleRecord,
    ctx: &Context,
) -> Result<()> {
    use evo_core::Validate;
    let input = &record.input;
    let bundle = &record.bundle;
    let binding = &record.binding;
    let bad = || Error::Invalid("historical authenticated input identity mismatch".into());
    evo_core::identifier(&input.profile.id)?;
    evo_core::identifier(&input.profile.parent_digest)?;
    evo_core::identifier(&input.profile.baseline_digest)?;
    input.search_parent.validate()?;
    input.baseline.validate()?;
    input.search_parent_strategy.validate()?;
    input.baseline_strategy.validate()?;
    let sequence = match &binding.watermark {
        Some((sequence, id)) => {
            evo_core::identifier(id)?;
            u64::try_from(*sequence).map_err(|_| bad())?
        }
        None => 0,
    };
    // persist_authenticated_bundle canonicalizes the complete output request
    // with these exact fields, after the old resolve -> replay -> binding check.
    if bundle.schema_version != evo_core::contract::BUNDLE_SCHEMA
        || binding.schema_version != crate::formal_authority::BINDING_SCHEMA
        || binding.namespace != ctx.namespace()
        || !input.profile.evolution_enabled
        || input.profile.id != bundle.profile_id
        || input.profile.parent_digest != bundle.parent_digest
        || input.profile.baseline_digest != bundle.baseline_digest
        || binding.parent_digest != bundle.parent_digest
        || binding.baseline_digest != bundle.baseline_digest
        || binding.profile_digest != binding.baseline_digest
        || fingerprint(&input.search_parent)? != fingerprint(&bundle.skill)?
        || fingerprint(&input.search_parent_strategy)? != fingerprint(&bundle.improver)?
        || binding.search_parent_skill_digest != fingerprint(&input.search_parent)?
        || binding.search_parent_strategy_digest != fingerprint(&input.search_parent_strategy)?
        || binding.baseline_skill_digest != fingerprint(&input.baseline)?
        || binding.baseline_strategy_digest != fingerprint(&input.baseline_strategy)?
        || binding.capabilities_digest != fingerprint(&input.capabilities)?
        || binding.environment_digest != input.environment_digest
        || binding.purpose != input.purpose
        || binding.application_order != ["mandatory_context", "skill", "task"]
        || input.revoke_watermark != sequence
        || input.expected_parent_strategy_digest.as_ref() != Some(&binding.parent_strategy_digest)
        || !input
            .capabilities
            .allows(&bundle.skill.required_capabilities)
        || binding.closure.digest != fingerprint(&(&binding.closure.nodes, &binding.closure.edges))?
    {
        return Err(bad());
    }
    let mut seal = binding.clone();
    let expected = std::mem::take(&mut seal.binding_digest);
    if fingerprint(&seal)? != expected {
        return Err(bad());
    }
    evo_core::identifier(&binding.profile_registration_id)?;
    if binding.closure.nodes.len() > 10_000
        || binding.closure.edges.len() > 100_000
        || binding
            .closure
            .nodes
            .windows(2)
            .any(|pair| (&pair[0].kind, &pair[0].id) >= (&pair[1].kind, &pair[1].id))
        || binding
            .closure
            .edges
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(bad());
    }
    let mut nodes = std::collections::BTreeSet::new();
    for node in &binding.closure.nodes {
        evo_core::identifier(&node.kind)?;
        evo_core::identifier(&node.id)?;
        nodes.insert((node.kind.as_str(), node.id.as_str()));
    }
    if !binding.closure.nodes.iter().any(|node| {
        node.kind == "artifact"
            && node.id == binding.profile_registration_id
            && node.schema_version == crate::formal_authority::PROFILE_SCHEMA
    }) || !nodes.contains(&("formal_parent", input.profile.parent_digest.as_str()))
    {
        return Err(bad());
    }
    for edge in &binding.closure.edges {
        if !nodes.contains(&(edge.from_kind.as_str(), edge.from_id.as_str()))
            || !nodes.contains(&(edge.to_kind.as_str(), edge.to_id.as_str()))
        {
            return Err(bad());
        }
    }
    const ORIGINS: [&str; 8] = [
        "skill.content",
        "skill.applicability",
        "skill.counterexample",
        "skill.required_capabilities",
        "skill.dependencies",
        "improver.instruction",
        "improver.max_candidates",
        "improver.max_rounds",
    ];
    let names: std::collections::BTreeSet<_> = bundle
        .origins
        .iter()
        .map(|origin| origin.field.as_str())
        .collect();
    if bundle.origins.len() != ORIGINS.len()
        || names.len() != ORIGINS.len()
        || ORIGINS.iter().any(|name| !names.contains(name))
    {
        return Err(bad());
    }
    // Check every recoverable Reset leaf. Inherit needs the historical P body;
    // this classifier does not invent that body or treat missing lineage as damage.
    for origin in &bundle.origins {
        let actual_value_digest = match origin.field.as_str() {
            "skill.content" => fingerprint(&bundle.skill.content)?,
            "skill.applicability" => fingerprint(&bundle.skill.applicability)?,
            "skill.counterexample" => fingerprint(&bundle.skill.counterexample)?,
            "skill.required_capabilities" => fingerprint(&bundle.skill.required_capabilities)?,
            "skill.dependencies" => fingerprint(&bundle.skill.dependencies)?,
            "improver.instruction" => fingerprint(&bundle.improver.instruction)?,
            "improver.max_candidates" => fingerprint(&bundle.improver.max_candidates)?,
            "improver.max_rounds" => fingerprint(&bundle.improver.max_rounds)?,
            _ => return Err(bad()),
        };
        if origin.value_digest != actual_value_digest {
            return Err(bad());
        }
        let valid = match origin.field.as_str() {
            "skill.content" => bundle.skill.content == input.baseline.content,
            "skill.applicability" => bundle.skill.applicability == input.baseline.applicability,
            "skill.counterexample" => bundle.skill.counterexample == input.baseline.counterexample,
            "skill.required_capabilities" => {
                bundle.skill.required_capabilities == input.baseline.required_capabilities
            }
            "skill.dependencies" => bundle.skill.dependencies == input.baseline.dependencies,
            "improver.instruction" => {
                bundle.improver.instruction == input.baseline_strategy.instruction
            }
            "improver.max_candidates" => {
                bundle.improver.max_candidates == input.baseline_strategy.max_candidates
            }
            "improver.max_rounds" => {
                bundle.improver.max_rounds == input.baseline_strategy.max_rounds
            }
            _ => return Err(bad()),
        };
        match (
            &origin.layer,
            origin.op.as_str(),
            origin.source_digest.as_str(),
        ) {
            (evo_core::contract::OriginLayer::Parent, "inherit", source)
                if source == bundle.parent_digest => {}
            (evo_core::contract::OriginLayer::Baseline, "reset_to_baseline", source)
                if source == bundle.baseline_digest && valid => {}
            (evo_core::contract::OriginLayer::Explicit, "set", "set") => {}
            _ => return Err(bad()),
        }
    }
    // Old complete replay remains verify_bundle_in_session, unchanged. No call
    // into Current resolution, and therefore no new verifier mutual recursion.
    Ok(())
}

#[cfg(test)]
pub(crate) mod current_carrier_content_controls {
    use super::*;
    use evo_core::contract::{
        BUNDLE_COMPILER_VERSION_V3, BUNDLE_SCHEMA_V3, CompilePartsV3, EmptyRootContentV1,
        FrozenCompilerProfileContentV1, HostCapabilities, ImproverPatch, ParentContentV3, PatchOp,
        Profile, ProfileContentProofV3, ResolvedBundleV3, SkillPatch, SkillSnapshot,
        compile_bundle_v3,
    };
    use evo_core::{Strategy, hash};
    use std::collections::BTreeSet;

    // Complete software content only. No registration, provider, Owner, role,
    // source witness or successful engine resolver is manufactured by this twin.
    pub(crate) fn content_bundles() -> (ResolvedBundleV3, ResolvedBundle) {
        let snapshot = SystemSnapshot {
            schema_version: "rsia.system_snapshot.v1".into(),
            profile_id: "profile".into(),
            host_id: "reference".into(),
            host_version: "0.1.0".into(),
            model_id: "development".into(),
            tools: vec![],
            mandatory_context_digest: hash(b"Complete carrier-control context."),
        };
        let baseline = FrozenCompilerProfileContentV1 {
            id: "profile-registration".into(),
            schema_version: "rsia.frozen_compiler_profile.v1".into(),
            namespace: "carrier-content".into(),
            profile_id: "profile".into(),
            version: 1,
            host_registration_id: "host-registration".into(),
            provider_registration_id: "provider-registration".into(),
            resource_instrumentation_id: None,
            environment_digest: snapshot.digest().unwrap(),
            system_snapshot: snapshot,
            capabilities: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            baseline_skill: SkillSnapshot::empty(),
            baseline_strategy: Strategy::default(),
            dependencies: vec![],
            purpose: evo_core::evidence::Purpose::Development,
            application_order: vec!["mandatory_context".into(), "skill".into(), "task".into()],
            authorized_by: "operator".into(),
        };
        let root = EmptyRootContentV1 {
            id: "empty-root".into(),
            schema_version: "rsia.empty_root_authorization.v1".into(),
            namespace: baseline.namespace.clone(),
            profile_registration_id: baseline.id.clone(),
            profile_digest: fingerprint(&baseline).unwrap(),
            host_registration_id: baseline.host_registration_id.clone(),
            skill: SkillSnapshot::empty(),
            strategy: Strategy {
                schema_version: "evo.strategy.v1".into(),
                instruction:
                    "Preserve the unchanged host baseline; no evolution instruction is applied."
                        .into(),
                max_candidates: 1,
                max_rounds: 1,
                require_counterexample: true,
            },
            authorized_by: "operator".into(),
        };
        let profile = Profile {
            id: baseline.profile_id.clone(),
            evolution_enabled: true,
            parent_digest: fingerprint(&root).unwrap(),
            baseline_digest: fingerprint(&baseline).unwrap(),
        };
        let proof = ProfileContentProofV3::from_content(
            profile.clone(),
            ParentContentV3::EmptyRoot(root.clone()),
            baseline.clone(),
        )
        .unwrap();
        let skill = SkillPatch {
            content: PatchOp::Set {
                value: "Complete carrier body.".into(),
            },
            ..SkillPatch::default()
        };
        let improver = ImproverPatch::default();
        let revoked = BTreeSet::new();
        let current = compile_bundle_v3(CompilePartsV3 {
            content: &proof,
            skill_patch: &skill,
            improver_patch: &improver,
            revoked: &revoked,
        })
        .unwrap();
        let legacy = compile_bundle(CompileParts {
            profile: &profile,
            parent: &root.skill,
            baseline: &baseline.baseline_skill,
            parent_strategy: &root.strategy,
            baseline_strategy: &baseline.baseline_strategy,
            skill_patch: &skill,
            improver_patch: &improver,
            caps: &baseline.capabilities,
            revoked: &revoked,
        })
        .unwrap();
        (current, legacy)
    }

    #[test]
    fn current_read_carrier_keeps_the_complete_compiler_wire() {
        let (current, _) = content_bundles();
        let bytes = serde_json::to_vec(&current).unwrap();
        let read: ReadBundle = serde_json::from_slice(&bytes).unwrap();
        read.validate_identity().unwrap();
        assert!(read.is_current());
        assert_eq!(read.schema_version, BUNDLE_SCHEMA_V3);
        assert_eq!(
            read.current().unwrap().compiler_version,
            BUNDLE_COMPILER_VERSION_V3
        );
        assert_eq!(read.current().unwrap().origins.len(), 8);
        assert_eq!(serde_json::to_vec(&read).unwrap(), bytes);
        assert_eq!(serde_json::to_vec(read.current().unwrap()).unwrap(), bytes);
    }

    #[test]
    fn legacy_read_carrier_keeps_old_bytes_and_cannot_be_a_current_bundle() {
        let (_, legacy) = content_bundles();
        let bytes = serde_json::to_vec(&legacy).unwrap();
        let read: ReadBundle = serde_json::from_slice(&bytes).unwrap();
        read.validate_identity().unwrap();
        assert!(!read.is_current());
        assert_eq!(serde_json::to_vec(&read).unwrap(), bytes);
        assert!(matches!(read.current(), Err(Error::Conflict(ref message))
            if message == "legacy bundle is not Current input"));
    }

    #[test]
    fn read_carrier_refuses_version_mismatch_unknown_version_and_extra_fields() {
        let (current, legacy) = content_bundles();
        let healthy = serde_json::to_value(&current).unwrap();
        let _: ReadBundle = serde_json::from_value(healthy.clone()).unwrap();
        let mut wrong_compiler = healthy.clone();
        wrong_compiler["compiler_version"] = serde_json::json!("unrecognized-compiler");
        assert!(serde_json::from_value::<ReadBundle>(wrong_compiler).is_err());
        let mut wrong_schema = healthy.clone();
        wrong_schema["schema_version"] = serde_json::json!(evo_core::contract::BUNDLE_SCHEMA);
        assert!(serde_json::from_value::<ReadBundle>(wrong_schema).is_err());
        let mut unknown = healthy;
        unknown["schema_version"] = serde_json::json!("rsia.resolved_bundle.v99");
        assert!(serde_json::from_value::<ReadBundle>(unknown).is_err());
        for mut body in [
            serde_json::to_value(current).unwrap(),
            serde_json::to_value(legacy).unwrap(),
        ] {
            body["unrecognized"] = serde_json::json!(null);
            assert!(serde_json::from_value::<ReadBundle>(body).is_err());
        }
    }
}

pub(crate) fn projection_for_loaded_snapshot(
    bundle: &ReadBundle,
    snapshot: &evo_core::contract::LoadedSystemSnapshot,
    evolution_enabled: bool,
) -> Result<RunProjection> {
    if let Some(old) = snapshot.legacy() {
        return projection_for_read_bundle(bundle, old, evolution_enabled);
    }
    projection_for_complete_snapshot(
        bundle,
        snapshot.complete().ok_or(Error::Internal)?,
        evolution_enabled,
    )
}
pub fn projection_for_complete_snapshot(
    bundle: &ReadBundle,
    snapshot: &evo_core::contract::SystemSnapshotV3,
    evolution_enabled: bool,
) -> Result<RunProjection> {
    snapshot.validate()?;
    Ok(RunProjection {
        bundle_digest: bundle.digest.clone(),
        instructions: if !evolution_enabled
            || (bundle.skill.content.is_empty() && bundle.skill.applicability.is_empty())
        {
            Vec::new()
        } else {
            vec![bundle.skill.content.clone()]
        },
        tools: snapshot.tools.clone(),
    })
}
