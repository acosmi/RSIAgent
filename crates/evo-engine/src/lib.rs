//! Controlled improvement and independent evaluation.
pub const DEFAULT_WORKER_CONCURRENCY: usize = 1;

pub mod broker;
pub mod capacity;
pub mod closed_loop;
pub mod compiler;
pub mod curriculum;
pub mod dispatch;
pub mod evaluator;
pub mod evidence;
pub mod executor;
pub mod exploration;
pub mod hosts;
pub mod import;
pub mod meta;
pub mod meta_experiment;
pub mod model;
pub mod monitoring;
pub mod optimization;
pub mod packages;
pub mod release_store;
pub mod releases;
pub mod replay;
pub mod replay_experiment;
pub mod seeds;
pub mod streaming_evaluator;
