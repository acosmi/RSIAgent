//! Run-level projection. Does not write host global config.
use evo_core::Result;
use evo_core::contract::{
    CompileParts, ResolvedBundle, RunProjection, SystemSnapshot, compile_bundle, project_run,
};
use evo_core::skill_edit::{
    CompiledSkillEdit, ProtectedTextRange, SkillEditBatch, TrustedEditContext,
    compile_skill_edit_batch,
};

pub fn compile(parts: CompileParts<'_>) -> Result<ResolvedBundle> {
    compile_bundle(parts)
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
