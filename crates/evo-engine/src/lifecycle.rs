//! Thin lifecycle orchestration over the SQLite tombstone/watermark/cleanup authority.

use evo_core::{Context, Result};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupStatus, LifecycleStore, TypedObjectRef};

pub struct LifecycleCoordinator;

impl LifecycleCoordinator {
    pub async fn revoke_source(
        ctx: &Context,
        store: &Store,
        source_id: impl Into<String>,
        reason: &str,
        now: i64,
    ) -> Result<CleanupStatus> {
        LifecycleStore::begin_revoke(
            ctx,
            store,
            TypedObjectRef {
                kind: "run".into(),
                id: source_id.into(),
            },
            reason,
            now,
        )
        .await
    }

    pub async fn continue_cleanup(
        ctx: &Context,
        store: &Store,
        job_id: &str,
        edge_page_limit: usize,
        now: i64,
    ) -> Result<CleanupStatus> {
        LifecycleStore::cleanup_step(ctx, store, job_id, edge_page_limit, now).await
    }

    pub async fn cleanup_status(
        ctx: &Context,
        store: &Store,
        job_id: &str,
    ) -> Result<CleanupStatus> {
        LifecycleStore::cleanup_status(ctx, store, job_id).await
    }
}
