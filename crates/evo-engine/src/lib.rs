//! Controlled improvement and independent evaluation.
pub const DEFAULT_WORKER_CONCURRENCY: usize = 1;

pub mod compiler;
pub mod dispatch;
pub mod evaluator;
pub mod evidence;
pub mod executor;
pub mod releases;
