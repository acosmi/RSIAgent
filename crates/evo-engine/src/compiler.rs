//! Run-level projection. Does not write host global config.
use evo_core::Result;
use evo_core::contract::{
    CompileParts, ResolvedBundle, RunProjection, SystemSnapshot, compile_bundle, project_run,
};

pub fn compile(parts: CompileParts<'_>) -> Result<ResolvedBundle> {
    compile_bundle(parts)
}

pub fn projection_for_new_run(
    bundle: &ResolvedBundle,
    snapshot: &SystemSnapshot,
    evolution_enabled: bool,
) -> Result<RunProjection> {
    project_run(bundle, snapshot, evolution_enabled)
}
