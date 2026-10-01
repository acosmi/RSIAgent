//! Persistent immutable replay consumer over the existing replay_worlds table.

use crate::{Session, Store};
use evo_core::evidence::Purpose;
use evo_core::evidence::{ExecutionAttestation, TaskOrigin};
use evo_core::optimization::OptimizationTrace;
use evo_core::replay::{
    REPLAY_POOL_SCHEMA, REPLAY_WORLD_SCHEMA, ReplayPoolManifestV1, ReplayWorldV2,
    STORED_REPLAY_REPORT_SCHEMA, StoredReplayReportV1,
};
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const REPLAY_STORE_ENVELOPE_SCHEMA: &str = "rsia.replay.store_envelope.v1";
pub const REPLAY_POOL_RECORD_KIND: &str = "replay_pool_manifest_v1";
pub const REPLAY_REPORT_RECORD_KIND: &str = "stored_replay_report_v1";
/// Record kind a replay world is named by. Worlds are not envelopes: the store
/// keeps them in their own table, and `replay_world` is the node kind their
/// dependency edges and cleanup events use.
const REPLAY_WORLD_RECORD_KIND: &str = "replay_world";
/// Body schema the revocation cleanup leaves in place of redacted content.
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayStoreEnvelope<T> {
    schema_version: String,
    id: String,
    namespace: String,
    record_kind: String,
    payload: T,
}

#[derive(Debug, Clone)]
pub struct LoadedReplayPool {
    pub manifest: ReplayPoolManifestV1,
    pub worlds: Vec<ReplayWorldV2>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTraceAuthorityWire {
    schema_version: String,
    record: StoredRunRecordWire,
    trace: OptimizationTrace,
    excerpt_start: usize,
    excerpt_end: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRunRecordWire {
    id: String,
    body: Vec<u8>,
    parent_family: String,
    task_origin: TaskOrigin,
    execution_attestation: ExecutionAttestation,
    purpose: Purpose,
}

pub async fn put_replay_world_draft(
    ctx: &Context,
    store: &Store,
    world: &ReplayWorldV2,
) -> Result<()> {
    ctx.require(&[Role::Worker, Role::Admin])?;
    if world.sealed_digest.is_some() {
        return Err(Error::Invalid("draft world already has a seal".into()));
    }
    let mut check = world.clone();
    check.seal()?;
    let mut session = store.session().await?;
    validate_sources_live(&mut session, ctx, &check).await?;
    session
        .put_world(
            ctx,
            &world.manifest.world_id,
            false,
            &serde_json::to_value(world).map_err(|_| Error::Internal)?,
        )
        .await?;
    session.commit().await
}

pub async fn seal_replay_world(ctx: &Context, store: &Store, world: &ReplayWorldV2) -> Result<()> {
    ctx.require(&[Role::Worker, Role::Admin])?;
    world.validate_sealed()?;
    let mut session = store.session().await?;
    validate_sources_live(&mut session, ctx, world).await?;
    if let Some((sealed, manifest)) = session.get_world(ctx, &world.manifest.world_id).await? {
        if sealed {
            let existing: ReplayWorldV2 = serde_json::from_value(manifest)
                .map_err(|_| Error::Conflict("sealed legacy world is not ReplayWorldV2".into()))?;
            existing.validate_sealed()?;
            if fingerprint(&existing)? != fingerprint(world)? {
                return Err(Error::Conflict("sealed replay world differs".into()));
            }
            session.commit().await?;
            return Ok(());
        }
        let mut draft: ReplayWorldV2 = serde_json::from_value(manifest)
            .map_err(|_| Error::Conflict("draft world schema differs".into()))?;
        if draft.schema_version != REPLAY_WORLD_SCHEMA || draft.sealed_digest.is_some() {
            return Err(Error::Conflict("stored draft is not ReplayWorldV2".into()));
        }
        draft.seal()?;
        if fingerprint(&draft)? != fingerprint(world)? {
            return Err(Error::Conflict("seal does not match stored draft".into()));
        }
    }
    let value = serde_json::to_value(world).map_err(|_| Error::Internal)?;
    session
        .put_world(ctx, &world.manifest.world_id, true, &value)
        .await?;
    for source in &world.manifest.source_closure {
        session
            .put_edge(
                ctx,
                "replay_world",
                &world.manifest.world_id,
                "run",
                &source.source_id,
            )
            .await?;
    }
    session
        .audit(ctx, "replay.world.seal", &world.manifest.world_id)
        .await?;
    session.commit().await
}

pub async fn load_live_replay_world(
    ctx: &Context,
    store: &Store,
    world_id: &str,
    purpose: Purpose,
) -> Result<ReplayWorldV2> {
    ctx.require(&[Role::Worker, Role::Admin])?;
    identifier(world_id)?;
    let mut session = store.session().await?;
    let world = load_live_replay_world_in_session(&mut session, ctx, world_id, purpose).await?;
    session.commit().await?;
    Ok(world)
}

async fn load_live_replay_world_in_session(
    session: &mut Session,
    ctx: &Context,
    world_id: &str,
    purpose: Purpose,
) -> Result<ReplayWorldV2> {
    identifier(world_id)?;
    let (sealed, value) = session
        .get_world(ctx, world_id)
        .await?
        .ok_or(Error::NotFound)?;
    // Before the seal check and the decode: a redacted world is the expected
    // state after a source revocation, whether or not it was ever sealed.
    if is_redacted(&value) {
        return Err(redacted_record(REPLAY_WORLD_RECORD_KIND, world_id));
    }
    if !sealed {
        return Err(Error::Conflict("replay world is not sealed".into()));
    }
    let world: ReplayWorldV2 = serde_json::from_value(value)
        .map_err(|_| Error::Invalid("sealed world is not ReplayWorldV2".into()))?;
    world.validate_sealed()?;
    if world.manifest.purpose != purpose {
        return Err(Error::Forbidden);
    }
    validate_sources_live(session, ctx, &world).await?;
    Ok(world)
}

pub async fn register_replay_pool(
    ctx: &Context,
    store: &Store,
    world_ids: &[String],
) -> Result<ReplayPoolManifestV1> {
    ctx.require(&[Role::Worker, Role::Admin])?;
    let mut ids = world_ids.to_vec();
    for id in &ids {
        identifier(id)?;
    }
    ids.sort();
    if ids.is_empty() || ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Error::Invalid(
            "replay pool world ids are empty or duplicated".into(),
        ));
    }
    let mut session = store.session().await?;
    let mut worlds = Vec::with_capacity(ids.len());
    for id in &ids {
        worlds.push(
            load_live_replay_world_in_session(&mut session, ctx, id, Purpose::Development).await?,
        );
    }
    let manifest = ReplayPoolManifestV1::build(&worlds)?;
    let id = replay_pool_storage_id(&manifest.pool_digest)?;
    let envelope = ReplayStoreEnvelope {
        schema_version: REPLAY_STORE_ENVELOPE_SCHEMA.into(),
        id: id.clone(),
        namespace: ctx.namespace().into(),
        record_kind: REPLAY_POOL_RECORD_KIND.into(),
        payload: manifest.clone(),
    };
    if let Some(existing) =
        read_envelope::<ReplayPoolManifestV1>(&mut session, ctx, REPLAY_POOL_RECORD_KIND, &id)
            .await?
    {
        validate_envelope(
            ctx,
            &id,
            REPLAY_POOL_RECORD_KIND,
            &existing,
            REPLAY_POOL_SCHEMA,
        )?;
        if fingerprint(&existing)? != fingerprint(&envelope)? {
            return Err(Error::Conflict(
                "replay pool id reused with different content".into(),
            ));
        }
        session.commit().await?;
        return Ok(existing.payload);
    }
    session
        .put(ctx, "artifact", &id, ctx.actor(), &envelope)
        .await?;
    for world in &worlds {
        session
            .put_edge(
                ctx,
                "artifact",
                &id,
                "replay_world",
                &world.manifest.world_id,
            )
            .await?;
    }
    session.audit(ctx, "replay.pool.register", &id).await?;
    session.commit().await?;
    Ok(manifest)
}

pub async fn load_live_replay_pool(
    ctx: &Context,
    store: &Store,
    pool_digest: &str,
) -> Result<LoadedReplayPool> {
    ctx.require(&[Role::Worker, Role::Admin, Role::Evaluator])?;
    let mut session = store.session().await?;
    let loaded = load_live_replay_pool_in_session(&mut session, ctx, pool_digest).await?;
    session.commit().await?;
    Ok(loaded)
}

async fn load_live_replay_pool_in_session(
    session: &mut Session,
    ctx: &Context,
    pool_digest: &str,
) -> Result<LoadedReplayPool> {
    let id = replay_pool_storage_id(pool_digest)?;
    let envelope: ReplayStoreEnvelope<ReplayPoolManifestV1> =
        read_envelope(session, ctx, REPLAY_POOL_RECORD_KIND, &id)
            .await?
            .ok_or(Error::NotFound)?;
    validate_envelope(
        ctx,
        &id,
        REPLAY_POOL_RECORD_KIND,
        &envelope,
        REPLAY_POOL_SCHEMA,
    )?;
    if envelope.payload.pool_digest != pool_digest {
        return Err(Error::Conflict("replay pool row digest mismatch".into()));
    }
    let mut worlds = Vec::with_capacity(envelope.payload.members.len());
    for member in &envelope.payload.members {
        worlds.push(
            load_live_replay_world_in_session(session, ctx, &member.world_id, Purpose::Development)
                .await?,
        );
    }
    envelope.payload.validate_against(&worlds)?;
    Ok(LoadedReplayPool {
        manifest: envelope.payload,
        worlds,
    })
}

pub async fn put_replay_report(
    ctx: &Context,
    store: &Store,
    report: &StoredReplayReportV1,
) -> Result<()> {
    ctx.require(&[Role::Worker, Role::Admin])?;
    if report.namespace != ctx.namespace() {
        return Err(Error::Forbidden);
    }
    let mut session = store.session().await?;
    let pool = load_live_replay_pool_in_session(&mut session, ctx, &report.pool_digest).await?;
    report.validate_static(&pool.manifest)?;
    let envelope = ReplayStoreEnvelope {
        schema_version: REPLAY_STORE_ENVELOPE_SCHEMA.into(),
        id: report.report_id.clone(),
        namespace: ctx.namespace().into(),
        record_kind: REPLAY_REPORT_RECORD_KIND.into(),
        payload: report.clone(),
    };
    if let Some(existing) = read_envelope::<StoredReplayReportV1>(
        &mut session,
        ctx,
        REPLAY_REPORT_RECORD_KIND,
        &report.report_id,
    )
    .await?
    {
        validate_envelope(
            ctx,
            &report.report_id,
            REPLAY_REPORT_RECORD_KIND,
            &existing,
            STORED_REPLAY_REPORT_SCHEMA,
        )?;
        if fingerprint(&existing)? != fingerprint(&envelope)? {
            return Err(Error::Conflict("immutable replay report differs".into()));
        }
        session.commit().await?;
        return Ok(());
    }
    session
        .put(ctx, "artifact", &report.report_id, ctx.actor(), &envelope)
        .await?;
    session
        .put_edge(
            ctx,
            "artifact",
            &report.report_id,
            "artifact",
            &replay_pool_storage_id(&report.pool_digest)?,
        )
        .await?;
    for member in &report.member_reports {
        session
            .put_edge(
                ctx,
                "artifact",
                &report.report_id,
                "replay_world",
                &member.world_id,
            )
            .await?;
    }
    session
        .audit(ctx, "replay.report.store", &report.report_id)
        .await?;
    session.commit().await
}

pub async fn load_live_replay_report(
    ctx: &Context,
    store: &Store,
    report_id: &str,
) -> Result<StoredReplayReportV1> {
    ctx.require(&[Role::Worker, Role::Admin, Role::Evaluator])?;
    identifier(report_id)?;
    let mut session = store.session().await?;
    let envelope: ReplayStoreEnvelope<StoredReplayReportV1> =
        read_envelope(&mut session, ctx, REPLAY_REPORT_RECORD_KIND, report_id)
            .await?
            .ok_or(Error::NotFound)?;
    validate_envelope(
        ctx,
        report_id,
        REPLAY_REPORT_RECORD_KIND,
        &envelope,
        STORED_REPLAY_REPORT_SCHEMA,
    )?;
    let pool =
        load_live_replay_pool_in_session(&mut session, ctx, &envelope.payload.pool_digest).await?;
    envelope.payload.validate_static(&pool.manifest)?;
    session.commit().await?;
    Ok(envelope.payload)
}

pub fn replay_pool_storage_id(pool_digest: &str) -> Result<String> {
    validate_digest(pool_digest)?;
    Ok(format!("replay-pool-{pool_digest}"))
}

fn validate_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(
            "replay content digest must be a lowercase sha256".into(),
        ));
    }
    Ok(())
}

fn is_redacted(body: &serde_json::Value) -> bool {
    body.get("schema_version")
        .and_then(serde_json::Value::as_str)
        == Some(REDACTED_SCHEMA)
}

/// The `Conflict` a read of a redacted replay record reports.
fn redacted_record(record_kind: &str, id: &str) -> Error {
    Error::Conflict(format!(
        "replay {record_kind} {id} was redacted because its source was revoked"
    ))
}

/// Reads the stored body of one replay pool or report as its envelope.
///
/// After a source revocation the cleanup replaces the body with an
/// `rsia.redacted.v1` tombstone, which is not an envelope. That is the expected
/// state of a revoked replay record, not corruption, so the read names it: a
/// `Conflict` carrying the record kind and id, the verdict the exploration and
/// curriculum stores give the same state (`read_envelope` in `exploration.rs`,
/// `get_record` in `curriculum.rs`), instead of a decode failure that would
/// surface as `Internal`. The body is inspected before it is decoded, so the
/// expected state does not raise the storage layer's "database operation failed"
/// error log either. Any other body that does not decode is corruption and stays
/// `Internal`.
async fn read_envelope<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
) -> Result<Option<ReplayStoreEnvelope<T>>> {
    let Some(body) = session
        .get::<serde_json::Value>(ctx, "artifact", id)
        .await?
    else {
        return Ok(None);
    };
    if is_redacted(&body) {
        return Err(redacted_record(record_kind, id));
    }
    match serde_json::from_value(body) {
        Ok(envelope) => Ok(Some(envelope)),
        Err(error) => {
            tracing::error!(%error, record_kind, id, "stored replay record does not decode");
            Err(Error::Internal)
        }
    }
}

fn validate_envelope<T: Serialize + DeserializeOwned>(
    ctx: &Context,
    row_id: &str,
    expected_kind: &str,
    envelope: &ReplayStoreEnvelope<T>,
    expected_payload_schema: &str,
) -> Result<()> {
    if envelope.schema_version != REPLAY_STORE_ENVELOPE_SCHEMA
        || envelope.id != row_id
        || envelope.namespace != ctx.namespace()
        || envelope.record_kind != expected_kind
    {
        return Err(Error::Conflict(
            "replay store envelope identity mismatch".into(),
        ));
    }
    let value = serde_json::to_value(&envelope.payload).map_err(|_| Error::Internal)?;
    if value.get("schema_version").and_then(|value| value.as_str()) != Some(expected_payload_schema)
    {
        return Err(Error::Invalid(
            "replay envelope payload schema mismatch".into(),
        ));
    }
    Ok(())
}

async fn validate_sources_live(
    session: &mut Session,
    ctx: &Context,
    world: &ReplayWorldV2,
) -> Result<()> {
    let watermark = session
        .watermark(ctx)
        .await?
        .and_then(|(sequence, _)| u64::try_from(sequence).ok())
        .ok_or_else(|| Error::Conflict("missing source revoke watermark".into()))?;
    if watermark != world.manifest.revoke_watermark {
        return Err(Error::Conflict("source revoke watermark changed".into()));
    }
    for source in &world.manifest.source_closure {
        if session
            .get::<serde_json::Value>(ctx, "tombstone", &source.source_id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
        let authority: StoredTraceAuthorityWire = session
            .get::<serde_json::Value>(ctx, "run", &source.source_id)
            .await?
            .ok_or(Error::NotFound)
            .and_then(|value| {
                serde_json::from_value(value)
                    .map_err(|_| Error::Invalid("invalid stored trace authority".into()))
            })?;
        let valid = authority.schema_version == "rsia.optimization.source.v1"
            && authority.record.id == source.source_id
            && authority.record.task_origin == TaskOrigin::TrustedRun
            && authority.record.execution_attestation == ExecutionAttestation::TrustedHost
            && authority.record.purpose == Purpose::Development
            && authority.trace.run_id == authority.record.id
            && authority.trace.parent_family == authority.record.parent_family
            && authority.trace.purpose == authority.record.purpose
            && authority.trace.source_digest == hash(&authority.record.body)
            && authority.trace.source_digest == source.content_digest
            && authority
                .record
                .body
                .get(authority.excerpt_start..authority.excerpt_end)
                == Some(authority.trace.excerpt.as_bytes());
        if !valid {
            return Err(Error::Invalid(
                "source is legacy, unverified, or changed".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    async fn read(
        store: &Store,
        ctx: &Context,
        id: &str,
    ) -> Result<Option<ReplayStoreEnvelope<Value>>> {
        let mut session = store.session().await.unwrap();
        let envelope = read_envelope::<Value>(&mut session, ctx, REPLAY_POOL_RECORD_KIND, id).await;
        session.commit().await.unwrap();
        envelope
    }

    #[tokio::test]
    async fn an_envelope_read_names_a_redacted_record_and_leaves_every_other_body_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("replay-envelope.sqlite3"))
            .await
            .unwrap();
        let ctx = Context::new("n", "admin", Role::Admin).unwrap();
        let put = |id: &'static str, body: Value| {
            let (store, ctx) = (store.clone(), ctx.clone());
            async move {
                let mut session = store.session().await.unwrap();
                session
                    .put(&ctx, "artifact", id, "admin", &body)
                    .await
                    .unwrap();
                session.commit().await.unwrap();
            }
        };
        put(
            "live",
            json!({
                "schema_version": REPLAY_STORE_ENVELOPE_SCHEMA,
                "id": "live",
                "namespace": "n",
                "record_kind": REPLAY_POOL_RECORD_KIND,
                "payload": {"member": 1},
            }),
        )
        .await;
        put(
            "redacted",
            json!({"id": "redacted", "schema_version": REDACTED_SCHEMA, "state": "source_revoked"}),
        )
        .await;
        put(
            "damaged",
            json!({"schema_version": REPLAY_STORE_ENVELOPE_SCHEMA, "id": "damaged"}),
        )
        .await;
        put(
            "not-a-tombstone",
            json!({"schema_version": "rsia.redacted.v2"}),
        )
        .await;
        put("not-an-object", json!([REDACTED_SCHEMA])).await;

        // Absent: no row, no error (the caller decides what absence means).
        assert!(read(&store, &ctx, "absent").await.unwrap().is_none());
        // A decodable envelope is returned as it is.
        let envelope = read(&store, &ctx, "live").await.unwrap().unwrap();
        assert_eq!(envelope.id, "live");
        assert_eq!(envelope.payload, json!({"member": 1}));
        // A tombstone is the expected state of a revoked record: a named Conflict.
        match read(&store, &ctx, "redacted").await {
            Err(Error::Conflict(message)) => assert_eq!(
                message,
                "replay replay_pool_manifest_v1 redacted was redacted because its source was revoked"
            ),
            other => panic!("a redacted row must be a named Conflict, got {other:?}"),
        }
        // Everything else that does not decode is corruption.
        for id in ["damaged", "not-a-tombstone", "not-an-object"] {
            assert!(
                matches!(read(&store, &ctx, id).await, Err(Error::Internal)),
                "{id} must stay Internal"
            );
        }
    }
}
