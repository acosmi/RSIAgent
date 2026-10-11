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
