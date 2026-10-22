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
    pub worlds: Vec<LoadedReplayWorld>,
}

#[derive(Debug, Clone)]
pub enum LoadedReplayWorld {
    /// Live checked legacy source; its root contract remains Unknown.
    LegacyV2(Box<ReplayWorldV2>),
    /// This variant needs the private complete V3 loader token.
    CurrentV3(Box<LoadedReplayWorldV3>),
}
impl LoadedReplayWorld {
    pub fn world_id(&self) -> &str {
        match self {
            Self::LegacyV2(w) => &w.manifest.world_id,
            Self::CurrentV3(w) => &w.world().manifest.world_id,
        }
    }
    pub fn partition(&self) -> evo_core::replay::WorldPartition {
        match self {
            Self::LegacyV2(w) => w.manifest.partition,
            Self::CurrentV3(w) => w.world().manifest.partition,
        }
    }
    pub fn digest(&self) -> Result<&str> {
        match self {
            Self::LegacyV2(w) => w.sealed_digest.as_deref(),
            Self::CurrentV3(w) => w.world().sealed_digest.as_deref(),
        }
        .ok_or_else(|| Error::Invalid("replay world is unsealed".into()))
    }
    pub fn fingerprint(&self) -> Result<String> {
        match self {
            Self::LegacyV2(w) => fingerprint(w),
            Self::CurrentV3(w) => fingerprint(w.world()),
        }
    }
    pub fn pool_projection(
        &self,
    ) -> Result<(
        evo_core::replay::ReplayPoolCompatibilityV1,
        evo_core::replay::ReplayPoolMemberV1,
    )> {
        Ok(match self {
            Self::LegacyV2(world) => (
                evo_core::replay::ReplayPoolCompatibilityV1 {
                    purpose: world.manifest.purpose,
                    generation_signature: world.manifest.generation_signature.clone(),
                    approved_parent_digest: world.manifest.approved_parent_digest.clone(),
                    model_digest: world.manifest.model_digest.clone(),
                    tools_digest: world.manifest.tools_digest.clone(),
                    scorer_digest: world.manifest.scorer_digest.clone(),
                    guidance_digest: world.manifest.guidance_digest.clone(),
                    repair_template_digest: world.manifest.repair_template_digest.clone(),
                    revoke_watermark: world.manifest.revoke_watermark,
                },
                evo_core::replay::ReplayPoolMemberV1 {
                    world_id: world.manifest.world_id.clone(),
                    world_digest: self.digest()?.to_owned(),
                    cluster_id: world.manifest.cluster_id.clone(),
                    partition: world.manifest.partition,
                },
            ),
            Self::CurrentV3(loaded) => (
                evo_core::replay::ReplayPoolCompatibilityV1 {
                    purpose: loaded.world().manifest.purpose,
                    generation_signature: loaded.world().manifest.generation_signature.clone(),
                    approved_parent_digest: loaded.world().manifest.approved_parent_digest.clone(),
                    model_digest: loaded.world().manifest.model_digest.clone(),
                    tools_digest: loaded.world().manifest.tools_digest.clone(),
                    scorer_digest: loaded.world().manifest.scorer_digest.clone(),
                    guidance_digest: loaded.world().manifest.guidance_digest.clone(),
                    repair_template_digest: loaded.world().manifest.repair_template_digest.clone(),
                    revoke_watermark: loaded.world().manifest.revoke_watermark,
                },
                evo_core::replay::ReplayPoolMemberV1 {
                    world_id: loaded.world().manifest.world_id.clone(),
                    world_digest: self.digest()?.to_owned(),
                    cluster_id: loaded.world().manifest.cluster_id.clone(),
                    partition: loaded.world().manifest.partition,
                },
            ),
        })
    }
    pub async fn revalidate_in_session(&self, session: &mut Session, ctx: &Context) -> Result<()> {
        match self {
            Self::CurrentV3(world) => world.revalidate_in_session(session, ctx).await,
            Self::LegacyV2(world) => {
                let live = load_live_replay_world_in_session(
                    session,
                    ctx,
                    &world.manifest.world_id,
                    Purpose::Development,
                )
                .await?;
                if fingerprint(&live)? != fingerprint(world)? {
                    return Err(Error::Conflict("legacy world changed during replay".into()));
                }
                Ok(())
            }
        }
    }
}
impl LoadedReplayPool {
    pub fn validate_projection(&self) -> Result<()> {
        let inputs = self
            .worlds
            .iter()
            .map(LoadedReplayWorld::pool_projection)
            .collect::<Result<Vec<_>>>()?;
        if ReplayPoolManifestV1::build_from_projections(&inputs)? != self.manifest {
            return Err(Error::Conflict(
                "replay pool differs from its qualified worlds".into(),
            ));
        }
        Ok(())
    }
}
async fn load_pool_world_in_session(
    session: &mut Session,
    ctx: &Context,
    world_id: &str,
    purpose: Purpose,
) -> Result<LoadedReplayWorld> {
    let (_, raw) = replay_root_existing_world_raw_in_session(session, ctx, world_id).await?;
    if is_redacted(&raw) {
        return Err(redacted_record(REPLAY_WORLD_RECORD_KIND, world_id));
    }
    match raw
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
    {
        Some(REPLAY_WORLD_SCHEMA) => Ok(LoadedReplayWorld::LegacyV2(Box::new(
            load_live_replay_world_in_session(session, ctx, world_id, purpose).await?,
        ))),
        Some(REPLAY_WORLD_SCHEMA_V3) => Ok(LoadedReplayWorld::CurrentV3(Box::new(
            load_replay_world_v3_in_session(session, ctx, world_id, purpose).await?,
        ))),
        _ => Err(Error::Invalid(
            "unsupported sealed replay world version".into(),
        )),
    }
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
        worlds.push(load_pool_world_in_session(&mut session, ctx, id, Purpose::Development).await?);
    }
    let projections = worlds
        .iter()
        .map(LoadedReplayWorld::pool_projection)
        .collect::<Result<Vec<_>>>()?;
    let manifest = ReplayPoolManifestV1::build_from_projections(&projections)?;
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
            .put_edge(ctx, "artifact", &id, "replay_world", world.world_id())
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

pub async fn load_live_replay_pool_in_session(
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
            load_pool_world_in_session(session, ctx, &member.world_id, Purpose::Development)
                .await?,
        );
    }
    let loaded = LoadedReplayPool {
        manifest: envelope.payload,
        worlds,
    };
    loaded.validate_projection()?;
    Ok(loaded)
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
    // v4.3 5.1.1/7.4.3: old judgments are historical, not newly
    // issued by this runtime. Exact existing retries above remain read-only.
    // Smallest fail-closed API interpretation, pending secondary acceptance.
    Err(Error::Invalid("legacy_replay_new_write_forbidden".into()))
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
    let Some(row) = session.object_row(ctx, "artifact", id).await? else {
        return Ok(None);
    };
    identifier(&row.owner)?;
    let body = replay_root_strict_json(&row.body)?;
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

// AG147 non-persistent historical read/query adapter. This does not change the
// old stored envelope, JSON, IDs, rows or any source-revocation authority.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayLineageEntry {
    pub engine_version: String,
    pub report_id: String,
    pub exists: bool,
    pub record_state: Option<evo_core::replay::ReplayRecordState>,
    pub isolation_reason: Option<String>,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayInvalidDiagnostic {
    EnvelopeIdentity,
    MissingEngineVersion,
    StoredWire,
    RowNamespaceBinding,
    StaticIdentity,
}
impl ReplayInvalidDiagnostic {
    fn code(self) -> &'static str {
        match self {
            Self::EnvelopeIdentity => "replay_envelope_identity_invalid",
            Self::MissingEngineVersion => "replay_engine_version_missing",
            Self::StoredWire => "replay_historical_wire_invalid",
            Self::RowNamespaceBinding => "replay_row_namespace_binding_invalid",
            Self::StaticIdentity => "replay_static_identity_invalid",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct ReplayReportRecordQuery {
    pub record: evo_core::replay::ReplayStaticReportRead,
    // Present ONLY after this Session qualifies actual live dependencies and
    // static Legacy identity. Invalid/isolation carries no body or fragment.
    pub raw_payload: Option<serde_json::Value>,
    pub invalid_diagnostic: Option<ReplayInvalidDiagnostic>,
    pub source_legacy: bool,
    pub lineage: Vec<ReplayLineageEntry>,
}
fn invalid_replay_report_query(diagnostic: ReplayInvalidDiagnostic) -> ReplayReportRecordQuery {
    ReplayReportRecordQuery {
        record: evo_core::replay::ReplayStaticReportRead::invalid(diagnostic.code()),
        raw_payload: None,
        invalid_diagnostic: Some(diagnostic),
        source_legacy: false,
        lineage: Vec::new(),
    }
}
/// Same role/namespace/live closure+watermark guards as the original report
/// read. All reads including lineage reuse one actual Session; never open a
/// nested transaction. This query writes/audits/recomputes nothing.
pub async fn query_replay_report_record(
    ctx: &Context,
    store: &Store,
    report_id: &str,
) -> evo_core::replay::ReplayReadResult<ReplayReportRecordQuery> {
    ctx.require(&[Role::Worker, Role::Admin, Role::Evaluator])?;
    identifier(report_id)?;
    let mut session = store.session().await?;
    let view = query_replay_report_record_in_session(&mut session, ctx, report_id).await?;
    session.commit().await?;
    Ok(view)
}
pub async fn query_replay_report_record_in_session(
    session: &mut Session,
    ctx: &Context,
    report_id: &str,
) -> evo_core::replay::ReplayReadResult<ReplayReportRecordQuery> {
    use evo_core::replay::{
        ReplayReportIdentityVersion, classify_historical_replay_report, replay_historical_version,
        replay_report_id_for_version,
    };
    ctx.require(&[Role::Worker, Role::Admin, Role::Evaluator])?;
    identifier(report_id)?;
    let envelope: ReplayStoreEnvelope<serde_json::Value> =
        read_envelope(session, ctx, REPLAY_REPORT_RECORD_KIND, report_id)
            .await?
            .ok_or(Error::NotFound)?;
    let raw_payload = envelope.payload.clone();
    // Read identity failure is INVALID, distinct from LEGACY and table-external
    // quarantine. Original source-revoked/redacted/role errors remain distinct.
    if envelope.schema_version != REPLAY_STORE_ENVELOPE_SCHEMA
        || envelope.id != report_id
        || envelope.namespace != ctx.namespace()
        || envelope.record_kind != REPLAY_REPORT_RECORD_KIND
    {
        return Ok(invalid_replay_report_query(
            ReplayInvalidDiagnostic::EnvelopeIdentity,
        ));
    }
    let Some(engine) = raw_payload
        .get("replay_engine_version")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(invalid_replay_report_query(
            ReplayInvalidDiagnostic::MissingEngineVersion,
        ));
    };
    // Table-external denial remains typed, but never reflects an unqualified
    // caller-controlled engine value in its error/body before source proof.
    match replay_historical_version(engine) {
        Ok(_) => {}
        Err(evo_core::replay::ReplayReadIsolation::VersionOutsideStaticTable { .. }) => {
            return Err(
                evo_core::replay::ReplayReadIsolation::UnqualifiedVersionOutsideStaticTable,
            );
        }
        Err(error) => return Err(error),
    }
    let report: StoredReplayReportV1 = match serde_json::from_value(raw_payload.clone()) {
        Ok(report) => report,
        Err(_) => {
            return Ok(invalid_replay_report_query(
                ReplayInvalidDiagnostic::StoredWire,
            ));
        }
    };
    if report.report_id != report_id || report.namespace != ctx.namespace() {
        return Ok(invalid_replay_report_query(
            ReplayInvalidDiagnostic::RowNamespaceBinding,
        ));
    }
    // Reuse the original live dependency/source/namespace/watermark validation.
    // Revocation rules are not waived because an engine version became legacy.
    let pool = load_live_replay_pool_in_session(session, ctx, &report.pool_digest).await?;
    let record = classify_historical_replay_report(report, &pool.manifest)?;
    if record.state() != evo_core::replay::ReplayRecordState::Legacy {
        // A claimed pool/id is not authorization for malformed report content.
        // Even post-guard Invalid never returns body or validator error detail.
        return Ok(invalid_replay_report_query(
            ReplayInvalidDiagnostic::StaticIdentity,
        ));
    }
    let source_legacy = record.source_legacy();
    let mut lineage = Vec::new();
    if let Some(report) = record.report() {
        for version in [
            ReplayReportIdentityVersion::LegacyV2,
            ReplayReportIdentityVersion::NextV3,
        ] {
            let id = replay_report_id_for_version(
                version,
                &report.namespace,
                report.partition,
                &report.pool_digest,
                &report.policy_digest,
                &report.profile_digest,
                &report.caps_digest,
            )?;
            if version == ReplayReportIdentityVersion::NextV3 {
                let candidate: Option<ReplayStoreEnvelope<serde_json::Value>> =
                    read_envelope(session, ctx, CURRENT_REPLAY_REPORT_RECORD_KIND, &id).await?;
                let exists = candidate.is_some();
                let (record_state, isolation_reason) = if let Some(candidate) = candidate {
                    match serde_json::from_value::<evo_core::replay::StoredReplayReportV2>(
                        candidate.payload,
                    ) {
                        Ok(current)
                            if candidate.schema_version == REPLAY_STORE_ENVELOPE_SCHEMA
                                && candidate.namespace == ctx.namespace()
                                && candidate.id == id
                                && candidate.record_kind == CURRENT_REPLAY_REPORT_RECORD_KIND
                                && current.report_id == id
                                && current.namespace == report.namespace
                                && current.partition == report.partition
                                && current.pool_digest == report.pool_digest
                                && current.policy_digest == report.policy_digest
                                && current.profile_digest == report.profile_digest
                                && current.caps_digest == report.caps_digest
                                && current.validate_static(&pool.manifest).is_ok() =>
                        {
                            match load_current_replay_report_in_session(session, ctx, &id).await {
                                Ok(_) => (
                                    None,
                                    Some("current_semantics_requires_engine_verifier".into()),
                                ),
                                Err(Error::Invalid(_)) => {
                                    (Some(evo_core::replay::ReplayRecordState::Invalid), None)
                                }
                                Err(error) => return Err(error.into()),
                            }
                        }
                        _ => (Some(evo_core::replay::ReplayRecordState::Invalid), None),
                    }
                } else {
                    (None, None)
                };
                lineage.push(ReplayLineageEntry {
                    engine_version: version.engine_version().into(),
                    report_id: id,
                    exists,
                    record_state,
                    isolation_reason,
                });
                continue;
            }
            let candidate: Option<ReplayStoreEnvelope<serde_json::Value>> =
                read_envelope(session, ctx, REPLAY_REPORT_RECORD_KIND, &id).await?;
            let exists = candidate.is_some();
            let (state, isolation) = match candidate {
                None => (None, None),
                Some(candidate) => {
                    if candidate.schema_version != REPLAY_STORE_ENVELOPE_SCHEMA
                        || candidate.id != id
                        || candidate.namespace != ctx.namespace()
                        || candidate.record_kind != REPLAY_REPORT_RECORD_KIND
                    {
                        (Some(evo_core::replay::ReplayRecordState::Invalid), None)
                    } else {
                        match candidate
                            .payload
                            .get("replay_engine_version")
                            .and_then(serde_json::Value::as_str)
                        {
                            Some(found) if found != version.engine_version() => {
                                (Some(evo_core::replay::ReplayRecordState::Invalid), None)
                            }
                            Some(found) => match replay_historical_version(found) {
                                Err(error) => (None, Some(error.to_string())),
                                Ok(_) => match serde_json::from_value::<StoredReplayReportV1>(
                                    candidate.payload,
                                ) {
                                    Err(_) => {
                                        (Some(evo_core::replay::ReplayRecordState::Invalid), None)
                                    }
                                    Ok(candidate) => {
                                        let same_inputs = candidate.report_id == id
                                            && candidate.namespace == report.namespace
                                            && candidate.partition == report.partition
                                            && candidate.pool_digest == report.pool_digest
                                            && candidate.policy_digest == report.policy_digest
                                            && candidate.profile_digest == report.profile_digest
                                            && candidate.caps_digest == report.caps_digest;
                                        if !same_inputs {
                                            (
                                                Some(evo_core::replay::ReplayRecordState::Invalid),
                                                None,
                                            )
                                        } else {
                                            let classified = classify_historical_replay_report(
                                                candidate,
                                                &pool.manifest,
                                            )?;
                                            (Some(classified.state()), None)
                                        }
                                    }
                                },
                            },
                            None => (Some(evo_core::replay::ReplayRecordState::Invalid), None),
                        }
                    }
                }
            };
            lineage.push(ReplayLineageEntry {
                engine_version: version.engine_version().into(),
                report_id: id,
                exists,
                record_state: state,
                isolation_reason: isolation,
            });
        }
    }
    // Both qualification gates above ran in this same Session. This is the
    // sole body-bearing return and it preserves the exact authorized old value.
    Ok(ReplayReportRecordQuery {
        record,
        raw_payload: Some(raw_payload),
        invalid_diagnostic: None,
        source_legacy,
        lineage,
    })
}

// AG147 R5: same-Session source admission. This is not a world/observation writer.
use evo_core::replay::{
    REPLAY_ROOT_SOURCE_SCHEMA, ReplayRootArtifactRefV1, ReplayRootOpportunityV1,
    ReplayRootOriginalRequestRefV1, ReplayRootRegistrationEvidenceV1, ReplayRootSourceV1,
};
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, ElasticPolicyV1, ExplorationCapsV1, LegalActionV1,
    OpportunityWait, PrefixNodeV2, SimulationContext,
};
use std::collections::{BTreeMap, BTreeSet};

/// No Deserialize/public-field constructor. Contents are minimized registration
/// material, never the raw management request, actor Context, or a paid capability.
pub struct ValidatedReplayRootSource {
    source: ReplayRootSourceV1,
    registration_image_utf8: Vec<u8>,
}
impl ValidatedReplayRootSource {
    pub fn source(&self) -> &ReplayRootSourceV1 {
        &self.source
    }
    pub fn registration_image_utf8(&self) -> &[u8] {
        &self.registration_image_utf8
    }
}
fn replay_root_source_unproven() -> Error {
    Error::Conflict("replay_root_source_unproven".into())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayExplorationDependencyWire {
    kind: String,
    id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplayExplorationStateWire {
    Collecting,
    Sealed,
    Stopped,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayExplorationWorldWire {
    schema_version: String,
    id: String,
    approved_parent_digest: String,
    context_signature: String,
    parent_skill_digest: String,
    parent_bundle_digest: String,
    environment_digest: String,
    model_digest: String,
    tools_digest: String,
    grader_digest: String,
    rules_digest: String,
    source_watermark: u64,
    caps: ExplorationCapsV1,
    policy: ElasticPolicyV1,
    simulation: SimulationContext,
    root_opportunities: Vec<ReplayRootOpportunityV1>,
    dependencies: Vec<ReplayExplorationDependencyWire>,
    successor_cost_upper_micros: u64,
    initial_baseline_quality_micros: u32,
    remaining_root_micros: u64,
    remaining_recovery_dispatches: u8,
    state: ReplayExplorationStateWire,
    node_ids: Vec<String>,
    dispatch_ids: Vec<String>,
    history_ids: Vec<String>,
    current_branch_seq: Option<u32>,
    current_branch_focus_actions: u8,
    decision_round: u32,
    waits: Vec<OpportunityWait>,
}
impl ReplayExplorationWorldWire {
    // Exactly the existing registration_fingerprint tuple, including the
    // ORIGINAL roots Vec order. The independently sorted replay roots are separate.
    fn registration_projection_utf8(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&(
            &self.schema_version,
            &self.id,
            &self.approved_parent_digest,
            &self.context_signature,
            (
                &self.parent_skill_digest,
                &self.parent_bundle_digest,
                &self.environment_digest,
                &self.model_digest,
                &self.tools_digest,
                &self.grader_digest,
                &self.rules_digest,
            ),
            self.source_watermark,
            &self.caps,
            &self.policy,
            self.simulation,
            &self.root_opportunities,
            &self.dependencies,
            (
                self.successor_cost_upper_micros,
                self.initial_baseline_quality_micros,
            ),
        ))
        .map_err(|_| Error::Internal)
    }
    fn validate_registration_shape(&self) -> Result<()> {
        if self.schema_version != "rsia.exploration_world.v1" {
            return Err(replay_root_source_unproven());
        }
        identifier(&self.id)?;
        for digest in [
            &self.approved_parent_digest,
            &self.context_signature,
            &self.parent_skill_digest,
            &self.parent_bundle_digest,
            &self.environment_digest,
            &self.model_digest,
            &self.tools_digest,
            &self.grader_digest,
            &self.rules_digest,
        ] {
            validate_digest(digest)?;
        }
        if self.context_signature
            != fingerprint(&(
                &self.parent_skill_digest,
                &self.parent_bundle_digest,
                &self.environment_digest,
                &self.model_digest,
                &self.tools_digest,
                &self.grader_digest,
                &self.rules_digest,
                self.source_watermark,
            ))?
            || self.successor_cost_upper_micros == 0
            || self.initial_baseline_quality_micros > 1_000_000
            || self.node_ids.len() > usize::from(self.caps.max_nodes)
        {
            return Err(replay_root_source_unproven());
        }
        self.caps.validate()?;
        self.policy.validate()?;
        self.simulation.width()?;
        let mut dependencies = BTreeSet::new();
        if self.dependencies.is_empty() {
            return Err(replay_root_source_unproven());
        }
        for dependency in &self.dependencies {
            identifier(&dependency.id)?;
            if dependency.kind != "run" || !dependencies.insert((&dependency.kind, &dependency.id))
            {
                return Err(replay_root_source_unproven());
            }
        }
        for id in self
            .node_ids
            .iter()
            .chain(&self.dispatch_ids)
            .chain(&self.history_ids)
        {
            identifier(id)?;
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayExplorationEnvelope<T> {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: T,
}
fn replay_exploration_storage_id(kind: &str, id: &str) -> Result<String> {
    identifier(kind)?;
    identifier(id)?;
    Ok(format!("e09-{}", fingerprint(&(kind, id))?))
}
async fn replay_exploration_record<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
) -> Result<T> {
    let physical = replay_exploration_storage_id(kind, id)?;
    let (_, raw) = replay_root_physical_original(session, ctx, "artifact", &physical).await?;
    let body = replay_root_strict_json(&raw)?;
    if body
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        == Some(REDACTED_SCHEMA)
    {
        return Err(Error::Forbidden);
    }
    let envelope: ReplayExplorationEnvelope<T> =
        serde_json::from_value(body).map_err(|_| replay_root_source_unproven())?;
    if envelope.schema_version != "rsia.exploration_artifact_envelope.v1"
        || envelope.id != physical
        || envelope.record_kind != kind
    {
        return Err(replay_root_source_unproven());
    }
    Ok(envelope.payload)
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum ReplayExplorationEvidenceWire {
    Trusted,
    FixtureDeclared,
    EvidenceRejected,
    #[default]
    NotObserved,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplayStepErrorKindWire {
    Invalid,
    Forbidden,
    NotFound,
    Budget,
    Conflict,
    Cancelled,
    Internal,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplayModelRejectionKindWire {
    Unauthorized,
    BudgetUnavailable,
    InvalidRequest,
    ProviderRejected,
    CancelledBeforeDispatch,
    CancelledAfterDispatch,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
enum ReplayStepTerminalClassWire {
    NoEligibleReflectionBatch,
    NoEditSuggestions,
    EditProducedNoChange,

    KeepIncumbentNotImproved {
        parent_total_micros: u64,
        candidate_total_micros: u64,
    },
    KeepIncumbentRetentionBroken {
        parent_total_micros: u64,
        candidate_total_micros: u64,
    },

    GrantUnavailable,
    ModelRejected {
        kind: ReplayModelRejectionKindWire,
    },
    SuggestionShapeInvalid,
    SuggestionPoolUnranked,
    EditCompileFailed,
    BundleCompileFailed,
    DevelopmentReportRejected,
    StepError {
        error: ReplayStepErrorKindWire,
    },

    ModelUsageUnknown,
    ModelDispatchUnrecorded,
    ModelDispatchConcurrent,
    ModelTransportOutcomeUnknown,
    DevelopmentExecutionUnrecorded,
    DevelopmentExecutionConcurrent,
    DevelopmentExecutionOutcomeUnknown,
    CancelledOrUncertain,
    OptimizationDispatchOutcomeUncertain,
    ExplorationSourceClosureChangedAfterDispatch,
    ExplorationPrefixChangedAfterDispatch,
    ClaimInputsChangedAfterDispatch,
    RecoverTargetMissing,
    DevelopmentEvidenceUnverified,
    CandidateMaterialUnavailable,

    CandidateObserved,
    FixtureEvidenceNotAccepted,
    DevelopmentEvidenceRejected,

    LegacyUnclassified,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayExplorationNodeWire {
    schema_version: String,
    world_id: String,
    node: PrefixNodeV2,
    candidate_bundle_digest: Option<String>,
    candidate_skill_digest: Option<String>,
    development_selection_digest: Option<String>,
    intermediate_only: bool,
    #[serde(default)]
    evidence: ReplayExplorationEvidenceWire,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_class: Option<ReplayStepTerminalClassWire>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayExplorationDecisionWire {
    world_id: String,
    prefix_digest: String,
    legal_actions_digest: String,
    policy_digest: String,
    caps_digest: String,
    action: BatchActionV1,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplayExplorationDispatchStateWire {
    Claimed,
    Observed,
    Uncertain,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayExplorationDispatchWire {
    schema_version: String,
    id: String,
    world_id: String,
    action_id: String,
    action_seq: u32,
    request_digest: String,
    idempotency_key: String,
    decision: ReplayExplorationDecisionWire,
    selected_action: LegalActionV1,
    expected_parent_skill_digest: String,
    expected_parent_bundle_digest: String,
    context_signature: String,
    state: ReplayExplorationDispatchStateWire,
    node_id: Option<String>,
    outcome_reason: Option<String>,
    #[serde(default)]
    evidence: ReplayExplorationEvidenceWire,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_class: Option<ReplayStepTerminalClassWire>,
}
async fn replay_registered_budget_in_session(
    session: &mut Session,
    ctx: &Context,
    world: &ReplayExplorationWorldWire,
) -> Result<(u64, u8)> {
    let mut nodes = BTreeMap::new();
    for (index, id) in world.node_ids.iter().enumerate() {
        let node: ReplayExplorationNodeWire =
            replay_exploration_record(session, ctx, "exploration_node_v1", id).await?;
        if node.schema_version != "rsia.exploration_node.v1"
            || node.world_id != world.id
            || node.node.node_seq
                != u32::try_from(index + 1).map_err(|_| replay_root_source_unproven())?
            || id != &format!("node-{}-{}", world.id, node.node.node_seq)
            || nodes.insert(node.node.node_seq, node.node).is_some()
        {
            return Err(replay_root_source_unproven());
        }
    }
    let mut root_micros = world.remaining_root_micros;
    let mut repairs = u32::from(world.remaining_recovery_dispatches);
    let mut facts = BTreeSet::new();
    let mut completed = BTreeSet::new();
    for id in &world.dispatch_ids {
        if !facts.insert(id) {
            return Err(replay_root_source_unproven());
        }
        let fact: ReplayExplorationDispatchWire =
            replay_exploration_record(session, ctx, "exploration_dispatch_v1", id).await?;
        let BatchActionV1::Dispatch {
            action_ids,
            action_seqs,
            estimated_cost_upper_micros,
            ..
        } = &fact.decision.action
        else {
            return Err(replay_root_source_unproven());
        };
        if fact.schema_version != "rsia.exploration_dispatch.v2"
            || fact.id != *id
            || fact.world_id != world.id
            || fact.decision.world_id != world.id
            || fact.context_signature != world.context_signature
            || fact.decision.policy_digest != fingerprint(&world.policy)?
            || fact.decision.caps_digest != fingerprint(&world.caps)?
            || action_ids.len() != 1
            || action_seqs.len() != 1
            || action_ids[0] != fact.action_id
            || action_seqs[0] != fact.action_seq
            || fact.selected_action.action_id != fact.action_id
            || fact.selected_action.action_seq != fact.action_seq
        {
            return Err(replay_root_source_unproven());
        }
        let cost = match &fact.selected_action.kind {
            ActionKindV1::Widen { root_slot } => {
                let root = world
                    .root_opportunities
                    .iter()
                    .find(|root| root.root_slot == *root_slot)
                    .ok_or_else(replay_root_source_unproven)?;
                let expected = LegalActionV1 {
                    action_id: format!("root-{}", root.root_slot),
                    action_seq: root.action_seq,
                    branch_seq: root.branch_seq,
                    target_depth: 1,
                    kind: ActionKindV1::Widen {
                        root_slot: root.root_slot,
                    },
                    estimated_cost_upper_micros: Some(root.estimated_cost_upper_micros),
                };
                if fingerprint(&expected)? != fingerprint(&fact.selected_action)? {
                    return Err(replay_root_source_unproven());
                }
                root.estimated_cost_upper_micros
            }
            ActionKindV1::Deepen { .. } | ActionKindV1::Recover { .. } => {
                if world
                    .root_opportunities
                    .iter()
                    .any(|root| root.action_seq == fact.action_seq)
                {
                    return Err(replay_root_source_unproven());
                }
                world.successor_cost_upper_micros
            }
        };
        if *estimated_cost_upper_micros != cost
            || fact.selected_action.estimated_cost_upper_micros != Some(cost)
        {
            return Err(replay_root_source_unproven());
        }
        match fact.state {
            ReplayExplorationDispatchStateWire::Claimed => {
                if fact.node_id.is_some() {
                    return Err(replay_root_source_unproven());
                }
            }
            ReplayExplorationDispatchStateWire::Observed
            | ReplayExplorationDispatchStateWire::Uncertain => {
                let id = fact
                    .node_id
                    .as_ref()
                    .ok_or_else(replay_root_source_unproven)?;
                let index = world
                    .node_ids
                    .iter()
                    .position(|listed| listed == id)
                    .ok_or_else(replay_root_source_unproven)?;
                let node_seq =
                    u32::try_from(index + 1).map_err(|_| replay_root_source_unproven())?;
                let node = nodes
                    .get(&node_seq)
                    .ok_or_else(replay_root_source_unproven)?;
                if !completed.insert(node_seq) {
                    return Err(replay_root_source_unproven());
                }
                let parent = match &fact.selected_action.kind {
                    ActionKindV1::Widen { .. } => None,
                    ActionKindV1::Deepen { parent_node_seq } => Some(*parent_node_seq),
                    ActionKindV1::Recover {
                        failed_node_seq, ..
                    } => Some(*failed_node_seq),
                };
                let depth = if parent.is_none() {
                    1
                } else {
                    fact.selected_action.target_depth
                };
                if node.search_parent_seq != parent
                    || node.branch_seq != fact.selected_action.branch_seq
                    || node.depth != depth
                    || parent.is_some_and(|parent| {
                        !nodes.get(&parent).is_some_and(|older| {
                            older.node_seq < node.node_seq
                                && older.branch_seq == node.branch_seq
                                && older.depth.checked_add(1) == Some(node.depth)
                        })
                    })
                {
                    return Err(replay_root_source_unproven());
                }
                if let ActionKindV1::Recover {
                    failed_node_seq, ..
                } = &fact.selected_action.kind
                {
                    use evo_core::strategy::{FailureKind, ObservedStatus};
                    let target = nodes
                        .get(failed_node_seq)
                        .ok_or_else(replay_root_source_unproven)?;
                    if !matches!(
                        target.status,
                        ObservedStatus::RepairableFailure {
                            failure_kind: FailureKind::Compile
                                | FailureKind::Implementation
                                | FailureKind::Type
                                | FailureKind::OutputShape,
                            ..
                        }
                    ) {
                        return Err(replay_root_source_unproven());
                    }
                    repairs = repairs
                        .checked_add(1)
                        .ok_or_else(replay_root_source_unproven)?;
                }
                root_micros = root_micros
                    .checked_add(cost)
                    .ok_or_else(replay_root_source_unproven)?;
            }
        }
    }
    if completed.len() != world.node_ids.len() {
        return Err(replay_root_source_unproven());
    }
    Ok((
        root_micros,
        u8::try_from(repairs).map_err(|_| replay_root_source_unproven())?,
    ))
}

/// Reads original JSON bytes and physical owner inside this same Session. Unlike
/// raw_object<Value>, this does not mistake sorted reserialization for old bytes.
/// This helper is private and performs SELECT only; it adds no schema/connection.
async fn replay_root_physical_original(
    session: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
) -> Result<(String, String)> {
    let row = session
        .object_row(ctx, kind, id)
        .await?
        .ok_or(Error::NotFound)?;
    let owner = row.owner;
    let body = row.body;
    identifier(&owner)?;
    // Existing objects put gate: storage/lib.rs rejects bodies above 4 MiB.
    if body.len() > 4 * 1024 * 1024 {
        return Err(replay_root_source_unproven());
    }
    Ok((owner, body))
}
// Reuse the restore/control-plane parser before any Value/DTO/hash. Keep the
// public diagnostic fixed; private request field names and bytes are not exposed.
fn replay_root_strict_json(raw: &str) -> Result<serde_json::Value> {
    crate::lifecycle::parse_strict_json(raw)
        .map_err(|_| Error::Invalid("replay_root_source_invalid_json".into()))
}

// Read sealed storage bytes before Value can erase a duplicate. The eventual
// COMPLETE V3 loader must use this same private gate before minting its guard;
// this helper alone does not qualify a world or expose a creation capability.
async fn replay_root_existing_world_raw_in_session(
    session: &mut Session,
    ctx: &Context,
    id: &str,
) -> Result<(bool, serde_json::Value)> {
    use sqlx::Row as _;
    identifier(id)?;
    let row = sqlx::query("SELECT sealed,manifest FROM replay_worlds WHERE namespace=? AND id=?")
        .bind(ctx.namespace())
        .bind(id)
        .fetch_optional(&mut *session.tx)
        .await
        .map_err(|_| Error::Internal)?
        .ok_or(Error::NotFound)?;
    let sealed: i64 = row.try_get("sealed").map_err(|_| Error::Internal)?;
    let raw: String = row.try_get("manifest").map_err(|_| Error::Internal)?;
    if !matches!(sealed, 0 | 1) {
        return Err(replay_root_source_unproven());
    }
    Ok((sealed == 1, replay_root_strict_json(&raw)?))
}

// Preserve ParsedRequest/ExplorationStartRequest declaration-order serialization;
// hashing an untyped Value would sort nested objects and change the old digest.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "operation")]
enum ReplayRootManagementRequestWire {
    ExplorationStart(ReplayRootExplorationStartWire),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayRootExplorationStartWire {
    schema_version: String,
    request_key: String,
    world: ReplayExplorationWorldWire,
}
fn replay_management_stable_id(prefix: &str, namespace: &str, actor: &str, key: &str) -> String {
    let digest = hash(format!("{namespace}\0{actor}\0exploration.start\0{key}").as_bytes());
    format!("{prefix}-{}", &digest[..24])
}
async fn replay_management_originals_in_session(
    session: &mut Session,
    ctx: &Context,
    world: &ReplayExplorationWorldWire,
    world_artifact_id: &str,
    registered_budget: (u64, u8),
) -> Result<Vec<ReplayRootRegistrationEvidenceV1>> {
    // Discovery is only an overinclusive filter. json_tree visits EVERY duplicate
    // path, so an overwritten world/id/schema cannot hide an original. Each
    // selected physical row is then strict-parsed before any DTO or digest.
    // Unrelated families are not decoded as ExplorationWorld; actual incoming
    // references also select malformed originals, which fail the strict gate.
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM objects WHERE namespace=? AND kind='artifact' AND \
         CASE WHEN json_valid(body) THEN \
         EXISTS(SELECT 1 FROM json_tree(body) j WHERE j.fullkey='$.schema_version' \
                AND j.type='text' AND j.atom='rsia.management_private_input.v1') AND \
         EXISTS(SELECT 1 FROM json_tree(body) j WHERE j.fullkey='$.payload.operation' \
                AND j.type='text' AND j.atom='exploration_start') AND \
         EXISTS(SELECT 1 FROM json_tree(body) j WHERE j.fullkey='$.payload.world.id' \
                AND j.type='text' AND j.atom=?) ELSE 0 END ORDER BY id",
    )
    .bind(ctx.namespace())
    .bind(&world.id)
    .fetch_all(&mut *session.tx)
    .await
    .map_err(|_| Error::Internal)?;
    let mut ids: BTreeSet<String> = ids.into_iter().collect();
    for (kind, id) in session
        .dependents(ctx, "artifact", world_artifact_id)
        .await?
    {
        if kind == "artifact" && id.starts_with("management-input-") {
            ids.insert(id);
        }
    }
    let mut found = Vec::new();
    for input_id in ids {
        let (owner, body) =
            replay_root_physical_original(session, ctx, "artifact", &input_id).await?;
        let private = replay_root_strict_json(&body)?;
        let object = private
            .as_object()
            .ok_or_else(replay_root_source_unproven)?;
        let expected_keys: BTreeSet<_> = [
            "id",
            "schema_version",
            "operation",
            "payload_digest",
            "payload",
        ]
        .into_iter()
        .collect();
        if object.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected_keys
            || private["id"] != input_id
            || private["schema_version"] != "rsia.management_private_input.v1"
            || private["operation"] != "exploration.start"
            || private["payload"]["operation"] != "exploration_start"
        {
            return Err(replay_root_source_unproven());
        }
        let payload = private["payload"]
            .as_object()
            .ok_or_else(replay_root_source_unproven)?;
        if payload.keys().map(String::as_str).collect::<BTreeSet<_>>()
            != ["operation", "schema_version", "request_key", "world"]
                .into_iter()
                .collect()
        {
            return Err(replay_root_source_unproven());
        }
        if private["payload"]["schema_version"] != "rsia.management.exploration_start.v1" {
            return Err(replay_root_source_unproven());
        }
        let key = private["payload"]["request_key"]
            .as_str()
            .ok_or_else(replay_root_source_unproven)?;
        identifier(key)?;
        let expected_input =
            replay_management_stable_id("management-input", ctx.namespace(), &owner, key);
        let job_id = replay_management_stable_id("management-job", ctx.namespace(), &owner, key);
        if expected_input != input_id {
            return Err(replay_root_source_unproven());
        }
        let (job_owner, job_body) =
            replay_root_physical_original(session, ctx, "job", &job_id).await?;
        let job = replay_root_strict_json(&job_body)?;
        if job_owner != owner
            || job["id"] != job_id
            || job["schema_version"] != "rsia.management_job.v1"
            || job["operation"] != "exploration.start"
            || job["owner_actor"] != owner
            || job["owner_role"] != "admin"
            || job["private_input_ref"] != input_id
            || job["request_key"] != key
            || job["payload_digest"] != private["payload_digest"]
        {
            return Err(replay_root_source_unproven());
        }
        // The Admin engine bridge also visits incoming job owners. Prove every
        // such physical row strict here, in this same Session, before that old
        // typed decoder runs; an extra/foreign job cannot hide behind Value.
        for (kind, dependent_id) in session.dependents(ctx, "artifact", &input_id).await? {
            if kind != "job" {
                continue;
            }
            let (dependent_owner, dependent_body) =
                replay_root_physical_original(session, ctx, "job", &dependent_id).await?;
            let dependent = replay_root_strict_json(&dependent_body)?;
            if dependent_owner != owner || dependent_id != job_id || dependent != job {
                return Err(replay_root_source_unproven());
            }
        }
        if ctx.role() == Role::Admin && ctx.actor() != owner {
            return Err(Error::Forbidden);
        }
        let parsed: ReplayRootManagementRequestWire =
            serde_json::from_value(private["payload"].clone())
                .map_err(|_| replay_root_source_unproven())?;
        let expected_digest = fingerprint(&parsed)?;
        if private["payload_digest"].as_str() != Some(expected_digest.as_str()) {
            return Err(replay_root_source_unproven());
        }
        let requested: ReplayExplorationWorldWire =
            serde_json::from_value(private["payload"]["world"].clone())
                .map_err(|_| replay_root_source_unproven())?;
        requested.validate_registration_shape()?;
        if requested.id != world.id
            || requested.registration_projection_utf8()? != world.registration_projection_utf8()?
            || (
                requested.remaining_root_micros,
                requested.remaining_recovery_dispatches,
            ) != registered_budget
            || !requested.node_ids.is_empty()
            || !requested.dispatch_ids.is_empty()
            || !requested.history_ids.is_empty()
            || requested.decision_round != 0
            || requested.current_branch_seq.is_some()
            || requested.current_branch_focus_actions != 0
            || !requested.waits.is_empty()
            || !matches!(requested.state, ReplayExplorationStateWire::Collecting)
        {
            return Err(replay_root_source_unproven());
        }
        let input_edges = session.dependency_edges(ctx, "artifact", &input_id).await?;
        let mut expected_edges = world
            .dependencies
            .iter()
            .map(|dependency| (dependency.kind.clone(), dependency.id.clone()))
            .collect::<BTreeSet<_>>();
        expected_edges.insert(("artifact".into(), world_artifact_id.into()));
        if input_edges.into_iter().collect::<BTreeSet<_>>() != expected_edges
            || !session
                .dependency_edges(ctx, "job", &job_id)
                .await?
                .contains(&("artifact".into(), input_id.clone()))
        {
            return Err(replay_root_source_unproven());
        }
        found.push(ReplayRootRegistrationEvidenceV1::ManagementOriginal {
            job_id,
            original_request_ref: ReplayRootOriginalRequestRefV1 {
                kind: "artifact".into(),
                id: input_id,
                schema_version: "rsia.management_private_input.v1".into(),
                body_digest: hash(body.as_bytes()),
            },
            payload_digest: expected_digest,
        });
    }
    found.sort_by_key(|evidence| match evidence {
        ReplayRootRegistrationEvidenceV1::ManagementOriginal { job_id, .. } => job_id.clone(),
        _ => String::new(),
    });
    Ok(found)
}

// Private reconstruction is shared by the two explicit role-separated gates.
// It never creates a CurrentVerified report, WorldV3, observation, or paid arm.
async fn reconstruct_replay_root_source_in_session(
    session: &mut Session,
    ctx: &Context,
    exploration_world_id: &str,
    existing_evidence: Option<&ReplayRootRegistrationEvidenceV1>,
) -> Result<ValidatedReplayRootSource> {
    identifier(exploration_world_id)?;
    let physical = replay_exploration_storage_id("exploration_world_v1", exploration_world_id)?;
    let world: ReplayExplorationWorldWire =
        replay_exploration_record(session, ctx, "exploration_world_v1", exploration_world_id)
            .await?;
    world.validate_registration_shape()?;
    if world.id != exploration_world_id {
        return Err(replay_root_source_unproven());
    }
    for sequence in [1, u32::from(evo_core::strategy::MAX_NODES)] {
        identifier(&format!("node-{}-{sequence}", world.id))?;
    }
    let watermark = session
        .watermark(ctx)
        .await?
        .and_then(|(seq, _)| u64::try_from(seq).ok())
        .ok_or_else(replay_root_source_unproven)?;
    if world.source_watermark != watermark {
        return Err(replay_root_source_unproven());
    }
    let source_edges = world
        .dependencies
        .iter()
        .map(|dependency| (dependency.kind.clone(), dependency.id.clone()))
        .collect::<BTreeSet<_>>();
    if session
        .dependency_edges(ctx, "artifact", &physical)
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>()
        != source_edges
    {
        return Err(replay_root_source_unproven());
    }
    let closure = session
        .upstream_closure(ctx, &[("artifact", physical.as_str())], 10_000)
        .await?;
    let mut pins = Vec::new();
    let forbidden: BTreeSet<String> = world
        .node_ids
        .iter()
        .map(|id| replay_exploration_storage_id("exploration_node_v1", id))
        .chain(
            world
                .dispatch_ids
                .iter()
                .map(|id| replay_exploration_storage_id("exploration_dispatch_v1", id)),
        )
        .chain(
            world
                .history_ids
                .iter()
                .map(|id| replay_exploration_storage_id("optimization_history_v1", id)),
        )
        .collect::<Result<_>>()?;
    for node in closure {
        if node.tombstone.is_some() || node.redacted {
            return Err(Error::Forbidden);
        }
        if node.kind == "artifact" && forbidden.contains(&node.id) {
            return Err(replay_root_source_unproven());
        }
        if node.kind == "artifact" && node.id == physical {
            continue;
        }
        let (_, raw) = replay_root_physical_original(session, ctx, &node.kind, &node.id).await?;
        let body = replay_root_strict_json(&raw)?;
        if node.kind == "run" {
            let authority: StoredTraceAuthorityWire =
                serde_json::from_value(body.clone()).map_err(|_| replay_root_source_unproven())?;
            identifier(&authority.record.parent_family)?;
            if authority.schema_version != "rsia.optimization.source.v1"
                || authority.record.id != node.id
                || authority.trace.run_id != node.id
                || authority.record.task_origin != TaskOrigin::TrustedRun
                || authority.record.execution_attestation != ExecutionAttestation::TrustedHost
                || authority.record.purpose != Purpose::Development
                || authority.trace.purpose != Purpose::Development
                || authority.trace.parent_family != authority.record.parent_family
                || authority.trace.source_digest != hash(&authority.record.body)
                || authority
                    .record
                    .body
                    .get(authority.excerpt_start..authority.excerpt_end)
                    != Some(authority.trace.excerpt.as_bytes())
            {
                return Err(replay_root_source_unproven());
            }
        }
        pins.push((node.kind, node.id, fingerprint(&body)?));
    }
    pins.sort();
    let budget = replay_registered_budget_in_session(session, ctx, &world).await?;
    let originals =
        replay_management_originals_in_session(session, ctx, &world, &physical, budget).await?;
    // Existing sealed/collecting worlds retain their actual stored proof choice.
    // A later valid idempotent management job must not invalidate an earlier
    // genuine proof merely by sorting before it. ALL present originals were
    // nevertheless fully checked above, so a new conflicting one is rejected.
    let registration_evidence = match existing_evidence {
        Some(expected @ ReplayRootRegistrationEvidenceV1::ManagementOriginal { .. }) => originals
            .iter()
            .find(|actual| *actual == expected)
            .cloned()
            .ok_or_else(replay_root_source_unproven)?,
        Some(ReplayRootRegistrationEvidenceV1::RegisteredSourceChain {
            source_closure_digest,
        }) => {
            if *source_closure_digest != fingerprint(&pins)? {
                return Err(replay_root_source_unproven());
            }
            ReplayRootRegistrationEvidenceV1::RegisteredSourceChain {
                source_closure_digest: source_closure_digest.clone(),
            }
        }
        None => originals.into_iter().next().unwrap_or(
            ReplayRootRegistrationEvidenceV1::RegisteredSourceChain {
                source_closure_digest: fingerprint(&pins)?,
            },
        ),
    };
    let registration_projection_utf8 = world.registration_projection_utf8()?;
    let registration_digest = hash(&registration_projection_utf8);
    let registration_image_utf8 = serde_json::to_vec(&serde_json::json!({
        "schema_version": "rsia.replay.registration_image.v1",
        "registration_projection_utf8": String::from_utf8(registration_projection_utf8).map_err(|_| Error::Internal)?,
        "initial_root_budget_micros": budget.0,
        "initial_recovery_dispatches": budget.1,
        "source_closure": pins,
    })).map_err(|_| Error::Internal)?;
    // Source gap, pending secondary acceptance: a minimized registration image
    // above 4 MiB fails closed. This is NOT an existing ReplayWorld capacity.
    if registration_image_utf8.len() > 4 * 1024 * 1024 {
        return Err(replay_root_source_unproven());
    }
    let mut roots = world.root_opportunities.clone();
    roots.sort_by_key(|root| root.root_slot);
    let mut source = ReplayRootSourceV1 {
        schema_version: REPLAY_ROOT_SOURCE_SCHEMA.into(),
        namespace: ctx.namespace().into(),
        exploration_world_id: world.id.clone(),
        world_artifact_ref: ReplayRootArtifactRefV1 {
            kind: "artifact".into(),
            id: physical,
            schema_version: world.schema_version,
        },
        registration_digest,
        registration_evidence,
        registration_image_digest: hash(&registration_image_utf8),
        source_watermark: world.source_watermark,
        roots,
        structure_digest: String::new(),
    };
    source.structure_digest = source.expected_structure_digest()?;
    source.validate_material(&registration_image_utf8)?;
    Ok(ValidatedReplayRootSource {
        source,
        registration_image_utf8,
    })
}

/// Capturing new material is an explicit Admin operation. The factory and final
/// V3 writer must call this in their actual writing Session, compare the complete
/// returned source/image to the candidate, and only then perform their first put.
pub async fn capture_replay_root_source_in_session(
    session: &mut Session,
    ctx: &Context,
    exploration_world_id: &str,
) -> Result<ValidatedReplayRootSource> {
    ctx.require(&[Role::Admin])?;
    reconstruct_replay_root_source_in_session(session, ctx, exploration_world_id, None).await
}

/// Only load_replay_world_v3_in_session in THIS module constructs this, AFTER
/// actual strict row identity, complete static world/seal, every canonical
/// baseline/transition authority body, live source/S0 and dependency checks.
/// DTO/schema-only reads cannot mint it. The single construction site pins
/// fingerprint(the strict physical row Value) after those full checks.
#[derive(Debug, Clone)]
pub struct ExistingReplayWorldV3RootMaterial {
    replay_world_id: String,
    row_body_digest: String,
    source: ReplayRootSourceV1,
    registration_image_utf8: Vec<u8>,
}

/// A restricted reader only revalidates EXISTING, already-qualified V3 material.
/// It cannot request capture or receive any opaque creation/capture capability.
pub async fn validate_replay_root_source_in_session(
    session: &mut Session,
    ctx: &Context,
    existing: &ExistingReplayWorldV3RootMaterial,
) -> Result<()> {
    ctx.require(&[Role::Admin, Role::Worker, Role::Evaluator])?;
    identifier(&existing.replay_world_id)?;
    let (sealed, body) =
        replay_root_existing_world_raw_in_session(session, ctx, &existing.replay_world_id).await?;
    if is_redacted(&body) {
        return Err(Error::Forbidden);
    }
    if !sealed
        || fingerprint(&body)? != existing.row_body_digest
        || body["schema_version"] != "rsia.replay_world.v3"
        || body["manifest"]["schema_version"] != "rsia.replay_manifest.v3"
        || body["manifest"]["world_id"] != existing.replay_world_id
        || body["manifest"]["purpose"] != "development"
        || body["manifest"]["root_source"]
            != serde_json::to_value(&existing.source).map_err(|_| Error::Internal)?
        || body["registration_image_utf8"].as_str()
            != std::str::from_utf8(&existing.registration_image_utf8).ok()
        || existing.source.namespace != ctx.namespace()
    {
        return Err(replay_root_source_unproven());
    }
    existing
        .source
        .validate_material(&existing.registration_image_utf8)?;
    let current = reconstruct_replay_root_source_in_session(
        session,
        ctx,
        &existing.source.exploration_world_id,
        Some(&existing.source.registration_evidence),
    )
    .await?;
    if current.source != existing.source
        || current.registration_image_utf8 != existing.registration_image_utf8
    {
        return Err(replay_root_source_unproven());
    }
    let edges = session
        .dependency_edges(ctx, "replay_world", &existing.replay_world_id)
        .await?;
    if !edges.contains(&(
        "artifact".into(),
        existing.source.world_artifact_ref.id.clone(),
    )) {
        return Err(replay_root_source_unproven());
    }
    if let ReplayRootRegistrationEvidenceV1::ManagementOriginal {
        original_request_ref,
        ..
    } = &existing.source.registration_evidence
        && !edges.contains(&("artifact".into(), original_request_ref.id.clone()))
    {
        return Err(replay_root_source_unproven());
    }
    Ok(())
}

// AG147 R5: complete V3 collector/store/load qualification, not a run producer.
use evo_core::replay::{
    REPLAY_WORLD_SCHEMA_V3, ReplayWorldV3, replay_baseline_observation_bytes_v2,
    replay_observation_bytes_v3,
};
#[derive(Debug, Clone)]
pub struct LoadedReplayWorldV3 {
    world: ReplayWorldV3,
    // Constructed at exactly one site, after every full sealed-world check.
    root_material: ExistingReplayWorldV3RootMaterial,
}
impl LoadedReplayWorldV3 {
    pub fn world(&self) -> &ReplayWorldV3 {
        &self.world
    }
    pub async fn revalidate_in_session(&self, session: &mut Session, ctx: &Context) -> Result<()> {
        validate_replay_root_source_in_session(session, ctx, &self.root_material).await?;
        let current = load_replay_world_v3_in_session(
            session,
            ctx,
            &self.world.manifest.world_id,
            Purpose::Development,
        )
        .await?;
        if fingerprint(&current.world)? != fingerprint(&self.world)? {
            return Err(Error::Conflict(
                "replay world changed during consumption".into(),
            ));
        }
        Ok(())
    }
}

/// Only verifies actual stored authority. This code never builds, inserts or
/// relabels a StoredRunRecord, TaskOrigin or ExecutionAttestation.
async fn live_replay_run_v3_in_session(
    session: &mut Session,
    ctx: &Context,
    source_id: &str,
) -> Result<StoredTraceAuthorityWire> {
    identifier(source_id)?;
    if session
        .get::<serde_json::Value>(ctx, "tombstone", source_id)
        .await?
        .is_some()
    {
        return Err(Error::Forbidden);
    }
    let (_, raw) = replay_root_physical_original(session, ctx, "run", source_id).await?;
    let value = replay_root_strict_json(&raw)?;
    if is_redacted(&value) {
        return Err(Error::Forbidden);
    }
    let authority: StoredTraceAuthorityWire = serde_json::from_value(value)
        .map_err(|_| Error::Invalid("invalid stored replay observation authority".into()))?;
    if authority.schema_version != "rsia.optimization.source.v1"
        || authority.record.id != source_id
        || authority.trace.run_id != source_id
        || authority.record.task_origin != TaskOrigin::TrustedRun
        || authority.record.execution_attestation != ExecutionAttestation::TrustedHost
        || authority.record.purpose != Purpose::Development
        || authority.trace.purpose != Purpose::Development
        || authority.trace.parent_family != authority.record.parent_family
        || authority.trace.source_digest != hash(&authority.record.body)
        || authority
            .record
            .body
            .get(authority.excerpt_start..authority.excerpt_end)
            != Some(authority.trace.excerpt.as_bytes())
    {
        return Err(Error::Invalid(
            "source is legacy, unverified, or changed".into(),
        ));
    }
    identifier(&authority.record.parent_family)?;
    Ok(authority)
}
async fn require_replay_run_v3_in_session(
    session: &mut Session,
    ctx: &Context,
    source: &evo_core::replay::ReplaySourceRef,
    canonical: Option<&[u8]>,
) -> Result<()> {
    let authority = live_replay_run_v3_in_session(session, ctx, &source.source_id).await?;
    if authority.trace.source_digest != source.content_digest
        || canonical.is_some_and(|bytes| bytes != authority.record.body.as_slice())
    {
        return Err(Error::Invalid(
            "source is legacy, unverified, or changed".into(),
        ));
    }
    Ok(())
}

fn replay_v3_expected_edges(world: &ReplayWorldV3) -> BTreeSet<(String, String)> {
    let mut edges: BTreeSet<_> = world
        .manifest
        .source_closure
        .iter()
        .map(|source| ("run".into(), source.source_id.clone()))
        .collect();
    edges.insert((
        "artifact".into(),
        world.manifest.root_source.world_artifact_ref.id.clone(),
    ));
    if let ReplayRootRegistrationEvidenceV1::ManagementOriginal {
        original_request_ref,
        ..
    } = &world.manifest.root_source.registration_evidence
    {
        edges.insert(("artifact".into(), original_request_ref.id.clone()));
    }
    edges
}

async fn validate_replay_v3_s0_in_session(
    session: &mut Session,
    ctx: &Context,
    world: &ReplayWorldV3,
) -> Result<()> {
    let registered: ReplayExplorationWorldWire = replay_exploration_record(
        session,
        ctx,
        "exploration_world_v1",
        &world.manifest.root_source.exploration_world_id,
    )
    .await?;
    registered.validate_registration_shape()?;
    let manifest = &world.manifest;
    if manifest.root_source.namespace != ctx.namespace()
        || manifest.approved_parent_digest != registered.approved_parent_digest
        || manifest.world_context_signature != registered.context_signature
        || manifest.model_digest != registered.model_digest
        || manifest.tools_digest != registered.tools_digest
        || manifest.scorer_digest != registered.grader_digest
        || manifest.initial_baseline_quality_micros != registered.initial_baseline_quality_micros
        || manifest.revoke_watermark != registered.source_watermark
    {
        return Err(replay_root_source_unproven());
    }
    // generation/input_order/guidance/repair/baseline_context are deliberately
    // not guessed from E09 context. The exact new baseline run binds those inputs.
    Ok(())
}

async fn validate_replay_v3_observations_in_session(
    session: &mut Session,
    ctx: &Context,
    world: &ReplayWorldV3,
) -> Result<()> {
    let watermark = session
        .watermark(ctx)
        .await?
        .and_then(|(seq, _)| u64::try_from(seq).ok())
        .ok_or_else(|| Error::Conflict("missing source revoke watermark".into()))?;
    if watermark != world.manifest.revoke_watermark {
        return Err(Error::Conflict("source revoke watermark changed".into()));
    }
    let baseline = replay_baseline_observation_bytes_v2(&world.manifest)?;
    for source in &world.manifest.source_closure {
        let observation;
        let canonical = if source.source_id == world.manifest.baseline_observation_source_id {
            Some(baseline.as_slice())
        } else if let Some(transition) = world
            .transitions
            .iter()
            .find(|transition| transition.observation_source_id == source.source_id)
        {
            observation = replay_observation_bytes_v3(&world.manifest, transition)?;
            Some(observation.as_slice())
        } else {
            None
        };
        require_replay_run_v3_in_session(session, ctx, source, canonical).await?;
    }
    let roots = replay_v3_expected_edges(world);
    let roots: Vec<_> = roots
        .iter()
        .map(|(kind, id)| (kind.as_str(), id.as_str()))
        .collect();
    let closure = session.upstream_closure(ctx, &roots, 10_000).await?;
    if closure
        .iter()
        .any(|node| node.tombstone.is_some() || node.redacted)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

/// All whole-world checks precede this private guard construction. No schema,
/// caller DTO, Value, current engine string or public constructor can mint it.
pub async fn load_replay_world_v3_in_session(
    session: &mut Session,
    ctx: &Context,
    world_id: &str,
    purpose: Purpose,
) -> Result<LoadedReplayWorldV3> {
    ctx.require(&[Role::Admin, Role::Worker, Role::Evaluator])?;
    identifier(world_id)?;
    let (sealed, raw_value) =
        replay_root_existing_world_raw_in_session(session, ctx, world_id).await?;
    if is_redacted(&raw_value) {
        return Err(redacted_record(REPLAY_WORLD_RECORD_KIND, world_id));
    }
    if !sealed {
        return Err(Error::Conflict("replay world is not sealed".into()));
    }
    let world: ReplayWorldV3 = serde_json::from_value(raw_value.clone())
        .map_err(|_| Error::Invalid("sealed world is not ReplayWorldV3".into()))?;
    if world.schema_version != REPLAY_WORLD_SCHEMA_V3 || world.manifest.world_id != world_id {
        return Err(Error::Conflict("replay world row identity differs".into()));
    }
    world.validate_sealed()?;
    if purpose != Purpose::Development || world.manifest.purpose != purpose {
        return Err(Error::Forbidden);
    }
    validate_replay_v3_observations_in_session(session, ctx, &world).await?;
    validate_replay_v3_s0_in_session(session, ctx, &world).await?;
    let current = reconstruct_replay_root_source_in_session(
        session,
        ctx,
        &world.manifest.root_source.exploration_world_id,
        Some(&world.manifest.root_source.registration_evidence),
    )
    .await?;
    if current.source != world.manifest.root_source
        || current.registration_image_utf8.as_slice() != world.registration_image_utf8.as_bytes()
    {
        return Err(replay_root_source_unproven());
    }
    let edges = session
        .dependency_edges(ctx, REPLAY_WORLD_RECORD_KIND, world_id)
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
    if edges != replay_v3_expected_edges(&world) {
        return Err(replay_root_source_unproven());
    }
    // The ONE construction site. Static/full seal + exact baseline/each
    // transition authority bytes + live closure + real root source + S0 + all
    // physical edges have already been checked in this very Session.
    let root_material = ExistingReplayWorldV3RootMaterial {
        replay_world_id: world_id.into(),
        row_body_digest: fingerprint(&raw_value)?,
        source: world.manifest.root_source.clone(),
        registration_image_utf8: world.registration_image_utf8.as_bytes().to_vec(),
    };
    Ok(LoadedReplayWorldV3 {
        world,
        root_material,
    })
}

pub async fn load_replay_world_v3(
    ctx: &Context,
    store: &Store,
    world_id: &str,
    purpose: Purpose,
) -> Result<LoadedReplayWorldV3> {
    ctx.require(&[Role::Admin, Role::Worker, Role::Evaluator])?;
    identifier(world_id)?;
    let mut session = store.session().await?;
    let loaded = load_replay_world_v3_in_session(&mut session, ctx, world_id, purpose).await?;
    session.commit().await?;
    Ok(loaded)
}

fn replay_v3_frozen_manifest(world: &ReplayWorldV3) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(&world.manifest).map_err(|_| Error::Internal)?;
    let object = value.as_object_mut().ok_or(Error::Internal)?;
    for field in ["action_catalog", "source_closure", "prefix_coverage"] {
        object.remove(field).ok_or(Error::Internal)?;
    }
    Ok(value)
}
fn replay_v3_is_exact_prefix<T: Serialize>(old: &[T], next: &[T]) -> Result<bool> {
    if old.len() > next.len() {
        return Ok(false);
    }
    Ok(serde_json::to_vec(old).map_err(|_| Error::Internal)?
        == serde_json::to_vec(&next[..old.len()]).map_err(|_| Error::Internal)?)
}
fn validate_replay_v3_collecting_extension(
    old: &ReplayWorldV3,
    next: &ReplayWorldV3,
) -> Result<()> {
    if old.registration_image_utf8 != next.registration_image_utf8
        || replay_v3_frozen_manifest(old)? != replay_v3_frozen_manifest(next)?
        || !replay_v3_is_exact_prefix(&old.manifest.action_catalog, &next.manifest.action_catalog)?
        || !replay_v3_is_exact_prefix(&old.manifest.source_closure, &next.manifest.source_closure)?
        || !replay_v3_is_exact_prefix(&old.transitions, &next.transitions)?
    {
        return Err(Error::Conflict(
            "replay collecting history was rewritten".into(),
        ));
    }
    for previous in &old.manifest.prefix_coverage {
        let current = next
            .manifest
            .prefix_coverage
            .iter()
            .find(|item| item.context_signature == previous.context_signature)
            .ok_or_else(|| Error::Conflict("replay collecting coverage was removed".into()))?;
        if previous.exhausted && !current.exhausted {
            return Err(Error::Conflict(
                "replay exhausted context was reopened".into(),
            ));
        }
    }
    Ok(())
}

/// Independent Admin writer: re-proves roots and every actual observation in
/// its caller's writing Session before the first write. Never accepts a
/// caller-deserialized proof object or borrows validation from another Session.
pub async fn put_replay_world_v3_draft_in_session(
    session: &mut Session,
    ctx: &Context,
    world: &ReplayWorldV3,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    world.validate_draft()?;
    let existing =
        match replay_root_existing_world_raw_in_session(session, ctx, &world.manifest.world_id)
            .await
        {
            Ok((sealed, value)) => {
                if is_redacted(&value) {
                    return Err(redacted_record(
                        REPLAY_WORLD_RECORD_KIND,
                        &world.manifest.world_id,
                    ));
                }
                if sealed {
                    return Err(Error::Conflict("sealed replay world is immutable".into()));
                }
                let old: ReplayWorldV3 = serde_json::from_value(value).map_err(|_| {
                    Error::Conflict("existing replay world is another version".into())
                })?;
                old.validate_draft()?;
                validate_replay_v3_collecting_extension(&old, world)?;
                let old_edges = session
                    .dependency_edges(ctx, REPLAY_WORLD_RECORD_KIND, &old.manifest.world_id)
                    .await?
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                if old_edges != replay_v3_expected_edges(&old) {
                    return Err(replay_root_source_unproven());
                }
                Some(old)
            }
            Err(Error::NotFound) => None,
            Err(error) => return Err(error),
        };
    let existing_evidence = existing
        .as_ref()
        .map(|old| &old.manifest.root_source.registration_evidence);
    let actual = reconstruct_replay_root_source_in_session(
        session,
        ctx,
        &world.manifest.root_source.exploration_world_id,
        existing_evidence,
    )
    .await?;
    if actual.source != world.manifest.root_source
        || actual.registration_image_utf8.as_slice() != world.registration_image_utf8.as_bytes()
    {
        return Err(replay_root_source_unproven());
    }
    validate_replay_v3_s0_in_session(session, ctx, world).await?;
    validate_replay_v3_observations_in_session(session, ctx, world).await?;
    if let Some(old) = &existing
        && fingerprint(old)? == fingerprint(world)?
    {
        // Exact retry only AFTER live root/source/S0/physical-edge revalidation.
        return Ok(());
    }
    // Every rejecting source/static/identity check above is before this first put.
    session
        .put_world(
            ctx,
            &world.manifest.world_id,
            false,
            &serde_json::to_value(world).map_err(|_| Error::Internal)?,
        )
        .await?;
    for (kind, id) in replay_v3_expected_edges(world) {
        session
            .put_edge(
                ctx,
                REPLAY_WORLD_RECORD_KIND,
                &world.manifest.world_id,
                &kind,
                &id,
            )
            .await?;
    }
    session
        .audit(ctx, "replay.world.v3.collect", &world.manifest.world_id)
        .await
}

/// Sealing only freezes the actual stored collecting history; it cannot append
/// observations, create a sealed world without a draft, or change the root image.
pub async fn seal_replay_world_v3_in_session(
    session: &mut Session,
    ctx: &Context,
    world_id: &str,
) -> Result<ReplayWorldV3> {
    ctx.require(&[Role::Admin])?;
    let (sealed, value) = replay_root_existing_world_raw_in_session(session, ctx, world_id).await?;
    if is_redacted(&value) {
        return Err(redacted_record(REPLAY_WORLD_RECORD_KIND, world_id));
    }
    if sealed {
        // Idempotent only AFTER the complete actual current loader requalifies it.
        return Ok(
            load_replay_world_v3_in_session(session, ctx, world_id, Purpose::Development)
                .await?
                .world,
        );
    }
    let mut world: ReplayWorldV3 = serde_json::from_value(value)
        .map_err(|_| Error::Invalid("draft world is not ReplayWorldV3".into()))?;
    if world.manifest.world_id != world_id {
        return Err(Error::Conflict("replay world row identity differs".into()));
    }
    world.validate_draft()?;
    let actual = reconstruct_replay_root_source_in_session(
        session,
        ctx,
        &world.manifest.root_source.exploration_world_id,
        Some(&world.manifest.root_source.registration_evidence),
    )
    .await?;
    if actual.source != world.manifest.root_source
        || actual.registration_image_utf8.as_slice() != world.registration_image_utf8.as_bytes()
    {
        return Err(replay_root_source_unproven());
    }
    validate_replay_v3_s0_in_session(session, ctx, &world).await?;
    validate_replay_v3_observations_in_session(session, ctx, &world).await?;
    let edges = session
        .dependency_edges(ctx, REPLAY_WORLD_RECORD_KIND, world_id)
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
    if edges != replay_v3_expected_edges(&world) {
        return Err(replay_root_source_unproven());
    }
    world.seal()?;
    session
        .put_world(
            ctx,
            world_id,
            true,
            &serde_json::to_value(&world).map_err(|_| Error::Internal)?,
        )
        .await?;
    session.audit(ctx, "replay.world.v3.seal", world_id).await?;
    Ok(world)
}

/// Admin collecting read returns an ordinary draft DTO, NOT an existing-sealed
/// opaque guard. It proves current inputs so collector updates cannot append to
/// stale, revoked, legacy or rewritten history.
pub async fn load_replay_world_v3_collecting_in_session(
    session: &mut Session,
    ctx: &Context,
    world_id: &str,
) -> Result<ReplayWorldV3> {
    ctx.require(&[Role::Admin])?;
    let (sealed, value) = replay_root_existing_world_raw_in_session(session, ctx, world_id).await?;
    if is_redacted(&value) {
        return Err(redacted_record(REPLAY_WORLD_RECORD_KIND, world_id));
    }
    if sealed {
        return Err(Error::Conflict("sealed replay world is immutable".into()));
    }
    let world: ReplayWorldV3 = serde_json::from_value(value)
        .map_err(|_| Error::Invalid("collecting world is not ReplayWorldV3".into()))?;
    if world.manifest.world_id != world_id {
        return Err(Error::Conflict("replay world row identity differs".into()));
    }
    world.validate_draft()?;
    let actual = reconstruct_replay_root_source_in_session(
        session,
        ctx,
        &world.manifest.root_source.exploration_world_id,
        Some(&world.manifest.root_source.registration_evidence),
    )
    .await?;
    if actual.source != world.manifest.root_source
        || actual.registration_image_utf8.as_slice() != world.registration_image_utf8.as_bytes()
    {
        return Err(replay_root_source_unproven());
    }
    validate_replay_v3_s0_in_session(session, ctx, &world).await?;
    validate_replay_v3_observations_in_session(session, ctx, &world).await?;
    let edges = session
        .dependency_edges(ctx, REPLAY_WORLD_RECORD_KIND, world_id)
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
    if edges != replay_v3_expected_edges(&world) {
        return Err(replay_root_source_unproven());
    }
    Ok(world)
}

/// The supplied action/transition are merely proposals. Every new source digest
/// is read from the actual run record, and the final writer verifies the exact
/// canonical baseline/observation bytes before its first put.
pub async fn collect_replay_transition_v3_in_session(
    session: &mut Session,
    ctx: &Context,
    world_id: &str,
    action: &evo_core::replay::ReplayActionSpecV1,
    transition: &evo_core::replay::ReplayTransitionV2,
) -> Result<ReplayWorldV3> {
    ctx.require(&[Role::Admin])?;
    let mut world = load_replay_world_v3_collecting_in_session(session, ctx, world_id).await?;
    if action.record_seq != transition.record_seq {
        return Err(Error::Conflict(
            "replay observation action identity differs".into(),
        ));
    }
    if let Some(existing) = world
        .transitions
        .iter()
        .find(|item| item.record_seq == transition.record_seq)
    {
        let old_action = world
            .manifest
            .action_catalog
            .iter()
            .find(|item| item.record_seq == action.record_seq)
            .ok_or_else(|| {
                Error::Conflict("existing replay observation lacks its action".into())
            })?;
        if fingerprint(existing)? != fingerprint(transition)?
            || fingerprint(old_action)? != fingerprint(action)?
        {
            return Err(Error::Conflict(
                "replay observation identity was reused".into(),
            ));
        }
        return Ok(world);
    }
    if let Some(existing) = world
        .manifest
        .action_catalog
        .iter()
        .find(|item| item.record_seq == action.record_seq)
    {
        if fingerprint(existing)? != fingerprint(action)? {
            return Err(Error::Conflict("replay action identity was reused".into()));
        }
    } else {
        world.manifest.action_catalog.push(action.clone());
    }
    let mut source_ids = transition.source_ids.clone();
    source_ids.sort();
    if source_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Error::Conflict(
            "duplicate replay observation source".into(),
        ));
    }
    for source_id in source_ids {
        if world
            .manifest
            .source_closure
            .iter()
            .any(|source| source.source_id == source_id)
        {
            continue;
        }
        let authority = live_replay_run_v3_in_session(session, ctx, &source_id).await?;
        world
            .manifest
            .source_closure
            .push(evo_core::replay::ReplaySourceRef {
                source_id,
                content_digest: authority.trace.source_digest,
            });
    }
    world.transitions.push(transition.clone());
    // In particular, do not fill an absent baseline/source/usage with a label,
    // zero, a generated body or the caller's digest.
    put_replay_world_v3_draft_in_session(session, ctx, &world).await?;
    Ok(world)
}

/// New report wire uses its own static record kind; old V1 objects stay intact.
pub const CURRENT_REPLAY_REPORT_RECORD_KIND: &str = "stored_replay_report_v2";

/// Closed version discovery. It returns no raw input or unqualified version
/// string, so an unknown payload cannot echo private data through diagnostics.
pub async fn replay_report_version_in_session(
    session: &mut Session,
    ctx: &Context,
    report_id: &str,
) -> evo_core::replay::ReplayReadResult<Option<evo_core::replay::ReplayReportIdentityVersion>> {
    use evo_core::replay::{ReplayReadIsolation, ReplayReportIdentityVersion};
    ctx.require(&[Role::Worker, Role::Evaluator, Role::Admin])?;
    identifier(report_id)?;
    let row = session
        .object_row(ctx, "artifact", report_id)
        .await?
        .ok_or(Error::NotFound)?;
    let value = replay_root_strict_json(&row.body)?;
    if is_redacted(&value) {
        // Metadata only selects a closed diagnostic name. It does not validate
        // a tombstone, grant a version, or authorize any new consumption.
        let kind = match value
            .get("metadata")
            .and_then(|metadata| metadata.get("record_kind"))
            .and_then(serde_json::Value::as_str)
        {
            Some(REPLAY_REPORT_RECORD_KIND) => REPLAY_REPORT_RECORD_KIND,
            Some(CURRENT_REPLAY_REPORT_RECORD_KIND) => CURRENT_REPLAY_REPORT_RECORD_KIND,
            _ => "replay_report",
        };
        return Err(redacted_record(kind, report_id).into());
    }
    let raw: ReplayStoreEnvelope<serde_json::Value> = match serde_json::from_value(value) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    if raw.schema_version != REPLAY_STORE_ENVELOPE_SCHEMA
        || raw.namespace != ctx.namespace()
        || raw.id != report_id
    {
        return Ok(None);
    }
    let version = match raw
        .payload
        .get("replay_engine_version")
        .and_then(serde_json::Value::as_str)
    {
        Some(evo_core::replay::LEGACY_REPLAY_ENGINE_VERSION_V2) => {
            ReplayReportIdentityVersion::LegacyV2
        }
        Some(evo_core::replay::NEXT_REPLAY_ENGINE_VERSION_V3) => {
            ReplayReportIdentityVersion::NextV3
        }
        Some(_) => return Err(ReplayReadIsolation::UnqualifiedVersionOutsideStaticTable),
        None => return Ok(None),
    };
    let expected_kind = match version {
        ReplayReportIdentityVersion::LegacyV2 => REPLAY_REPORT_RECORD_KIND,
        ReplayReportIdentityVersion::NextV3 => CURRENT_REPLAY_REPORT_RECORD_KIND,
    };
    if raw.record_kind != expected_kind {
        return Ok(None);
    }
    Ok(Some(version))
}

fn current_report_expected_edges(
    report: &evo_core::replay::StoredReplayReportV2,
) -> Result<BTreeSet<(String, String)>> {
    let mut edges = BTreeSet::from([(
        "artifact".into(),
        replay_pool_storage_id(&report.pool_digest)?,
    )]);
    for id in report
        .member_reports
        .iter()
        .map(|m| &m.world_id)
        .chain(report.failed_worlds.iter().map(|m| &m.world_id))
    {
        edges.insert(("replay_world".into(), id.clone()));
    }
    Ok(edges)
}

async fn read_current_replay_envelope(
    session: &mut Session,
    ctx: &Context,
    report_id: &str,
) -> Result<Option<ReplayStoreEnvelope<evo_core::replay::StoredReplayReportV2>>> {
    let Some(raw) = read_envelope::<serde_json::Value>(
        session,
        ctx,
        CURRENT_REPLAY_REPORT_RECORD_KIND,
        report_id,
    )
    .await?
    else {
        return Ok(None);
    };
    let payload = serde_json::from_value(raw.payload)
        .map_err(|_| Error::Invalid("current replay report wire is invalid".into()))?;
    Ok(Some(ReplayStoreEnvelope {
        schema_version: raw.schema_version,
        id: raw.id,
        namespace: raw.namespace,
        record_kind: raw.record_kind,
        payload,
    }))
}

pub async fn load_current_replay_report_in_session(
    session: &mut Session,
    ctx: &Context,
    report_id: &str,
) -> Result<evo_core::replay::StoredReplayReportV2> {
    ctx.require(&[Role::Worker, Role::Evaluator, Role::Admin])?;
    identifier(report_id)?;
    let envelope: ReplayStoreEnvelope<evo_core::replay::StoredReplayReportV2> =
        read_current_replay_envelope(session, ctx, report_id)
            .await?
            .ok_or(Error::NotFound)?;
    validate_envelope(
        ctx,
        report_id,
        CURRENT_REPLAY_REPORT_RECORD_KIND,
        &envelope,
        evo_core::replay::STORED_REPLAY_REPORT_SCHEMA_V2,
    )
    .map_err(|_| Error::Invalid("current replay envelope is invalid".into()))?;
    if envelope.payload.namespace != ctx.namespace() || envelope.payload.report_id != report_id {
        return Err(Error::Invalid(
            "current replay row identity is invalid".into(),
        ));
    }
    // Reject a forged identity/self-digest before following a claimed pool.
    // This static gate grants no live-source or Current authority.
    evo_core::replay::validate_current_v3_report_without_pool(&envelope.payload)
        .map_err(|_| Error::Invalid("current replay static identity is invalid".into()))?;
    // Qualify the actual complete pool in this same supplied Session.
    let pool =
        load_live_replay_pool_in_session(session, ctx, &envelope.payload.pool_digest).await?;
    envelope
        .payload
        .validate_static(&pool.manifest)
        .map_err(|_| Error::Invalid("current replay static identity is invalid".into()))?;
    let actual: BTreeSet<_> = session
        .dependency_edges(ctx, "artifact", report_id)
        .await?
        .into_iter()
        .collect();
    if actual != current_report_expected_edges(&envelope.payload)? {
        return Err(Error::Invalid(
            "current replay dependency edges differ".into(),
        ));
    }
    Ok(envelope.payload)
}
pub async fn load_current_replay_report(
    ctx: &Context,
    store: &Store,
    report_id: &str,
) -> Result<evo_core::replay::StoredReplayReportV2> {
    let mut session = store.session().await?;
    let record = load_current_replay_report_in_session(&mut session, ctx, report_id).await?;
    session.commit().await?;
    Ok(record)
}

/// Storage's independent final source/static guard. The engine separately
/// recomputes semantics in this same Session before invoking this writer.
/// Persisting a DTO never creates a CurrentVerified report read capability.
pub async fn put_current_replay_report_in_session(
    session: &mut Session,
    ctx: &Context,
    report: &evo_core::replay::StoredReplayReportV2,
) -> Result<()> {
    ctx.require(&[Role::Worker, Role::Admin])?;
    if report.namespace != ctx.namespace() {
        return Err(Error::Forbidden);
    }
    let pool = load_live_replay_pool_in_session(session, ctx, &report.pool_digest).await?;
    report.validate_static(&pool.manifest)?;
    let envelope = ReplayStoreEnvelope {
        schema_version: REPLAY_STORE_ENVELOPE_SCHEMA.into(),
        id: report.report_id.clone(),
        namespace: ctx.namespace().into(),
        record_kind: CURRENT_REPLAY_REPORT_RECORD_KIND.into(),
        payload: report.clone(),
    };
    if let Some(existing) = read_current_replay_envelope(session, ctx, &report.report_id).await? {
        validate_envelope(
            ctx,
            &report.report_id,
            CURRENT_REPLAY_REPORT_RECORD_KIND,
            &existing,
            evo_core::replay::STORED_REPLAY_REPORT_SCHEMA_V2,
        )?;
        existing.payload.validate_static(&pool.manifest)?;
        let edges: BTreeSet<_> = session
            .dependency_edges(ctx, "artifact", &report.report_id)
            .await?
            .into_iter()
            .collect();
        if edges != current_report_expected_edges(report)?
            || fingerprint(&existing)? != fingerprint(&envelope)?
        {
            return Err(Error::Conflict(
                "immutable current replay report differs".into(),
            ));
        }
        return Ok(());
    }
    // All live/version/root/namespace/identity checks precede the first write.
    session
        .put(ctx, "artifact", &report.report_id, ctx.actor(), &envelope)
        .await?;
    for (kind, id) in current_report_expected_edges(report)? {
        session
            .put_edge(ctx, "artifact", &report.report_id, &kind, &id)
            .await?;
    }
    session
        .audit(ctx, "replay.report.store", &report.report_id)
        .await?;
    Ok(())
}
pub async fn put_current_replay_report(
    ctx: &Context,
    store: &Store,
    report: &evo_core::replay::StoredReplayReportV2,
) -> Result<()> {
    let mut session = store.session().await?;
    put_current_replay_report_in_session(&mut session, ctx, report).await?;
    session.commit().await
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
