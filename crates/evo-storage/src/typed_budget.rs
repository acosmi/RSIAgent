//! E16 budget snapshots bind raw storage bytes, not business fingerprints or
//! permission to use the imported payload. A future consumer must still validate
//! the actual E16 records, purposes, materials and physical blobs.

use crate::budget::{BudgetArtifact, BudgetCallRecord};
use crate::lifecycle::{
    REVOKE_TOMBSTONE_SCHEMA, RevokeTombstone, TypedObjectRef, parse_strict_json,
};
use crate::{Session, Store, internal, upstream_rows_in_tx};
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier, text};
use serde::{Deserialize, Serialize};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::BTreeSet;

pub const E16_BUDGET_REQUEST_SCHEMA: &str = "rsia.e16.model_budget_request.v1";
pub const E16_BUDGET_REF_SCHEMA: &str = "rsia.budget_call_e16_ref.v1";
pub const E16_SOURCE_UNAVAILABLE: &str = "e16_source_unavailable";
pub const MAX_E16_BUDGET_SOURCES: usize = 202;
const MAX_SOURCE_BYTES: i64 = 4 * 1024 * 1024;
const MAX_UPSTREAM_NODES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct E16BudgetSourceRef {
    pub namespace: String,
    pub kind: String,
    pub id: String,
    pub schema_version: String,
    pub owner_actor: String,
    /// SHA256 of the original SQLite TEXT UTF-8 bytes, without reserialization.
    pub storage_body_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct E16BudgetRequest {
    pub schema_version: String,
    pub namespace: String,
    pub billing_scope: String,
    pub call_id: String,
    pub object_refs: Vec<E16BudgetSourceRef>,
    /// Opaque, exact JSON bytes for the future content consumer. This digest is
    /// the reservation's actual_input_digest; the envelope has its own digest.
    pub input_artifact: BudgetArtifact,
}

impl E16BudgetRequest {
    pub fn artifact(&self) -> Result<BudgetArtifact> {
        if !valid_request(self) {
            return Err(request_invalid());
        }
        BudgetArtifact::from_serializable(E16_BUDGET_REQUEST_SCHEMA, self)
            .map_err(|_| request_invalid())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct E16BudgetCallRef {
    pub id: String,
    pub schema_version: String,
    pub namespace: String,
    pub billing_scope: String,
    pub call_id: String,
    pub request_schema: String,
    pub request_digest: String,
    pub actual_input_digest: String,
    pub object_refs: Vec<E16BudgetSourceRef>,
}

/// Deterministic independent domain; an E16 call never creates a legacy v1 ref.
pub fn e16_budget_ref_id(namespace: &str, billing_scope: &str, call_id: &str) -> Result<String> {
    identifier(namespace)?;
    identifier(billing_scope)?;
    identifier(call_id)?;
    let digest = fingerprint(&(E16_BUDGET_REF_SCHEMA, namespace, billing_scope, call_id))?;
    Ok(format!("budget-e16-ref-{}", &digest[..32]))
}

pub(crate) fn request_invalid() -> Error {
    Error::Invalid("e16_budget_request_invalid".into())
}

pub(crate) fn sources_unavailable() -> Error {
    Error::Conflict("e16_budget_sources_unavailable".into())
}

pub(crate) fn mode_mismatch() -> Error {
    Error::Conflict("e16_budget_mode_mismatch".into())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn e16_schema(schema: &str) -> bool {
    matches!(
        schema,
        "rsia.e16.source_selection.v1" | "rsia.e16.import_source.v1" | "rsia.e16.import_result.v1"
    )
}

fn valid_refs(namespace: &str, refs: &[E16BudgetSourceRef]) -> bool {
    if refs.is_empty() || refs.len() > MAX_E16_BUDGET_SOURCES {
        return false;
    }
    let mut keys = BTreeSet::new();
    for reference in refs {
        if reference.namespace != namespace
            || reference.kind != "artifact"
            || !e16_schema(&reference.schema_version)
            || identifier(&reference.id).is_err()
            || identifier(&reference.owner_actor).is_err()
            || !valid_digest(&reference.storage_body_digest)
            || !keys.insert((&reference.kind, &reference.id))
        {
            return false;
        }
    }
    refs.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_artifact(artifact: &BudgetArtifact) -> bool {
    identifier(&artifact.schema_version).is_ok()
        && valid_digest(&artifact.digest)
        && artifact.body.len() <= MAX_SOURCE_BYTES as usize
        && hash(artifact.body.as_bytes()) == artifact.digest
        && serde_json::from_str::<serde_json::Value>(&artifact.body).is_ok()
}

fn valid_request(request: &E16BudgetRequest) -> bool {
    request.schema_version == E16_BUDGET_REQUEST_SCHEMA
        && identifier(&request.namespace).is_ok()
        && identifier(&request.billing_scope).is_ok()
        && identifier(&request.call_id).is_ok()
        && valid_refs(&request.namespace, &request.object_refs)
        && valid_artifact(&request.input_artifact)
}

pub(crate) fn parse_request(artifact: &BudgetArtifact) -> Option<E16BudgetRequest> {
    if artifact.schema_version != E16_BUDGET_REQUEST_SCHEMA || !valid_artifact(artifact) {
        return None;
    }
    let value = parse_strict_json(&artifact.body).ok()?;
    let request: E16BudgetRequest = serde_json::from_value(value).ok()?;
    valid_request(&request).then_some(request)
}

/// Mode detection only: strict validation happens in the bound-ref readers.
/// A low-level writer that removes both the new request and the independent ref
/// domain while forging a legacy call is outside this detectable contract.
pub(crate) fn request_marks_e16(artifact: &BudgetArtifact) -> bool {
    request_body_marks_e16(&artifact.schema_version, &artifact.body)
}

pub(crate) fn request_body_marks_e16(schema: &str, body: &str) -> bool {
    schema == E16_BUDGET_REQUEST_SCHEMA
        || (schema == "rsia.redacted.v1"
            && serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|value| {
                    value
                        .get("original_schema")
                        .and_then(|s| s.as_str())
                        .map(str::to_owned)
                })
                .as_deref()
                == Some(E16_BUDGET_REQUEST_SCHEMA))
}

#[derive(Deserialize)]
struct SourceHeader {
    id: String,
    namespace: String,
    owner_actor: String,
    schema_version: String,
}

impl Store {
    /// Host/Admin snapshot only. Header identity and raw bytes are checked;
    /// business payload, original import permissions and blobs remain untrusted.
    pub async fn snapshot_e16_budget_sources(
        &self,
        ctx: &Context,
        sources: &[TypedObjectRef],
    ) -> Result<Vec<E16BudgetSourceRef>> {
        let mut session = self.session().await?;
        let references = session.snapshot_e16_budget_sources(ctx, sources).await?;
        session.commit().await?;
        Ok(references)
    }
}

impl Session {
    /// All point reads share this transaction. Keeps the original row owner,
    /// which may differ from the authenticated model Host's actor.
    pub async fn snapshot_e16_budget_sources(
        &mut self,
        ctx: &Context,
        sources: &[TypedObjectRef],
    ) -> Result<Vec<E16BudgetSourceRef>> {
        ctx.require(&[Role::Host, Role::Admin])?;
        let mut keys = BTreeSet::new();
        if sources.is_empty() || sources.len() > MAX_E16_BUDGET_SOURCES {
            return Err(request_invalid());
        }
        for source in sources {
            if source.kind != "artifact"
                || identifier(&source.id).is_err()
                || !keys.insert(source.id.as_str())
            {
                return Err(request_invalid());
            }
        }
        let mut references = Vec::with_capacity(sources.len());
        for id in keys {
            references.push(
                read_source_in_tx(&mut self.tx, ctx.namespace(), id)
                    .await?
                    .ok_or_else(sources_unavailable)?,
            );
        }
        references.sort();
        Ok(references)
    }
}

async fn read_source_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    id: &str,
) -> Result<Option<E16BudgetSourceRef>> {
    // Do not allocate a source larger than this call's processing limit.
    let row = sqlx::query(
        "SELECT namespace,kind,id,owner,
                CASE WHEN length(CAST(body AS BLOB))<=? THEN body ELSE NULL END AS body
         FROM objects WHERE namespace=? AND kind='artifact' AND id=?",
    )
    .bind(MAX_SOURCE_BYTES)
    .bind(namespace)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some(row) = row else { return Ok(None) };
    let body: Option<String> = row.try_get("body").map_err(internal)?;
    let Some(body) = body else { return Ok(None) };
    let owner: String = row.try_get("owner").map_err(internal)?;
    let Ok(value) = parse_strict_json(&body) else {
        return Ok(None);
    };
    let Ok(header) = serde_json::from_value::<SourceHeader>(value) else {
        return Ok(None);
    };
    let row_namespace: String = row.try_get("namespace").map_err(internal)?;
    let row_kind: String = row.try_get("kind").map_err(internal)?;
    let row_id: String = row.try_get("id").map_err(internal)?;
    if header.id != row_id
        || header.namespace != row_namespace
        || header.owner_actor != owner
        || !e16_schema(&header.schema_version)
        || identifier(&owner).is_err()
    {
        return Ok(None);
    }
    Ok(Some(E16BudgetSourceRef {
        namespace: row_namespace,
        kind: row_kind,
        id: row_id,
        schema_version: header.schema_version,
        owner_actor: owner,
        storage_body_digest: hash(body.as_bytes()),
    }))
}

pub(crate) async fn snapshots_live_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    refs: &[E16BudgetSourceRef],
) -> Result<bool> {
    if !valid_refs(namespace, refs) {
        return Ok(false);
    }
    for reference in refs {
        if read_source_in_tx(tx, namespace, &reference.id)
            .await?
            .as_ref()
            != Some(reference)
        {
            return Ok(false);
        }
    }
    let seeds: Vec<(&str, &str)> = refs
        .iter()
        .map(|r| (r.kind.as_str(), r.id.as_str()))
        .collect();
    let Some(nodes) = upstream_rows_in_tx(tx, namespace, &seeds, MAX_UPSTREAM_NODES, true).await?
    else {
        return Ok(false);
    };
    for node in nodes {
        if !node.exists || node.redacted {
            return Ok(false);
        }
        if let Some(body) = node.tombstone {
            let Some(tombstone) = parse_strict_json(&body)
                .ok()
                .and_then(|value| serde_json::from_value::<RevokeTombstone>(value).ok())
            else {
                return Ok(false);
            };
            if tombstone.schema_version != REVOKE_TOMBSTONE_SCHEMA
                || tombstone.id != node.id
                || !matches!(tombstone.source_kind.as_str(), "run" | "artifact")
                || tombstone.watermark_seq == 0
                || !valid_digest(&tombstone.watermark_digest)
                || tombstone.created_at < 0
                || text(&tombstone.reason, "reason", 512).is_err()
                || tombstone.source_kind == node.kind
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum E16CallMode {
    Legacy,
    E16,
    Mismatch,
}

pub(crate) async fn new_ref_exists_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    scope: &str,
    call: &str,
) -> Result<bool> {
    let id = e16_budget_ref_id(namespace, scope, call)?;
    sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM objects WHERE namespace=? AND kind='artifact' AND id=?)",
    )
    .bind(namespace)
    .bind(id)
    .fetch_one(&mut **tx)
    .await
    .map(|n| n != 0)
    .map_err(internal)
}

pub(crate) async fn call_mode_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
) -> Result<E16CallMode> {
    let present =
        new_ref_exists_in_tx(tx, &call.namespace, &call.billing_scope, &call.call_id).await?;
    if call
        .request_artifact
        .as_ref()
        .is_some_and(request_marks_e16)
    {
        Ok(E16CallMode::E16)
    } else if present {
        Ok(E16CallMode::Mismatch)
    } else {
        Ok(E16CallMode::Legacy)
    }
}

fn ref_from_request(
    call: &BudgetCallRecord,
    request: &E16BudgetRequest,
) -> Option<E16BudgetCallRef> {
    if request.namespace != call.namespace
        || request.billing_scope != call.billing_scope
        || request.call_id != call.call_id
        || request.input_artifact.digest != call.actual_input_digest
    {
        return None;
    }
    let artifact = call.request_artifact.as_ref()?;
    Some(E16BudgetCallRef {
        id: e16_budget_ref_id(&call.namespace, &call.billing_scope, &call.call_id).ok()?,
        schema_version: E16_BUDGET_REF_SCHEMA.into(),
        namespace: call.namespace.clone(),
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        request_schema: E16_BUDGET_REQUEST_SCHEMA.into(),
        request_digest: artifact.digest.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        object_refs: request.object_refs.clone(),
    })
}

fn valid_ref(call: &BudgetCallRecord, reference: &E16BudgetCallRef) -> bool {
    reference.schema_version == E16_BUDGET_REF_SCHEMA
        && reference.namespace == call.namespace
        && reference.billing_scope == call.billing_scope
        && reference.call_id == call.call_id
        && e16_budget_ref_id(&call.namespace, &call.billing_scope, &call.call_id)
            .ok()
            .as_ref()
            == Some(&reference.id)
        && reference.request_schema == E16_BUDGET_REQUEST_SCHEMA
        && valid_digest(&reference.request_digest)
        && reference.actual_input_digest == call.actual_input_digest
        && valid_digest(&reference.actual_input_digest)
        && valid_refs(&call.namespace, &reference.object_refs)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetRedaction {
    schema_version: String,
    state: String,
    reason: String,
    original_schema: String,
    original_digest: String,
}

fn redaction_matches(artifact: &BudgetArtifact, reference: &E16BudgetCallRef) -> bool {
    let Some(redaction) = parse_strict_json(&artifact.body)
        .ok()
        .and_then(|v| serde_json::from_value::<BudgetRedaction>(v).ok())
    else {
        return false;
    };
    artifact.schema_version == "rsia.redacted.v1"
        && redaction.schema_version == artifact.schema_version
        && redaction.state == "source_revoked"
        && text(&redaction.reason, "reason", 512).is_ok()
        && redaction.original_schema == E16_BUDGET_REQUEST_SCHEMA
        && redaction.original_digest == reference.request_digest
}

async fn load_ref_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
) -> Result<Option<String>> {
    sqlx::query_scalar("SELECT body FROM objects WHERE namespace=? AND kind='artifact' AND id=?")
        .bind(&call.namespace)
        .bind(e16_budget_ref_id(
            &call.namespace,
            &call.billing_scope,
            &call.call_id,
        )?)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)
}

async fn legacy_ref_exists_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
) -> Result<bool> {
    let digest = fingerprint(&(
        "rsia.budget_call_ref.v1",
        &call.namespace,
        &call.billing_scope,
        &call.call_id,
    ))?;
    sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM objects WHERE namespace=? AND kind='artifact' AND id=?)",
    )
    .bind(&call.namespace)
    .bind(format!("budget-ref-{}", &digest[..32]))
    .fetch_one(&mut **tx)
    .await
    .map(|n| n != 0)
    .map_err(internal)
}

fn parse_ref(call: &BudgetCallRecord, body: &str) -> Option<E16BudgetCallRef> {
    if body.len() > MAX_SOURCE_BYTES as usize {
        return None;
    }
    let reference: E16BudgetCallRef = serde_json::from_value(parse_strict_json(body).ok()?).ok()?;
    valid_ref(call, &reference).then_some(reference)
}

/// No missing-ref fallback at any content gate, including repeated reserve.
pub(crate) async fn call_sources_live_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    call: &BudgetCallRecord,
) -> Result<bool> {
    if legacy_ref_exists_in_tx(tx, call).await? {
        return Ok(false);
    }
    let Some(request) = call.request_artifact.as_ref().and_then(parse_request) else {
        return Ok(false);
    };
    let Some(expected) = ref_from_request(call, &request) else {
        return Ok(false);
    };
    let Some(body) = load_ref_in_tx(tx, call).await? else {
        return Ok(false);
    };
    if parse_ref(call, &body).as_ref() != Some(&expected) {
        return Ok(false);
    };
    snapshots_live_in_tx(tx, &call.namespace, &request.object_refs).await
}

pub(crate) async fn create_ref_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    call: &BudgetCallRecord,
) -> Result<()> {
    let request = call
        .request_artifact
        .as_ref()
        .and_then(parse_request)
        .ok_or_else(sources_unavailable)?;
    let reference = ref_from_request(call, &request).ok_or_else(sources_unavailable)?;
    insert_ref_in_tx(tx, ctx, &reference).await?;
    repair_edges_in_tx(tx, &reference).await?;
    Ok(())
}

async fn insert_ref_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    reference: &E16BudgetCallRef,
) -> Result<u64> {
    let result =
        sqlx::query("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,'artifact',?,?,?)")
            .bind(&reference.namespace)
            .bind(&reference.id)
            .bind(ctx.actor())
            .bind(serde_json::to_string(reference).map_err(internal)?)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
    Ok(result.rows_affected())
}

async fn repair_edges_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    reference: &E16BudgetCallRef,
) -> Result<u64> {
    let mut added = 0;
    for source in &reference.object_refs {
        added+=sqlx::query("INSERT OR IGNORE INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,'artifact',?,'artifact',?)")
            .bind(&reference.namespace).bind(&reference.id).bind(&source.id)
            .execute(&mut **tx).await.map_err(internal)?.rows_affected();
    }
    Ok(added)
}

pub(crate) enum E16History {
    Legacy,
    Unavailable,
    Bound {
        reference: Box<E16BudgetCallRef>,
        index_added: u64,
    },
}

/// Cleanup-only history maintenance. Does not grant dispatch or content use and
/// does not consult a source body that cleanup may already have destroyed.
pub(crate) async fn maintain_history_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    call: &BudgetCallRecord,
) -> Result<E16History> {
    match call_mode_in_tx(tx, call).await? {
        E16CallMode::Legacy => return Ok(E16History::Legacy),
        E16CallMode::Mismatch => return Ok(E16History::Unavailable),
        E16CallMode::E16 => {}
    }
    if call.namespace != ctx.namespace() || legacy_ref_exists_in_tx(tx, call).await? {
        return Ok(E16History::Unavailable);
    }
    let Some(artifact) = call.request_artifact.as_ref() else {
        return Ok(E16History::Unavailable);
    };
    let stored = load_ref_in_tx(tx, call).await?;
    let mut added = 0;
    let reference = if let Some(request) = parse_request(artifact) {
        let Some(expected) = ref_from_request(call, &request) else {
            return Ok(E16History::Unavailable);
        };
        if let Some(body) = stored {
            if parse_ref(call, &body).as_ref() != Some(&expected) {
                return Ok(E16History::Unavailable);
            };
        } else {
            added += insert_ref_in_tx(tx, ctx, &expected).await?;
        }
        expected
    } else {
        let Some(reference) = stored.as_deref().and_then(|body| parse_ref(call, body)) else {
            return Ok(E16History::Unavailable);
        };
        if !redaction_matches(artifact, &reference) {
            return Ok(E16History::Unavailable);
        };
        reference
    };
    added += repair_edges_in_tx(tx, &reference).await?;
    Ok(E16History::Bound {
        reference: Box::new(reference),
        index_added: added,
    })
}

/// A frontier ref is usable as history only when it identifies the actual call
/// and its original request binding. Arbitrary edges cannot target another call.
pub(crate) async fn history_call_for_ref_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &Context,
    node: &TypedObjectRef,
    body: &str,
) -> Result<Option<BudgetCallRecord>> {
    if body.len() > MAX_SOURCE_BYTES as usize {
        return Ok(None);
    }
    let Some(reference) = parse_strict_json(body)
        .ok()
        .and_then(|v| serde_json::from_value::<E16BudgetCallRef>(v).ok())
    else {
        return Ok(None);
    };
    if reference.namespace != ctx.namespace()
        || reference.id != node.id
        || node.kind != "artifact"
        || identifier(&reference.billing_scope).is_err()
        || identifier(&reference.call_id).is_err()
    {
        return Ok(None);
    }
    let Some(call) =
        crate::budget::load_call(tx, &reference.billing_scope, &reference.call_id).await?
    else {
        return Ok(None);
    };
    match maintain_history_in_tx(tx, ctx, &call).await? {
        E16History::Bound {
            reference: bound, ..
        } if bound.as_ref() == &reference => Ok(Some(call)),
        _ => Ok(None),
    }
}
