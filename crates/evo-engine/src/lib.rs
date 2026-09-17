//! Controlled improvement and independent evaluation.
pub const DEFAULT_WORKER_CONCURRENCY: usize = 1;

pub mod closed_loop;
pub mod compiler;
pub mod curriculum;
pub mod dispatch;
pub mod evaluator;
pub mod evidence;
pub mod executor;
pub mod exploration;
pub mod import;
pub mod meta;
pub mod meta_experiment;
pub mod monitoring;
pub mod releases;
pub mod replay;
pub mod replay_experiment;
