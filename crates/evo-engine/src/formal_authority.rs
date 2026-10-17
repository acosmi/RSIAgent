//! Versioned formal authority. Wire values never mint the opaque execution or
//! compiler capabilities in this module. Store reads and fences share the
//! caller's Session; external transport runs after that transaction is closed.
use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, ResolvedBundle, SkillPatch,
    SkillSnapshot, SystemSnapshot, compile_bundle, rebase_skill_patch, replay_resolved_bundle,
};
use evo_core::evaluation::CompleteEpisodeCostMeasurements;
use evo_core::evidence::Purpose;
use evo_core::{Context, Error, Result, Role, Strategy, Validate, fingerprint, hash, identifier};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const PROFILE_SCHEMA: &str = "rsia.frozen_compiler_profile.v1";
pub const ROOT_SCHEMA: &str = "rsia.empty_root_authorization.v1";
pub const BINDING_SCHEMA: &str = "rsia.formal_compiler_input_binding.v2";
pub const FORMAL_SCHEMA: &str = "rsia.issued_formal_authority.v2";
pub const HOST_SCHEMA: &str = "rsia.registered_formal_host.v1";
pub const PROVIDER_SCHEMA: &str = "rsia.registered_external_provider.v1";
pub const METER_SCHEMA: &str = "rsia.registered_resource_instrumentation.v1";

#[derive(Serialize, Deserialize)]
enum OperatorRefusalStatus {
    #[serde(rename = "refused")]
    Refused,
}
#[derive(Serialize, Deserialize)]
enum OperatorRefusalSchema {
    #[serde(rename = "rsia.formal_operator_refusal.v1")]
    V1,
}
#[derive(Serialize, Deserialize)]
enum OperatorRefusalCode {
    #[serde(rename = "clock_regressed")]
    ClockRegressed,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypedOperatorRefusal {
    schema_version: OperatorRefusalSchema,
    code: OperatorRefusalCode,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorClockRefusal {
    status: OperatorRefusalStatus,
    error: TypedOperatorRefusal,
}

// Only an authenticated execute result chooses this private refusal packet.
fn operator_response(result: &Result<Value>) -> Result<Value> {
    match result {
        Ok(value) => Ok(serde_json::json!({"status":"complete","value":value})),
        Err(Error::ClockRegressed) => serde_json::to_value(OperatorClockRefusal {
            status: OperatorRefusalStatus::Refused,
            error: TypedOperatorRefusal {
                schema_version: OperatorRefusalSchema::V1,
                code: OperatorRefusalCode::ClockRegressed,
            },
        })
        .map_err(|_| Error::Internal),
        Err(_) => Ok(serde_json::json!({"status":"refused"})),
    }
}

async fn write_operator_response<W: tokio::io::AsyncWrite + Unpin>(
    stream: &mut W,
    result: &Result<Value>,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let response = operator_response(result)?;
    let body = serde_json::to_vec(&response).map_err(|_| Error::Internal)?;
    if body.len() > 1024 * 1024 {
        return Err(Error::Internal);
    }
    stream
        .write_u32(u32::try_from(body.len()).map_err(|_| Error::Internal)?)
        .await
        .map_err(|_| Error::Internal)?;
    stream.write_all(&body).await.map_err(|_| Error::Internal)?;
    Ok(())
}

fn operator_result_after_response(result: Result<Value>, response: Result<()>) -> Result<Value> {
    // An already determined clock refusal must reach the authority loop even
    // when the authenticated peer can no longer receive its response.
    if matches!(&result, Err(Error::ClockRegressed)) {
        return result;
    }
    response?;
    result
}

/// Read only after the caller has authenticated the authority socket's peer.
/// This private wire adds a closed clock refusal, with no request-time source.
#[doc(hidden)]
pub fn decode_formal_operator_response(bytes: &[u8]) -> Result<Value> {
    let response: Value = serde_json::from_slice(bytes).map_err(|_| Error::Internal)?;
    if response["status"] == "complete" {
        return Ok(response);
    }
    // Decode the special packet from bytes: Value would have lost duplicate keys.
    if serde_json::from_slice::<OperatorClockRefusal>(bytes).is_ok() {
        return Err(Error::ClockRegressed);
    }
    Err(Error::Forbidden)
}

fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Invalid("expected lowercase SHA256".into()));
    }
    Ok(())
}
fn append_bounded_body(body: &mut Vec<u8>, chunk: &[u8], limit: usize) -> Result<()> {
    if body
        .len()
        .checked_add(chunk.len())
        .is_none_or(|n| n > limit)
    {
        return Err(Error::Conflict(
            "response body exceeds bound; capture incomplete".into(),
        ));
    }
    body.try_reserve(chunk.len()).map_err(|_| Error::Internal)?;
    body.extend_from_slice(chunk);
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    if !v.is_object() {
        return Err(Error::Conflict(
            "stored authority must be a JSON object; quarantine".into(),
        ));
    }
    serde_json::from_value(v)
        .map_err(|_| Error::Conflict("stored authority schema is unreadable; quarantine".into()))
}
async fn immutable<T: Serialize>(
    s: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    body: &T,
) -> Result<()> {
    if let Some(old) = s.get::<Value>(ctx, kind, id).await? {
        if content_digest(&old)? != content_digest(body)? {
            return Err(Error::Conflict(
                "immutable authority identity reused".into(),
            ));
        }
        return Ok(());
    }
    s.put(ctx, kind, id, ctx.actor(), body).await
}
/// The canonical content digest: exactly `evo_core::canonical_fingerprint`
/// (object keys sorted at every depth), never a second implementation.
pub fn content_digest<T: Serialize>(body: &T) -> Result<String> {
    evo_core::canonical_fingerprint(body)
}

pub(crate) async fn write_content_witness<T: Serialize>(
    s: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    body: &T,
) -> Result<()> {
    let operation = match ctx.role() {
        Role::Admin => "formal.admin.content.v1",
        Role::Host => "formal.host.content.v1",
        Role::Evaluator => "formal.evaluator.content.v1",
        _ => return Err(Error::Forbidden),
    };
    let operation = format!("{operation}.{}", fingerprint(&(ctx.namespace(), kind, id))?);
    let entity = fingerprint(&(ctx.namespace(), kind, id, content_digest(body)?))?;
    s.audit(ctx, &operation, &entity).await
}
pub(crate) async fn attest_content<T: Serialize>(
    s: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    body: &T,
) -> Result<()> {
    write_content_witness(s, ctx, kind, id, body).await?;
    attest_registered_content(s, ctx, kind, id, body).await
}
/// The registered-principal half of content attestation. Formal release
/// mutations run this unchanged guard before the shared content writer.
pub(crate) async fn attest_registered_content<T: Serialize>(
    s: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    body: &T,
) -> Result<()> {
    if ctx.role() == Role::Admin {
        let value = serde_json::to_value(body).map_err(|_| Error::Internal)?;
        if let Some(host) = registered_subject_host(s, ctx, kind, &value).await?
            && s.get::<Value>(ctx, "artifact", &format!("formal-principals-{}", host.id))
                .await?
                .is_some()
        {
            record_operator_action(s, ctx, &host, kind, id, body, Role::Admin).await?;
        }
    }
    Ok(())
}
pub(crate) async fn require_content_witness(
    s: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    body: &Value,
    role: Role,
) -> Result<()> {
    let operation = match role {
        Role::Admin => "formal.admin.content.v1",
        Role::Host => "formal.host.content.v1",
        Role::Evaluator => "formal.evaluator.content.v1",
        _ => return Err(Error::Forbidden),
    };
    let operation = format!("{operation}.{}", fingerprint(&(ctx.namespace(), kind, id))?);
    let entity = fingerprint(&(ctx.namespace(), kind, id, content_digest(body)?))?;
    if !s
        .has_role_audit_witness(ctx, role, &operation, &entity)
        .await?
    {
        return Err(Error::Conflict(
            "missing authenticated content authorization; quarantine".into(),
        ));
    }
    Ok(())
}
async fn attested_read<T: serde::de::DeserializeOwned>(
    s: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    role: Role,
) -> Result<T> {
    let body: Value = s.need(ctx, kind, id).await?;
    require_content_witness(s, ctx, kind, id, &body, role).await?;
    if role == Role::Admin {
        require_registered_body_authority(s, ctx, kind, id, &body).await?;
    }
    decode(body)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedContentRef {
    pub kind: String,
    pub id: String,
    pub schema_version: String,
    pub content_digest: String,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedContentEdge {
    pub from_kind: String,
    pub from_id: String,
    pub to_kind: String,
    pub to_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedContentClosure {
    pub nodes: Vec<TypedContentRef>,
    pub edges: Vec<TypedContentEdge>,
    pub digest: String,
}
/// An explicit wire registry. A namespace prefix is never a version or kind
/// check: the same id stored under another kind has a different identity.
pub(crate) fn compiler_dependency_schema(kind: &str, schema: &str) -> bool {
    match kind {
        "formal_parent" => schema == "rsia.formal_parent_selection.v1",
        "release" => schema == "rsia.persistent_release.v1",
        "pointer" => schema == "rsia.profile_pointer.v1",
        "receipt" => schema == "rsia.host_application.v1",
        "artifact" => matches!(
            schema,
            PROFILE_SCHEMA
                | ROOT_SCHEMA
                | FORMAL_SCHEMA
                | HOST_SCHEMA
                | PROVIDER_SCHEMA
                | METER_SCHEMA
                | PRINCIPAL_ACTION_SCHEMA
                | CONTROL_PRINCIPAL_ACTION_SCHEMA
                | "rsia.formal_principal_set_observation.v1"
                | "rsia.formal_profile_selection.v1"
                | "rsia.persisted_compiler_binding.v1"
                | "rsia.authenticated_bundle.v1"
                | "rsia.actual_formal_provider_capture.v1"
                | "rsia.independent_grade_capture.v1"
                | "rsia.complete_episode_cost.v1"
                | "rsia.trusted_resource_meter.v1"
                | "rsia.host_surface_record.v1"
                | "rsia.run_application.v1"
                | "rsia.host_execution_receipt.v1"
                | "rsia.exploration_artifact_envelope.v1"
                | "rsia.optimization.stage_fact.v1"
                | "rsia.optimization.compiler_use.v1"
                | "rsia.monitoring.environment.v1"
                | "rsia.monitoring.observation.v1"
                | "rsia.monitoring.development_cycle.v2"
                | "rsia.monitoring.development_report_binding.v1"
                | "rsia.monitoring.consolidation_scope.v1"
                | "rsia.monitoring.consolidation_claim.v1"
                | "rsia.monitoring.consolidation_proposal.v1"
                | "rsia.monitoring.consolidation_staging.v1"
                | "rsia.monitoring.consolidation_run.v1"
                | "rsia.monitoring.environment_drift.v1"
                | "rsia.legacy_source_selection.v1"
        ),
        _ => false,
    }
}

fn supported_node_schema(kind: &str, id: &str, body: &Value) -> Result<String> {
    // The original source-selection wire has no schema member. Its only
    // supported legacy adapter is the exact core type under its canonical id.
    if kind == "artifact" && id.starts_with("optgrant-") && body.get("schema_version").is_none() {
        let grant: evo_core::evidence::SourceSelection = decode(body.clone())?;
        grant.validate()?;
        if id != format!("optgrant-{}", fingerprint(&grant)?) {
            return Err(Error::Conflict("source grant identity alias".into()));
        }
        return Ok("rsia.legacy_source_selection.v1".into());
    }
    let schema = body
        .get("schema_version")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Conflict("dependency missing typed schema".into()))?;
    let supported = match kind {
        "run" => schema == "rsia.optimization.source.v1",
        "release" => schema == "rsia.persistent_release.v1",
        "formal_parent" => schema == "rsia.formal_parent_selection.v1",
        "pointer" => schema == "rsia.profile_pointer.v1",
        "receipt" => schema == "rsia.host_application.v1",
        "artifact" => matches!(
            schema,
            PROFILE_SCHEMA
                | ROOT_SCHEMA
                | FORMAL_SCHEMA
                | HOST_SCHEMA
                | PROVIDER_SCHEMA
                | METER_SCHEMA
                | PRINCIPAL_ACTION_SCHEMA
                | CONTROL_PRINCIPAL_ACTION_SCHEMA
                | "rsia.formal_principal_set_observation.v1"
                | "rsia.formal_profile_selection.v1"
                | "rsia.persisted_compiler_binding.v1"
                | "rsia.actual_formal_provider_capture.v1"
                | "rsia.independent_grade_capture.v1"
                | "rsia.complete_episode_cost.v1"
                | "rsia.trusted_resource_meter.v1"
                | "rsia.release_candidate.v1"
                | "rsia.host_surface_record.v1"
                | "rsia.run_application.v1"
                | "rsia.typed_artifact_envelope.v1"
                | "rsia.exploration_artifact_envelope.v1"
                | "rsia.optimization.stage_fact.v1"
                | "rsia.optimization.compiler_use.v1"
                | "rsia.monitoring.environment.v1"
                | "rsia.monitoring.observation.v1"
                | "rsia.monitoring.development_cycle.v2"
                | "rsia.monitoring.development_report_binding.v1"
                | "rsia.monitoring.consolidation_scope.v1"
                | "rsia.monitoring.consolidation_claim.v1"
                | "rsia.monitoring.consolidation_proposal.v1"
                | "rsia.monitoring.consolidation_staging.v1"
                | "rsia.monitoring.consolidation_run.v1"
                | "rsia.monitoring.environment_drift.v1"
                | "rsia.authenticated_bundle.v1"
                | "rsia.host_execution_receipt.v1"
        ),
        _ => false,
    };
    if !supported {
        return Err(Error::Conflict(
            "unsupported dependency kind/schema/version; quarantine".into(),
        ));
    }
    if body
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|stored| stored != id)
        || body
            .get("artifact_id")
            .and_then(Value::as_str)
            .is_some_and(|stored| stored != id)
    {
        return Err(Error::Conflict("typed object id alias".into()));
    }
    match schema {
        "rsia.typed_artifact_envelope.v1" => {
            crate::streaming_evaluator::validate_content_envelope(id, body)?;
        }
        "rsia.exploration_artifact_envelope.v1" => {
            crate::exploration::validate_content_envelope(id, body)?;
        }
        PROFILE_SCHEMA => {
            let _: FrozenCompilerProfile = decode(body.clone())?;
        }
        ROOT_SCHEMA => {
            let _: EmptyRootAuthorization = decode(body.clone())?;
        }
        FORMAL_SCHEMA => {
            let _: IssuedFormalAuthority = decode(body.clone())?;
        }
        HOST_SCHEMA => {
            let _: RegisteredFormalHost = decode(body.clone())?;
        }
        PRINCIPAL_ACTION_SCHEMA | CONTROL_PRINCIPAL_ACTION_SCHEMA => {
            let _: PrincipalAction = decode(body.clone())?;
        }
        "rsia.formal_principal_set_observation.v1" => {
            let _: PrincipalSetObservation = decode(body.clone())?;
        }
        PROVIDER_SCHEMA => {
            let _: RegisteredExternalProvider = decode(body.clone())?;
        }
        METER_SCHEMA => {
            let _: RegisteredResourceInstrumentation = decode(body.clone())?;
        }
        "rsia.formal_profile_selection.v1" => {
            let _: ProfileSelection = decode(body.clone())?;
        }
        "rsia.formal_parent_selection.v1" => {
            let _: ParentSelection = decode(body.clone())?;
        }
        "rsia.persisted_compiler_binding.v1" => {
            let _: crate::release_store::PersistedCompilerBinding = decode(body.clone())?;
        }
        "rsia.actual_formal_provider_capture.v1" => {
            let c: FormalProviderCaptureRecord = decode(body.clone())?;
            validate_provider_response(&c)?;
        }
        "rsia.independent_grade_capture.v1" => {
            let _: IndependentGradeCapture = decode(body.clone())?;
        }
        "rsia.complete_episode_cost.v1" => {
            let c: EpisodeCostRecord = decode(body.clone())?;
            c.measurements.validate_complete()?;
        }
        "rsia.trusted_resource_meter.v1" => {
            let c: ResourceMeasurementRecord = decode(body.clone())?;
            validate_meter_response(&c)?;
        }
        "rsia.release_candidate.v1" => {
            let c: crate::release_store::ReleaseCandidateRecord = decode(body.clone())?;
            if c.bundle.digest != c.bundle_digest
                || c.bundle.profile_id != c.profile_id
                || c.bundle.parent_digest != c.parent_digest
            {
                return Err(Error::Conflict("typed candidate content mismatch".into()));
            }
        }
        "rsia.persistent_release.v1" => {
            let _: crate::release_store::PersistentRelease = decode(body.clone())?;
        }
        "rsia.profile_pointer.v1" => {
            let _: crate::release_store::PersistentProfilePointer = decode(body.clone())?;
        }
        "rsia.host_surface_record.v1" => {
            let _: crate::release_store::HostSurfaceRecord = decode(body.clone())?;
        }
        "rsia.run_application.v1" => {
            let _: crate::release_store::RunApplicationSnapshot = decode(body.clone())?;
        }
        "rsia.host_application.v1" => {
            let _: crate::release_store::HostApplicationRecord = decode(body.clone())?;
        }
        "rsia.host_execution_receipt.v1" => {
            let _: crate::release_store::TrustedHostExecutionReceipt = decode(body.clone())?;
        }
        "rsia.optimization.stage_fact.v1" => {
            let f: crate::optimization::StageFact = decode(body.clone())?;
            f.validate()?;
        }
        "rsia.optimization.compiler_use.v1" => {
            let u: crate::model::CompilerInputUse = decode(body.clone())?;
            if u.artifact_id()? != id {
                return Err(Error::Conflict("compiler use identity alias".into()));
            }
        }
        "rsia.monitoring.environment.v1" => {
            let _: crate::monitoring::EnvironmentIdentityRecord = decode(body.clone())?;
        }
        "rsia.monitoring.observation.v1" => {
            let _: crate::monitoring::DeploymentObservation = decode(body.clone())?;
        }
        "rsia.monitoring.development_cycle.v2" => {
            let _: crate::monitoring::DevelopmentCycleRecord = decode(body.clone())?;
        }
        "rsia.monitoring.development_report_binding.v1" => {
            let _: crate::monitoring::DevelopmentReportBinding = decode(body.clone())?;
        }
        "rsia.monitoring.consolidation_scope.v1" => {
            let _: crate::monitoring::ConsolidationScopeIndex = decode(body.clone())?;
        }
        "rsia.monitoring.consolidation_claim.v1" => {
            let _: crate::monitoring::ConsolidationClaim = decode(body.clone())?;
        }
        "rsia.monitoring.consolidation_proposal.v1" => {
            let _: crate::monitoring::ConsolidationProposal = decode(body.clone())?;
        }
        "rsia.monitoring.consolidation_staging.v1" => {
            let _: crate::monitoring::ConsolidationStagingBinding = decode(body.clone())?;
        }
        "rsia.monitoring.consolidation_run.v1" => {
            let _: crate::monitoring::ConsolidationRunRecord = decode(body.clone())?;
        }
        "rsia.monitoring.environment_drift.v1" => {
            let _: crate::monitoring::EnvironmentDriftRecord = decode(body.clone())?;
        }
        "rsia.authenticated_bundle.v1" => {
            let r: crate::compiler::AuthenticatedBundleRecord = decode(body.clone())?;
            if r.id != format!("formal-bundle-{}", r.bundle.digest) {
                return Err(Error::Conflict("compiled bundle alias".into()));
            }
        }
        "rsia.optimization.source.v1" => {
            let source: crate::evidence::StoredTraceAuthority = decode(body.clone())?;
            source.validate()?;
            if source.record.id != id {
                return Err(Error::Conflict("run identity alias".into()));
            }
        }
        _ => return Err(Error::Conflict("unsupported typed dependency".into())),
    }
    Ok(schema.into())
}
/// Traverses every typed edge, including releases and intermediate controls.
/// Missing objects, unsupported schema, cycles and conflicting identities fail.
/// One edge class takes no part in the cycle check: the GA-18 attachment edge
/// from a bound `evaluation_ticket_v2` to an `execution_receipt_v2` or
/// `independent_grader_receipt_v2` written after issue, judged from both
/// validated envelope bodies. It is still traversed and kept in the closure's
/// edges and digest; any cycle of other edges (a Skill or StageFact dependency
/// cycle, L618/V048) still fails.
pub async fn read_typed_closure(
    s: &mut Session,
    ctx: &Context,
    roots: &[(String, String)],
) -> Result<TypedContentClosure> {
    if roots.len() > 10_000 {
        return Err(Error::Conflict(
            "typed content root count exceeds bound".into(),
        ));
    }
    crate::revocation_gate::ensure_dependencies_live(ctx, s, roots, "formal content").await?;
    let mut pending = roots.to_vec();
    let mut nodes = BTreeMap::new();
    let mut edges = BTreeSet::new();
    let mut total_bytes = 0usize;
    // Validated E05 envelopes at either end of a GA-18 attachment edge.
    let mut tickets = BTreeSet::new();
    let mut attached_facts = BTreeSet::new();
    while let Some((kind, id)) = pending.pop() {
        identifier(&kind)?;
        identifier(&id)?;
        if nodes.contains_key(&(kind.clone(), id.clone())) {
            continue;
        }
        if nodes.len() >= 10_000 {
            return Err(Error::Conflict(
                "typed content closure exceeds bound".into(),
            ));
        }
        let body: Value = s.need(ctx, &kind, &id).await?;
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|_| Error::Internal)?
            .len();
        total_bytes = total_bytes
            .checked_add(body_bytes)
            .ok_or(Error::Forbidden)?;
        if body_bytes > 4 * 1024 * 1024 || total_bytes > 64 * 1024 * 1024 {
            return Err(Error::Conflict(
                "typed dependency exceeds byte bound".into(),
            ));
        }
        let schema = supported_node_schema(&kind, &id, &body)?;
        if schema == "rsia.typed_artifact_envelope.v1" {
            match body["record_kind"].as_str() {
                Some("evaluation_ticket_v2") => {
                    tickets.insert((kind.clone(), id.clone()));
                }
                Some("execution_receipt_v2" | "independent_grader_receipt_v2") => {
                    attached_facts.insert((kind.clone(), id.clone()));
                }
                _ => {}
            }
        }
        let authority_role = match schema.as_str() {
            PROFILE_SCHEMA
            | ROOT_SCHEMA
            | HOST_SCHEMA
            | PROVIDER_SCHEMA
            | METER_SCHEMA
            | "rsia.formal_profile_selection.v1"
            | "rsia.formal_parent_selection.v1"
            | "rsia.persistent_release.v1"
            | "rsia.profile_pointer.v1"
            | "rsia.host_surface_record.v1" => Some(Role::Admin),
            FORMAL_SCHEMA | "rsia.independent_grade_capture.v1" => Some(Role::Evaluator),
            "rsia.actual_formal_provider_capture.v1"
            | "rsia.complete_episode_cost.v1"
            | "rsia.trusted_resource_meter.v1"
            | "rsia.run_application.v1"
            | "rsia.host_application.v1"
            | "rsia.host_execution_receipt.v1" => Some(Role::Host),
            _ => None,
        };
        if let Some(role) = authority_role {
            require_content_witness(s, ctx, &kind, &id, &body, role).await?;
        }
        if authority_role == Some(Role::Admin) {
            require_registered_body_authority(s, ctx, &kind, &id, &body).await?;
        }
        if matches!(
            schema.as_str(),
            PRINCIPAL_ACTION_SCHEMA | CONTROL_PRINCIPAL_ACTION_SCHEMA
        ) {
            require_content_witness(s, ctx, &kind, &id, &body, Role::Evaluator).await?;
        }
        if schema == "rsia.formal_principal_set_observation.v1" {
            require_content_witness(s, ctx, &kind, &id, &body, Role::Admin).await?;
            require_content_witness(s, ctx, &kind, &id, &body, Role::Evaluator).await?;
        }
        if schema == "rsia.typed_artifact_envelope.v1"
            && matches!(
                body["record_kind"].as_str(),
                Some("registered_evaluation_control_v41" | "protected_holdout_v41")
            )
        {
            require_content_witness(s, ctx, &kind, &id, &body, Role::Evaluator).await?;
        }

        let outgoing = s.dependency_edges(ctx, &kind, &id).await?;
        for (to_kind, to_id) in outgoing {
            edges.insert(TypedContentEdge {
                from_kind: kind.clone(),
                from_id: id.clone(),
                to_kind: to_kind.clone(),
                to_id: to_id.clone(),
            });
            if edges.len() > 100_000 {
                return Err(Error::Conflict("typed closure exceeds edge bound".into()));
            }
            pending.push((to_kind, to_id));
        }
        nodes.insert(
            (kind.clone(), id.clone()),
            TypedContentRef {
                kind,
                id,
                schema_version: schema,
                content_digest: fingerprint(&body)?,
            },
        );
    }
    // Kahn's algorithm detects a real directed cycle without recursion. A
    // ticket -> receipt attachment edge is not counted, so only a cycle it
    // closes is accepted; output order and digest below never depend on this.
    let mut degrees: BTreeMap<_, usize> = nodes.keys().cloned().map(|key| (key, 0)).collect();
    let mut children: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
    for edge in &edges {
        let parent = (edge.from_kind.clone(), edge.from_id.clone());
        let child = (edge.to_kind.clone(), edge.to_id.clone());
        if tickets.contains(&parent) && attached_facts.contains(&child) {
            continue;
        }
        *degrees.get_mut(&child).ok_or(Error::NotFound)? += 1;
        children.entry(parent).or_default().push(child);
    }
    let mut ready: Vec<_> = degrees
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(key, _)| key.clone())
        .collect();
    let mut visited = 0;
    while let Some(key) = ready.pop() {
        visited += 1;
        if let Some(outgoing) = children.remove(&key) {
            for child in outgoing {
                let n = degrees.get_mut(&child).ok_or(Error::NotFound)?;
                *n -= 1;
                if *n == 0 {
                    ready.push(child);
                }
            }
        }
    }
    if visited != nodes.len() {
        return Err(Error::Conflict("typed content dependency cycle".into()));
    }
    let nodes: Vec<_> = nodes.into_values().collect();
    let edges: Vec<_> = edges.into_iter().collect();
    let digest = fingerprint(&(&nodes, &edges))?;
    Ok(TypedContentClosure {
        nodes,
        edges,
        digest,
    })
}
pub async fn verify_typed_closure(
    s: &mut Session,
    ctx: &Context,
    roots: &[(String, String)],
    expected: &TypedContentClosure,
) -> Result<()> {
    let actual = read_typed_closure(s, ctx, roots).await?;
    if fingerprint(&actual)? != fingerprint(expected)? {
        return Err(Error::Conflict(
            "typed content changed or undeclared dependency read".into(),
        ));
    }
    Ok(())
}
async fn edges(
    s: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    refs: &[TypedContentRef],
) -> Result<()> {
    for r in refs {
        s.put_edge(ctx, kind, id, &r.kind, &r.id).await?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredFormalHost {
    pub id: String,
    pub schema_version: String,
    pub namespace: String,
    pub host_id: String,
    pub host_version: String,
    pub code_digest: String,
    pub grader_code_digest: String,
    pub host_uid: u32,
    pub grader_uid: u32,
    pub administrator_uid: u32,
    pub host_workspace: String,
    pub grader_workspace: String,
    pub grader_credential_environment_key: String,
    /// Authenticated credential/principal identities, not Context actor aliases.
    pub host_principal: String,
    pub grader_principal: String,
    pub administrator_principal: String,
    pub worker_principal: String,
    pub credential_binding_digest: String,
    pub isolated_workspace_digest: String,
    pub grader_workspace_digest: String,
    pub snapshot: SystemSnapshot,
    pub capabilities: HostCapabilities,
    pub host_surface_digest: String,
    pub adapter_digest: String,
    pub authorized_by: String,
}
impl RegisteredFormalHost {
    fn validate(&self, ctx: &Context) -> Result<()> {
        if self.schema_version != HOST_SCHEMA
            || self.namespace != ctx.namespace()
            || self.snapshot.host_id != self.host_id
            || self.snapshot.host_version != self.host_version
        {
            return Err(Error::Conflict("host registration scope mismatch".into()));
        }
        for v in [
            &self.id,
            &self.host_id,
            &self.host_version,
            &self.host_principal,
            &self.grader_principal,
            &self.administrator_principal,
            &self.worker_principal,
        ] {
            identifier(v)?;
        }
        let principals: BTreeSet<_> = [
            &self.host_principal,
            &self.grader_principal,
            &self.administrator_principal,
            &self.worker_principal,
        ]
        .into_iter()
        .collect();
        if principals.len() != 4 || self.isolated_workspace_digest == self.grader_workspace_digest {
            return Err(Error::Forbidden);
        }
        if self.host_uid == self.grader_uid
            || self.host_uid == self.administrator_uid
            || self.grader_uid == self.administrator_uid
            || self.host_workspace == self.grader_workspace
        {
            return Err(Error::Forbidden);
        }
        digest(&self.grader_code_digest)?;
        for v in [
            &self.code_digest,
            &self.credential_binding_digest,
            &self.isolated_workspace_digest,
            &self.grader_workspace_digest,
            &self.host_surface_digest,
            &self.adapter_digest,
        ] {
            digest(v)?;
        }
        Ok(())
    }
}
pub const PRINCIPALS_SCHEMA: &str = "rsia.registered_formal_principals.v1";
pub const PRINCIPAL_ACTION_SCHEMA: &str = "rsia.observed_principal_action.v1";
/// Actual incoming Evaluator authorization, distinct from the legacy service
/// self-observation used for control registration. Other v1 actions stay v1.
pub const CONTROL_PRINCIPAL_ACTION_SCHEMA: &str = "rsia.observed_control_principal_action.v2";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredPrincipal {
    pub uid: u32,
    pub actor: String,
    pub principal: String,
    /// SHA256 commitment to a distinct private bearer credential. The bearer
    /// itself is accepted only over a kernel-authenticated local connection.
    pub credential_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredPrincipalSet {
    pub id: String,
    pub schema_version: String,
    pub namespace: String,
    pub host_registration_id: String,
    pub version: u64,
    pub host: RegisteredPrincipal,
    pub grader: RegisteredPrincipal,
    pub administrator: RegisteredPrincipal,
    pub worker: RegisteredPrincipal,
}
impl RegisteredPrincipalSet {
    pub fn binding_digest(&self) -> Result<String> {
        fingerprint(self)
    }
    pub fn validate(&self, host: &RegisteredFormalHost) -> Result<()> {
        if self.schema_version != PRINCIPALS_SCHEMA
            || self.version == 0
            || self.namespace != host.namespace
            || self.host_registration_id != host.id
            || self.id != format!("formal-principals-{}", host.id)
            || self.host.uid != host.host_uid
            || self.grader.uid != host.grader_uid
            || self.administrator.uid != host.administrator_uid
            || self.host.principal != host.host_principal
            || self.grader.principal != host.grader_principal
            || self.administrator.principal != host.administrator_principal
            || self.worker.principal != host.worker_principal
            || self.binding_digest()? != host.credential_binding_digest
        {
            return Err(Error::Forbidden);
        }
        let all = [&self.host, &self.grader, &self.administrator, &self.worker];
        for p in all {
            identifier(&p.actor)?;
            identifier(&p.principal)?;
            digest(&p.credential_digest)?;
        }
        if all.iter().map(|p| p.uid).collect::<BTreeSet<_>>().len() != 4
            || all.iter().map(|p| &p.actor).collect::<BTreeSet<_>>().len() != 4
            || all
                .iter()
                .map(|p| &p.principal)
                .collect::<BTreeSet<_>>()
                .len()
                != 4
            || all
                .iter()
                .map(|p| &p.credential_digest)
                .collect::<BTreeSet<_>>()
                .len()
                != 4
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    fn principal(&self, role: Role) -> Result<&RegisteredPrincipal> {
        match role {
            Role::Host => Ok(&self.host),
            Role::Evaluator => Ok(&self.grader),
            Role::Admin => Ok(&self.administrator),
            Role::Worker => Ok(&self.worker),
            _ => Err(Error::Forbidden),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrincipalSetObservation {
    id: String,
    schema_version: String,
    namespace: String,
    host_digest: String,
    principals: RegisteredPrincipalSet,
    grader_pid: u32,
    grader_uid: u32,
    grader_executable_digest: String,
    grader_workspace: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrincipalAction {
    id: String,
    schema_version: String,
    namespace: String,
    host_registration_id: String,
    host_digest: String,
    principal_set_digest: String,
    role: Role,
    actor: String,
    uid: u32,
    principal: String,
    credential_digest: String,
    subject_kind: String,
    subject_id: String,
    subject_content_digest: String,
    operation: String,
    authenticated_request_digest: String,
    peer_uid: u32,
    peer_gid: Option<u32>,
    peer_pid: Option<u32>,
    authority_pid: u32,
    authority_uid: u32,
    authority_executable_digest: String,
}
/// Private fields and no Deserialize/Clone. Only kernel peer authentication
/// plus the independently observed grader process can enter this scope.
struct OperatorCredential {
    namespace: String,
    actor: String,
    role: Role,
    host_digest: String,
    principals: RegisteredPrincipalSet,
    peer: crate::executor::ObservedPeerIdentity,
    authority: crate::executor::ObservedProcessIdentity,
    operation: String,
    request_digest: String,
}
tokio::task_local! { static OPERATOR_CREDENTIAL: OperatorCredential; }

async fn registered_subject_host(
    s: &mut Session,
    ctx: &Context,
    kind: &str,
    body: &Value,
) -> Result<Option<RegisteredFormalHost>> {
    let schema = body
        .get("schema_version")
        .and_then(Value::as_str)
        .unwrap_or("");
    let host_id = match schema {
        HOST_SCHEMA => body.get("id").and_then(Value::as_str).map(str::to_owned),
        PROFILE_SCHEMA | ROOT_SCHEMA | PROVIDER_SCHEMA | METER_SCHEMA => body
            .get("host_registration_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        "rsia.formal_profile_selection.v1" => {
            let id = body["registration_id"].as_str().ok_or(Error::Forbidden)?;
            let profile: Value = s.need(ctx, "artifact", id).await?;
            require_content_witness(s, ctx, "artifact", id, &profile, Role::Admin).await?;
            Some(
                profile["host_registration_id"]
                    .as_str()
                    .ok_or(Error::Forbidden)?
                    .into(),
            )
        }
        "rsia.formal_parent_selection.v1" => {
            let selection: ParentSelection = decode(body.clone())?;
            Some(match selection.authority {
                ParentAuthority::EmptyRoot { id } => {
                    let root: Value = s.need(ctx, "artifact", &id).await?;
                    require_content_witness(s, ctx, "artifact", &id, &root, Role::Admin).await?;
                    root["host_registration_id"]
                        .as_str()
                        .ok_or(Error::Forbidden)?
                        .to_string()
                }
                ParentAuthority::ApprovedRelease { release_id } => {
                    let release: Value = s.need(ctx, "release", &release_id).await?;
                    require_content_witness(s, ctx, "release", &release_id, &release, Role::Admin)
                        .await?;
                    host_id_for_profile(
                        s,
                        ctx,
                        release["profile_id"].as_str().ok_or(Error::Forbidden)?,
                    )
                    .await?
                }
            })
        }
        "rsia.profile_pointer.v1" | "rsia.persistent_release.v1" => Some(
            host_id_for_profile(s, ctx, body["profile_id"].as_str().ok_or(Error::Forbidden)?)
                .await?,
        ),
        _ => None,
    };
    if let Some(id) = host_id {
        let host_body: Value = s.need(ctx, "artifact", &id).await?;
        require_content_witness(s, ctx, "artifact", &id, &host_body, Role::Admin).await?;
        let host: RegisteredFormalHost = decode(host_body)?;
        host.validate(ctx)?;
        Ok(Some(host))
    } else {
        let _ = kind;
        Ok(None)
    }
}
async fn host_id_for_profile(s: &mut Session, ctx: &Context, profile_id: &str) -> Result<String> {
    let selection_id = format!("formal-profile-{profile_id}");
    let selection: Value = s.need(ctx, "artifact", &selection_id).await?;
    require_content_witness(s, ctx, "artifact", &selection_id, &selection, Role::Admin).await?;
    let profile_id = selection["registration_id"]
        .as_str()
        .ok_or(Error::Forbidden)?;
    let profile: Value = s.need(ctx, "artifact", profile_id).await?;
    require_content_witness(s, ctx, "artifact", profile_id, &profile, Role::Admin).await?;
    Ok(profile["host_registration_id"]
        .as_str()
        .ok_or(Error::Forbidden)?
        .into())
}
pub(crate) async fn require_registered_body_authority(
    s: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    body: &Value,
) -> Result<()> {
    if let Some(host) = registered_subject_host(s, ctx, kind, body).await?
        && s.get::<Value>(ctx, "artifact", &format!("formal-principals-{}", host.id))
            .await?
            .is_some()
    {
        require_principal_action(s, ctx, &host, kind, id, body, Role::Admin).await?;
    }
    Ok(())
}
async fn principal_set(
    s: &mut Session,
    ctx: &Context,
    host: &RegisteredFormalHost,
) -> Result<RegisteredPrincipalSet> {
    let id = format!("formal-principals-{}", host.id);
    let body: Value = s.need(ctx, "artifact", &id).await?;
    require_content_witness(s, ctx, "artifact", &id, &body, Role::Admin).await?;
    require_content_witness(s, ctx, "artifact", &id, &body, Role::Evaluator).await?;
    let observation: PrincipalSetObservation = decode(body)?;
    observation.principals.validate(host)?;
    if observation.id != id
        || observation.schema_version != "rsia.formal_principal_set_observation.v1"
        || observation.namespace != ctx.namespace()
        || observation.host_digest != fingerprint(host)?
        || observation.grader_uid != host.grader_uid
        || observation.grader_executable_digest != host.grader_code_digest
        || observation.grader_workspace != host.grader_workspace
        || observation.grader_pid == 0
    {
        return Err(Error::Forbidden);
    }
    Ok(observation.principals)
}
fn process_principal(
    ctx: &Context,
    host: &RegisteredFormalHost,
    set: &RegisteredPrincipalSet,
    role: Role,
) -> Result<crate::executor::ObservedProcessIdentity> {
    let p = set.principal(role)?;
    if ctx.role() != role || ctx.actor() != p.actor {
        return Err(Error::Forbidden);
    }
    let (code, workspace, key) = match role {
        Role::Evaluator => (
            &host.grader_code_digest,
            &host.grader_workspace,
            &host.grader_credential_environment_key,
        ),
        _ => return Err(Error::Forbidden),
    };
    let bearer = std::env::var(key).map_err(|_| Error::Forbidden)?;
    if bearer.len() < 32 || bearer.len() > 4096 || hash(bearer.as_bytes()) != p.credential_digest {
        return Err(Error::Forbidden);
    }
    crate::executor::observe_registered_process(p.uid, code, workspace)
}
async fn observe_host_principal(
    s: &mut Session,
    ctx: &Context,
    host: &RegisteredFormalHost,
    profile: &FrozenCompilerProfile,
) -> Result<crate::executor::ObservedProcessIdentity> {
    let set = principal_set(s, ctx, host).await?;
    let provider: RegisteredExternalProvider = attested_read(
        s,
        ctx,
        "artifact",
        &profile.provider_registration_id,
        Role::Admin,
    )
    .await?;
    let bearer =
        std::env::var(&provider.credential_environment_key).map_err(|_| Error::Forbidden)?;
    if ctx.role() != Role::Host
        || ctx.actor() != set.host.actor
        || bearer.len() < 32
        || bearer.len() > 4096
        || hash(bearer.as_bytes()) != set.host.credential_digest
        || provider.host_registration_id != host.id
    {
        return Err(Error::Forbidden);
    }
    crate::executor::observe_registered_process(
        set.host.uid,
        &host.code_digest,
        &host.host_workspace,
    )
}
async fn record_operator_action<T: Serialize>(
    s: &mut Session,
    ctx: &Context,
    host: &RegisteredFormalHost,
    kind: &str,
    id: &str,
    body: &T,
    expected: Role,
) -> Result<()> {
    let set = principal_set(s, ctx, host).await?;
    let (principal, peer, authority, operation, request_digest) = OPERATOR_CREDENTIAL
        .try_with(|cap| {
            if cap.role != expected
                || ctx.role() != expected
                || cap.namespace != ctx.namespace()
                || cap.actor != ctx.actor()
                || cap.host_digest != fingerprint(host)?
                || cap.principals.binding_digest()? != set.binding_digest()?
            {
                return Err(Error::Forbidden);
            }
            Ok((
                set.principal(expected)?.clone(),
                cap.peer.clone(),
                cap.authority.clone(),
                cap.operation.clone(),
                cap.request_digest.clone(),
            ))
        })
        .map_err(|_| Error::Forbidden)??;
    if peer.uid() != principal.uid || authority.uid != set.grader.uid {
        return Err(Error::Forbidden);
    }
    let control_action = expected == Role::Evaluator && operation == "register_control";
    let action = PrincipalAction {
        id: if control_action {
            control_principal_action_id(kind, id)?
        } else {
            principal_action_id(kind, id, expected)?
        },
        schema_version: if control_action {
            CONTROL_PRINCIPAL_ACTION_SCHEMA
        } else {
            PRINCIPAL_ACTION_SCHEMA
        }
        .into(),
        namespace: ctx.namespace().into(),
        host_registration_id: host.id.clone(),
        host_digest: fingerprint(host)?,
        principal_set_digest: set.binding_digest()?,
        role: expected,
        actor: principal.actor,
        uid: principal.uid,
        principal: principal.principal,
        credential_digest: principal.credential_digest,
        subject_kind: kind.into(),
        subject_id: id.into(),
        subject_content_digest: content_digest(body)?,
        operation,
        authenticated_request_digest: request_digest,
        peer_uid: peer.uid(),
        peer_gid: Some(peer.gid()),
        peer_pid: peer.pid(),
        authority_pid: authority.pid,
        authority_uid: authority.uid,
        authority_executable_digest: authority.executable_digest,
    };
    persist_principal_action(s, ctx, host, &set, action).await
}
fn principal_action_id(kind: &str, id: &str, role: Role) -> Result<String> {
    Ok(format!(
        "principal-action-{}",
        fingerprint(&(kind, id, role))?
    ))
}
fn control_principal_action_id(kind: &str, id: &str) -> Result<String> {
    Ok(format!(
        "principal-action-{}",
        fingerprint(&(kind, id, Role::Evaluator, CONTROL_PRINCIPAL_ACTION_SCHEMA))?
    ))
}

fn control_request_digest(
    control: &crate::streaming_evaluator::RegisteredEvaluationControl,
) -> Result<String> {
    FormalOperatorCommand::RegisterControl {
        control: Box::new(control.clone()),
    }
    .authentication_digest()
}

/// This pure preflight runs before root-budget reads or registration writes.
/// A supplied Context alone cannot turn the grader service into a delegate.
pub(crate) fn authenticate_control_registration(
    ctx: &Context,
    control: &crate::streaming_evaluator::RegisteredEvaluationControl,
) -> Result<()> {
    if control.evidence_scope == crate::streaming_evaluator::EvaluationEvidenceScope::ProgramFixture
    {
        return Ok(());
    }
    let expected_digest = control_request_digest(control)?;
    OPERATOR_CREDENTIAL
        .try_with(|cap| {
            if cap.role != Role::Evaluator
                || ctx.role() != Role::Evaluator
                || cap.actor != ctx.actor()
                || cap.namespace != ctx.namespace()
                || cap.principals.namespace != ctx.namespace()
                || cap.actor != cap.principals.grader.actor
                || cap.peer.uid() != cap.principals.grader.uid
                || cap.operation != "register_control"
                || cap.request_digest != expected_digest
            {
                return Err(Error::Forbidden);
            }
            Ok(())
        })
        .map_err(|_| Error::Forbidden)?
}
async fn persist_principal_action(
    s: &mut Session,
    ctx: &Context,
    host: &RegisteredFormalHost,
    set: &RegisteredPrincipalSet,
    action: PrincipalAction,
) -> Result<()> {
    let grader = Context::new(ctx.namespace(), &set.grader.actor, Role::Evaluator)?;
    // A real grader process/capability produced this witness. Worker/Admin role
    // switching never reaches this write with a deserialized capability.
    process_principal(&grader, host, set, Role::Evaluator)?;
    s.put(&grader, "artifact", &action.id, grader.actor(), &action)
        .await?;
    write_content_witness(s, &grader, "artifact", &action.id, &action).await?;
    s.put_edge(&grader, "artifact", &action.id, "artifact", &set.id)
        .await?;
    s.put_edge(
        &grader,
        &action.subject_kind,
        &action.subject_id,
        "artifact",
        &action.id,
    )
    .await
}
async fn record_grader_action<T: Serialize>(
    s: &mut Session,
    ctx: &Context,
    host: &RegisteredFormalHost,
    kind: &str,
    id: &str,
    body: &T,
    operation: &str,
) -> Result<()> {
    let set = principal_set(s, ctx, host).await?;
    let observed = process_principal(ctx, host, &set, Role::Evaluator)?;
    let p = &set.grader;
    let action = PrincipalAction {
        id: principal_action_id(kind, id, Role::Evaluator)?,
        schema_version: PRINCIPAL_ACTION_SCHEMA.into(),
        namespace: ctx.namespace().into(),
        host_registration_id: host.id.clone(),
        host_digest: fingerprint(host)?,
        principal_set_digest: set.binding_digest()?,
        role: Role::Evaluator,
        actor: p.actor.clone(),
        uid: p.uid,
        principal: p.principal.clone(),
        credential_digest: p.credential_digest.clone(),
        subject_kind: kind.into(),
        subject_id: id.into(),
        subject_content_digest: content_digest(body)?,
        operation: operation.into(),
        authenticated_request_digest: content_digest(body)?,
        peer_uid: p.uid,
        peer_gid: None,
        peer_pid: Some(observed.pid),
        authority_pid: observed.pid,
        authority_uid: observed.uid,
        authority_executable_digest: observed.executable_digest,
    };
    persist_principal_action(s, ctx, host, &set, action).await
}
async fn require_principal_action<T: Serialize>(
    s: &mut Session,
    ctx: &Context,
    host: &RegisteredFormalHost,
    kind: &str,
    id: &str,
    body: &T,
    role: Role,
) -> Result<RegisteredPrincipal> {
    // Closure validation is nested in ticket, report, and release readers. Keep
    // this authorization state machine off their default-size thread stacks.
    Box::pin(async {
        let set = principal_set(s, ctx, host).await?;
        let body_value = serde_json::to_value(body).map_err(|_| Error::Internal)?;
        let control = if role == Role::Evaluator
            && matches!(
                body_value["schema_version"].as_str(),
                Some(
                    crate::streaming_evaluator::RegisteredEvaluationControl::SCHEMA
                        | crate::streaming_evaluator::RegisteredEvaluationControl::SCHEMA_V1
                )
            ) {
            Some(decode::<
                crate::streaming_evaluator::RegisteredEvaluationControl,
            >(body_value)?)
        } else {
            None
        };
        let action_id = if control.is_some() {
            control_principal_action_id(kind, id)?
        } else {
            principal_action_id(kind, id, role)?
        };
        if control.is_some()
            && s.get::<Value>(ctx, "artifact", &action_id).await?.is_none()
            && s.get::<Value>(ctx, "artifact", &principal_action_id(kind, id, role)?)
                .await?
                .is_some()
        {
            return Err(Error::Conflict(
                "legacy control authorization cannot authorize new formal consumption".into(),
            ));
        }
        let action_body: Value = s.need(ctx, "artifact", &action_id).await?;
        require_content_witness(
            s,
            ctx,
            "artifact",
            &action_id,
            &action_body,
            Role::Evaluator,
        )
        .await?;
        let action: PrincipalAction = decode(action_body)?;
        let p = set.principal(role)?;
        if action.id != action_id
            || action.schema_version
                != if control.is_some() {
                    CONTROL_PRINCIPAL_ACTION_SCHEMA
                } else {
                    PRINCIPAL_ACTION_SCHEMA
                }
            || action.namespace != ctx.namespace()
            || action.role != role
            || action.actor != p.actor
            || action.uid != p.uid
            || action.peer_uid != p.uid
            || action.principal != p.principal
            || action.credential_digest != p.credential_digest
            || action.host_registration_id != host.id
            || action.host_digest != fingerprint(host)?
            || action.principal_set_digest != set.binding_digest()?
            || action.subject_kind != kind
            || action.subject_id != id
            || action.subject_content_digest != content_digest(body)?
            || action.authority_uid != set.grader.uid
            || action.authority_executable_digest != host.grader_code_digest
            || action.authority_pid == 0
        {
            return Err(Error::Conflict(
                "actual proposer/approver credential binding is missing or stale".into(),
            ));
        }
        digest(&action.authenticated_request_digest)?;
        if let Some(control) = &control
            && (action.operation != "register_control"
                || action.authenticated_request_digest != control_request_digest(control)?
                || action.peer_gid.is_none())
        {
            return Err(Error::Conflict(
                "control authorization differs from the authenticated request".into(),
            ));
        }
        let valid_operation = match role {
            Role::Worker => action.operation == "stage",
            Role::Admin => matches!(
                action.operation.as_str(),
                "bootstrap"
                    | "register_provider"
                    | "register_meter"
                    | "freeze_profile"
                    | "empty_root"
                    | "approve"
                    | "canary"
                    | "activate"
                    | "rollback"
            ),
            Role::Evaluator => matches!(
                action.operation.as_str(),
                "register_control" | "issue_report"
            ),
            _ => false,
        };
        if !valid_operation {
            return Err(Error::Forbidden);
        }
        Ok(p.clone())
    })
    .await
}
async fn require_registered_admin(
    s: &mut Session,
    ctx: &Context,
    host: &RegisteredFormalHost,
) -> Result<()> {
    if s.get::<Value>(ctx, "artifact", &format!("formal-principals-{}", host.id))
        .await?
        .is_none()
    {
        return Ok(());
    }
    let set = principal_set(s, ctx, host).await?;
    OPERATOR_CREDENTIAL
        .try_with(|cap| {
            if ctx.role() != Role::Admin
                || cap.role != Role::Admin
                || cap.actor != ctx.actor()
                || cap.namespace != ctx.namespace()
                || cap.peer.uid() != set.administrator.uid
                || cap.host_digest != fingerprint(host)?
                || cap.principals.binding_digest()? != set.binding_digest()?
            {
                return Err(Error::Forbidden);
            }
            Ok(())
        })
        .map_err(|_| Error::Forbidden)?
}
pub(crate) async fn verify_candidate_principals_from_binding(
    s: &mut Session,
    ctx: &Context,
    binding: &FormalCompilerInputBinding,
    candidate: &crate::release_store::ReleaseCandidateRecord,
    control: &crate::streaming_evaluator::RegisteredEvaluationControl,
) -> Result<()> {
    let profile: FrozenCompilerProfile = attested_read(
        s,
        ctx,
        "artifact",
        &binding.profile_registration_id,
        Role::Admin,
    )
    .await?;
    verify_candidate_principals(s, ctx, &profile.host_registration_id, candidate, control).await?;
    Ok(())
}
pub(crate) async fn verify_candidate_principals(
    s: &mut Session,
    ctx: &Context,
    host_id: &str,
    candidate: &crate::release_store::ReleaseCandidateRecord,
    control: &crate::streaming_evaluator::RegisteredEvaluationControl,
) -> Result<RegisteredPrincipalSet> {
    let host: RegisteredFormalHost =
        attested_read(s, ctx, "artifact", host_id, Role::Admin).await?;
    let set = principal_set(s, ctx, &host).await?;
    let worker = require_principal_action(
        s,
        ctx,
        &host,
        "artifact",
        &candidate.id,
        candidate,
        Role::Worker,
    )
    .await?;
    let grader = require_principal_action(
        s,
        ctx,
        &host,
        "artifact",
        &crate::streaming_evaluator::storage_id("registered_evaluation_control_v41", &control.id)?,
        control,
        Role::Evaluator,
    )
    .await?;
    if candidate.proposer_actor != worker.actor
        || control.proposer_actor != worker.actor
        || control.evaluator_actor != grader.actor
        || control.approver_actor != set.administrator.actor
        || control.executor_actor != set.host.actor
        || control.evidence_scope
            != crate::streaming_evaluator::EvaluationEvidenceScope::RegisteredExternalHost
    {
        return Err(Error::Forbidden);
    }
    Ok(set)
}
pub(crate) async fn verify_release_principal(
    s: &mut Session,
    ctx: &Context,
    release: &crate::release_store::PersistentRelease,
    candidate: &crate::release_store::ReleaseCandidateRecord,
) -> Result<()> {
    let selection: ProfileSelection = attested_read(
        s,
        ctx,
        "artifact",
        &format!("formal-profile-{}", candidate.profile_id),
        Role::Admin,
    )
    .await?;
    let profile: FrozenCompilerProfile =
        attested_read(s, ctx, "artifact", &selection.registration_id, Role::Admin).await?;
    let host: RegisteredFormalHost = attested_read(
        s,
        ctx,
        "artifact",
        &profile.host_registration_id,
        Role::Admin,
    )
    .await?;
    let admin =
        require_principal_action(s, ctx, &host, "release", &release.id, release, Role::Admin)
            .await?;
    let worker = require_principal_action(
        s,
        ctx,
        &host,
        "artifact",
        &candidate.id,
        candidate,
        Role::Worker,
    )
    .await?;
    if admin.actor != release.approved_by
        || worker.actor != candidate.proposer_actor
        || worker.actor != release.proposer_actor
        || admin.uid == worker.uid
        || admin.principal == worker.principal
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(crate) async fn bind_candidate_principal_from_binding(
    s: &mut Session,
    ctx: &Context,
    binding: &FormalCompilerInputBinding,
    candidate: &crate::release_store::ReleaseCandidateRecord,
) -> Result<()> {
    let profile: FrozenCompilerProfile = attested_read(
        s,
        ctx,
        "artifact",
        &binding.profile_registration_id,
        Role::Admin,
    )
    .await?;
    bind_candidate_principal(s, ctx, &profile.host_registration_id, candidate).await
}
pub(crate) async fn bind_candidate_principal(
    s: &mut Session,
    ctx: &Context,
    host_id: &str,
    candidate: &crate::release_store::ReleaseCandidateRecord,
) -> Result<()> {
    let host: RegisteredFormalHost =
        attested_read(s, ctx, "artifact", host_id, Role::Admin).await?;
    // Explicit old ProgramFixture/EmptyRoot diagnostics remain usable. They
    // cannot execute a registered provider or obtain an issued formal report.
    if s.get::<Value>(ctx, "artifact", &format!("formal-principals-{}", host.id))
        .await?
        .is_none()
    {
        return Ok(());
    }
    if ctx.role() != Role::Worker || ctx.actor() != candidate.proposer_actor {
        return Err(Error::Forbidden);
    }
    let expected = fingerprint(candidate)?;
    OPERATOR_CREDENTIAL
        .try_with(|cap| {
            if cap.operation == "stage" && cap.request_digest == expected {
                Ok(())
            } else {
                Err(Error::Forbidden)
            }
        })
        .map_err(|_| Error::Forbidden)??;
    record_operator_action(
        s,
        ctx,
        &host,
        "artifact",
        &candidate.id,
        candidate,
        Role::Worker,
    )
    .await
}
pub(crate) async fn bind_control_principals(
    s: &mut Session,
    ctx: &Context,
    control: &crate::streaming_evaluator::RegisteredEvaluationControl,
) -> Result<()> {
    if control.evidence_scope == crate::streaming_evaluator::EvaluationEvidenceScope::ProgramFixture
    {
        return Ok(());
    }
    authenticate_control_registration(ctx, control)?;
    // The registered control names a staged candidate indirectly only later;
    // the service's authenticated, observed host is the unique authority here.
    let host_id = OPERATOR_CREDENTIAL
        .try_with(|cap| cap.principals.host_registration_id.clone())
        .map_err(|_| Error::Forbidden)?;
    let host: RegisteredFormalHost =
        attested_read(s, ctx, "artifact", &host_id, Role::Admin).await?;
    let set = principal_set(s, ctx, &host).await?;
    if control.executor_actor != set.host.actor
        || control.evaluator_actor != set.grader.actor
        || control.proposer_actor != set.worker.actor
        || control.approver_actor != set.administrator.actor
    {
        return Err(Error::Forbidden);
    }
    record_operator_action(
        s,
        ctx,
        &host,
        "artifact",
        &crate::streaming_evaluator::storage_id("registered_evaluation_control_v41", &control.id)?,
        control,
        Role::Evaluator,
    )
    .await
}
pub(crate) async fn bind_release_principal(
    s: &mut Session,
    ctx: &Context,
    release: &crate::release_store::PersistentRelease,
) -> Result<()> {
    let candidate: crate::release_store::ReleaseCandidateRecord =
        s.need(ctx, "artifact", &release.candidate_id).await?;
    let inputs = resolve_bundle_in_session(
        s,
        ctx,
        &candidate.bundle,
        &candidate.environment_digest,
        candidate.revoke_watermark,
        Purpose::Development,
    )
    .await?;
    let host: RegisteredFormalHost = attested_read(
        s,
        ctx,
        "artifact",
        &inputs.profile.host_registration_id,
        Role::Admin,
    )
    .await?;
    let worker = require_principal_action(
        s,
        ctx,
        &host,
        "artifact",
        &candidate.id,
        &candidate,
        Role::Worker,
    )
    .await?;
    let set = principal_set(s, ctx, &host).await?;
    if release.proposer_actor != worker.actor
        || release.approved_by != set.administrator.actor
        || release.evaluator_actor != set.grader.actor
    {
        return Err(Error::Forbidden);
    }
    record_operator_action(s, ctx, &host, "release", &release.id, release, Role::Admin).await
}

/// Fixed operator operations. The peer cannot submit a Role, UID, Context or
/// an arbitrary closure to execute inside the private credential scope.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum FormalOperatorCommand {
    Bootstrap,
    RegisterProvider {
        provider: Box<RegisteredExternalProvider>,
    },
    RegisterMeter {
        meter: Box<RegisteredResourceInstrumentation>,
    },
    FreezeProfile {
        profile: Box<FrozenCompilerProfile>,
    },
    EmptyRoot {
        root: Box<EmptyRootAuthorization>,
    },
    Stage {
        request: Box<crate::release_store::StageBundleRequest>,
    },
    RegisterControl {
        control: Box<crate::streaming_evaluator::RegisteredEvaluationControl>,
    },
    Approve {
        candidate_id: String,
        report_id: String,
    },
    Canary {
        release_id: String,
    },
    Activate {
        release_id: String,
        expected_pointer_epoch: u64,
    },
    Rollback {
        profile_id: String,
        target_release_id: String,
        expected_pointer_epoch: u64,
        environment_digest: String,
    },
}
impl<'de> Deserialize<'de> for FormalOperatorCommand {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // serde's internally tagged unit visitor ignores the remaining map.
        // A strict empty struct variant validates Bootstrap's zero-field shape
        // while the public unit variant and its serialized wire stay unchanged.
        #[derive(Deserialize)]
        #[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Bootstrap {},
            RegisterProvider {
                provider: Box<RegisteredExternalProvider>,
            },
            RegisterMeter {
                meter: Box<RegisteredResourceInstrumentation>,
            },
            FreezeProfile {
                profile: Box<FrozenCompilerProfile>,
            },
            EmptyRoot {
                root: Box<EmptyRootAuthorization>,
            },
            Stage {
                request: Box<crate::release_store::StageBundleRequest>,
            },
            RegisterControl {
                control: Box<crate::streaming_evaluator::RegisteredEvaluationControl>,
            },
            Approve {
                candidate_id: String,
                report_id: String,
            },
            Canary {
                release_id: String,
            },
            Activate {
                release_id: String,
                expected_pointer_epoch: u64,
            },
            Rollback {
                profile_id: String,
                target_release_id: String,
                expected_pointer_epoch: u64,
                environment_digest: String,
            },
        }
        // The envelope's object check does not constrain its nested command.
        // Forward the original MapAccess so duplicate keys remain observable.
        struct CommandObjectVisitor;
        impl<'wire> serde::de::Visitor<'wire> for CommandObjectVisitor {
            type Value = Wire;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a formal operator command object")
            }

            fn visit_map<A>(self, map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'wire>,
            {
                Wire::deserialize(serde::de::value::MapAccessDeserializer::new(map))
            }
        }
        Ok(match deserializer.deserialize_map(CommandObjectVisitor)? {
            Wire::Bootstrap {} => Self::Bootstrap,
            Wire::RegisterProvider { provider } => Self::RegisterProvider { provider },
            Wire::RegisterMeter { meter } => Self::RegisterMeter { meter },
            Wire::FreezeProfile { profile } => Self::FreezeProfile { profile },
            Wire::EmptyRoot { root } => Self::EmptyRoot { root },
            Wire::Stage { request } => Self::Stage { request },
            Wire::RegisterControl { control } => Self::RegisterControl { control },
            Wire::Approve {
                candidate_id,
                report_id,
            } => Self::Approve {
                candidate_id,
                report_id,
            },
            Wire::Canary { release_id } => Self::Canary { release_id },
            Wire::Activate {
                release_id,
                expected_pointer_epoch,
            } => Self::Activate {
                release_id,
                expected_pointer_epoch,
            },
            Wire::Rollback {
                profile_id,
                target_release_id,
                expected_pointer_epoch,
                environment_digest,
            } => Self::Rollback {
                profile_id,
                target_release_id,
                expected_pointer_epoch,
                environment_digest,
            },
        })
    }
}

impl FormalOperatorCommand {
    fn role(&self) -> Role {
        match self {
            Self::Stage { .. } => Role::Worker,
            Self::RegisterControl { .. } => Role::Evaluator,
            _ => Role::Admin,
        }
    }
    fn operation(&self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::RegisterProvider { .. } => "register_provider",
            Self::RegisterMeter { .. } => "register_meter",
            Self::FreezeProfile { .. } => "freeze_profile",
            Self::EmptyRoot { .. } => "empty_root",
            Self::Stage { .. } => "stage",
            Self::RegisterControl { .. } => "register_control",
            Self::Approve { .. } => "approve",
            Self::Canary { .. } => "canary",
            Self::Activate { .. } => "activate",
            Self::Rollback { .. } => "rollback",
        }
    }
    fn authentication_digest(&self) -> Result<String> {
        if let Self::Stage { request } = self {
            let candidate = crate::release_store::ReleaseCandidateRecord {
                id: request.candidate_id.clone(),
                schema_version: crate::release_store::RELEASE_CANDIDATE_SCHEMA.into(),
                bundle: request.bundle.clone(),
                bundle_digest: request.bundle.digest.clone(),
                environment_digest: request.environment_digest.clone(),
                profile_id: request.bundle.profile_id.clone(),
                parent_digest: request.bundle.parent_digest.clone(),
                proposer_actor: request.proposer_actor.clone(),
                sources: {
                    let mut refs = request.sources.clone();
                    refs.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
                    refs
                },
                revoke_watermark: request.revoke_watermark,
            };
            return fingerprint(&candidate);
        }
        fingerprint(self)
    }
}
fn authenticate_operator_peer(
    set: &RegisteredPrincipalSet,
    peer: &crate::executor::ObservedPeerIdentity,
    credential: &str,
    command: &FormalOperatorCommand,
) -> Result<RegisteredPrincipal> {
    let p = set.principal(command.role())?;
    if peer.uid() != p.uid
        || credential.len() < 32
        || credential.len() > 4096
        || hash(credential.as_bytes()) != p.credential_digest
    {
        return Err(Error::Forbidden);
    }
    Ok(p.clone())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorEnvelope {
    credential: String,
    command: FormalOperatorCommand,
}
/// Finite local authority service. Construction observes the actual registered
/// grader executable, OS UID, private workspace and private credential.
pub struct PrincipalAuthorityService {
    host: RegisteredFormalHost,
    principals: RegisteredPrincipalSet,
    grader: Context,
    observed: crate::executor::ObservedProcessIdentity,
}
impl PrincipalAuthorityService {
    pub fn new(host: RegisteredFormalHost, principals: RegisteredPrincipalSet) -> Result<Self> {
        let grader = Context::new(&host.namespace, &principals.grader.actor, Role::Evaluator)?;
        host.validate(&grader)?;
        principals.validate(&host)?;
        let observed = process_principal(&grader, &host, &principals, Role::Evaluator)?;
        Ok(Self {
            host,
            principals,
            grader,
            observed,
        })
    }
    pub async fn serve_connection(
        &self,
        store: &Store,
        mut stream: tokio::net::UnixStream,
    ) -> Result<Value> {
        use tokio::io::AsyncReadExt;
        let peer = crate::executor::observe_kernel_peer(&stream)?;
        let size = tokio::time::timeout(std::time::Duration::from_secs(30), stream.read_u32())
            .await
            .map_err(|_| Error::Forbidden)?
            .map_err(|_| Error::Forbidden)? as usize;
        if size == 0 || size > 1024 * 1024 {
            return Err(Error::Forbidden);
        }
        let mut bytes = vec![0; size];
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            stream.read_exact(&mut bytes),
        )
        .await
        .map_err(|_| Error::Forbidden)?
        .map_err(|_| Error::Forbidden)?;
        let mut envelope: OperatorEnvelope = strict_raw_object(&bytes, 1024 * 1024)?;
        let role = envelope.command.role();
        let p = authenticate_operator_peer(
            &self.principals,
            &peer,
            &envelope.credential,
            &envelope.command,
        )?;
        // The secret has no representable field in the persisted observation.
        envelope.credential.clear();
        bytes.fill(0);
        let ctx = Context::new(&self.host.namespace, &p.actor, role)?;
        let cap = OperatorCredential {
            namespace: ctx.namespace().into(),
            actor: ctx.actor().into(),
            role,
            host_digest: fingerprint(&self.host)?,
            principals: self.principals.clone(),
            peer,
            authority: self.observed.clone(),
            operation: envelope.command.operation().into(),
            request_digest: envelope.command.authentication_digest()?,
        };
        let result = OPERATOR_CREDENTIAL
            .scope(cap, self.execute(store, &ctx, envelope.command))
            .await;
        let response = write_operator_response(&mut stream, &result).await;
        operator_result_after_response(result, response)
    }
    async fn execute(
        &self,
        store: &Store,
        ctx: &Context,
        command: FormalOperatorCommand,
    ) -> Result<Value> {
        use crate::release_store::ReleaseStore;
        match command {
            FormalOperatorCommand::Bootstrap => {
                let mut s = store.session().await?;
                if ctx.actor() != self.host.authorized_by
                    || ctx.actor() != self.principals.administrator.actor
                {
                    return Err(Error::Forbidden);
                }
                immutable(&mut s, ctx, "artifact", &self.host.id, &self.host).await?;
                attest_content(&mut s, ctx, "artifact", &self.host.id, &self.host).await?;
                let observation = PrincipalSetObservation {
                    id: self.principals.id.clone(),
                    schema_version: "rsia.formal_principal_set_observation.v1".into(),
                    namespace: ctx.namespace().into(),
                    host_digest: fingerprint(&self.host)?,
                    principals: self.principals.clone(),
                    grader_pid: self.observed.pid,
                    grader_uid: self.observed.uid,
                    grader_executable_digest: self.observed.executable_digest.clone(),
                    grader_workspace: self.observed.workspace.clone(),
                };
                // A changed set uses a new immutable host version. Replaying an
                // old body cannot replace the latest Admin/Evaluator witnesses.
                immutable(&mut s, ctx, "artifact", &observation.id, &observation).await?;
                attest_content(&mut s, ctx, "artifact", &observation.id, &observation).await?;
                attest_content(
                    &mut s,
                    &self.grader,
                    "artifact",
                    &observation.id,
                    &observation,
                )
                .await?;
                attest_content(&mut s, ctx, "artifact", &self.host.id, &self.host).await?;
                // Host -> authenticated action -> principal observation is the
                // authority DAG. The observation already binds the complete
                // immutable host digest; a reverse edge would create a cycle.
                // principal_set validates that binding whenever it is consumed.
                s.commit().await?;
                Ok(
                    serde_json::json!({"registered_host":self.host.id,"principal_version":self.principals.version}),
                )
            }
            FormalOperatorCommand::RegisterProvider { provider } => {
                FormalRegistry::register_provider(ctx, store, *provider).await?;
                Ok(Value::Null)
            }
            FormalOperatorCommand::RegisterMeter { meter } => {
                FormalRegistry::register_resource_instrumentation(ctx, store, *meter).await?;
                Ok(Value::Null)
            }
            FormalOperatorCommand::FreezeProfile { profile } => {
                FormalRegistry::freeze_profile(ctx, store, *profile).await?;
                Ok(Value::Null)
            }
            FormalOperatorCommand::EmptyRoot { root } => Ok(Value::String(
                FormalRegistry::authorize_empty_root(ctx, store, *root).await?,
            )),
            FormalOperatorCommand::Stage { request } => {
                serde_json::to_value(ReleaseStore::stage_bundle(ctx, store, *request).await?)
                    .map_err(|_| Error::Internal)
            }
            FormalOperatorCommand::RegisterControl { control } => Ok(Value::String(
                crate::streaming_evaluator::IndependentEvaluationControl::register_control(
                    ctx, store, *control,
                )
                .await?,
            )),
            FormalOperatorCommand::Approve {
                candidate_id,
                report_id,
            } => serde_json::to_value(
                ReleaseStore::approve_verified(ctx, store, &candidate_id, &report_id).await?,
            )
            .map_err(|_| Error::Internal),
            FormalOperatorCommand::Canary { release_id } => {
                serde_json::to_value(ReleaseStore::advance_canary(ctx, store, &release_id).await?)
                    .map_err(|_| Error::Internal)
            }
            FormalOperatorCommand::Activate {
                release_id,
                expected_pointer_epoch,
            } => serde_json::to_value(
                ReleaseStore::activate(ctx, store, &release_id, expected_pointer_epoch).await?,
            )
            .map_err(|_| Error::Internal),
            FormalOperatorCommand::Rollback {
                profile_id,
                target_release_id,
                expected_pointer_epoch,
                environment_digest,
            } => serde_json::to_value(
                ReleaseStore::rollback(
                    ctx,
                    store,
                    &profile_id,
                    &target_release_id,
                    expected_pointer_epoch,
                    &environment_digest,
                )
                .await?,
            )
            .map_err(|_| Error::Internal),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredExternalProvider {
    pub id: String,
    pub schema_version: String,
    pub namespace: String,
    pub host_registration_id: String,
    pub endpoint: String,
    pub provider_model: String,
    pub model_digest: String,
    pub transport_code_digest: String,
    pub credential_environment_key: String,
    pub credential_principal: String,
    pub currency: String,
    pub pricing_version: String,
    pub input_micros_per_million_tokens: u64,
    pub output_micros_per_million_tokens: u64,
    pub max_output_tokens: u32,
    pub authorized_by: String,
}
impl RegisteredExternalProvider {
    fn validate(&self, ctx: &Context) -> Result<()> {
        if self.schema_version != PROVIDER_SCHEMA
            || self.namespace != ctx.namespace()
            || !self.endpoint.starts_with("https://")
            || self.endpoint.contains('@')
            || self.max_output_tokens == 0
            || self.max_output_tokens > 32768
        {
            return Err(Error::Forbidden);
        }
        for v in [
            &self.id,
            &self.host_registration_id,
            &self.provider_model,
            &self.credential_environment_key,
            &self.credential_principal,
            &self.currency,
            &self.pricing_version,
        ] {
            identifier(v)?;
        }
        digest(&self.model_digest)?;
        digest(&self.transport_code_digest)?;
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenCompilerProfile {
    pub id: String,
    pub schema_version: String,
    pub namespace: String,
    pub profile_id: String,
    pub version: u64,
    pub host_registration_id: String,
    pub provider_registration_id: String,
    pub resource_instrumentation_id: Option<String>,
    pub system_snapshot: SystemSnapshot,
    pub capabilities: HostCapabilities,
    pub environment_digest: String,
    pub baseline_skill: SkillSnapshot,
    pub baseline_strategy: Strategy,
    pub dependencies: Vec<TypedContentRef>,
    pub purpose: Purpose,
    pub application_order: Vec<String>,
    pub authorized_by: String,
}
impl FrozenCompilerProfile {
    pub fn baseline_digest(&self) -> Result<String> {
        fingerprint(self)
    }
    fn validate(&self, ctx: &Context) -> Result<()> {
        if self.schema_version != PROFILE_SCHEMA
            || self.namespace != ctx.namespace()
            || self.system_snapshot.profile_id != self.profile_id
            || self.version == 0
            || self.application_order != ["mandatory_context", "skill", "task"]
        {
            return Err(Error::Conflict(
                "frozen profile identity/order mismatch".into(),
            ));
        }
        identifier(&self.id)?;
        identifier(&self.profile_id)?;
        digest(&self.environment_digest)?;
        self.baseline_skill.validate()?;
        self.baseline_strategy.validate()?;
        if !self
            .capabilities
            .allows(&self.baseline_skill.required_capabilities)
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyRootAuthorization {
    pub id: String,
    pub schema_version: String,
    pub namespace: String,
    pub profile_registration_id: String,
    pub profile_digest: String,
    pub host_registration_id: String,
    pub skill: SkillSnapshot,
    pub strategy: Strategy,
    pub authorized_by: String,
}
pub fn empty_root_strategy() -> Strategy {
    Strategy {
        schema_version: "evo.strategy.v1".into(),
        instruction: "Preserve the unchanged host baseline; no evolution instruction is applied."
            .into(),
        max_candidates: 1,
        max_rounds: 1,
        require_counterexample: true,
    }
}
impl EmptyRootAuthorization {
    pub fn digest(&self) -> Result<String> {
        fingerprint(self)
    }
    fn validate(&self, ctx: &Context, profile: &FrozenCompilerProfile) -> Result<()> {
        if self.schema_version != ROOT_SCHEMA
            || self.namespace != ctx.namespace()
            || self.profile_registration_id != profile.id
            || self.profile_digest != fingerprint(profile)?
            || self.host_registration_id != profile.host_registration_id
            || fingerprint(&self.skill)? != fingerprint(&SkillSnapshot::empty())?
            || fingerprint(&self.strategy)? != fingerprint(&empty_root_strategy())?
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileSelection {
    id: String,
    schema_version: String,
    profile_id: String,
    registration_id: String,
    registration_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum ParentAuthority {
    EmptyRoot { id: String },
    ApprovedRelease { release_id: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentSelection {
    id: String,
    schema_version: String,
    authority: ParentAuthority,
}
async fn parent_selection(s: &mut Session, ctx: &Context, id: &str) -> Result<ParentAuthority> {
    let selection: ParentSelection =
        attested_read(s, ctx, "formal_parent", id, Role::Admin).await?;
    if selection.id != id || selection.schema_version != "rsia.formal_parent_selection.v1" {
        return Err(Error::Forbidden);
    }
    Ok(selection.authority)
}

pub struct FormalRegistry;
impl FormalRegistry {
    pub async fn register_resource_instrumentation(
        ctx: &Context,
        store: &Store,
        meter: RegisteredResourceInstrumentation,
    ) -> Result<()> {
        ctx.require(&[Role::Admin])?;
        meter.validate(ctx)?;
        if meter.authorized_by != ctx.actor() {
            return Err(Error::Forbidden);
        }
        let mut s = store.session().await?;
        let host: RegisteredFormalHost = attested_read(
            &mut s,
            ctx,
            "artifact",
            &meter.host_registration_id,
            Role::Admin,
        )
        .await?;
        host.validate(ctx)?;
        require_registered_admin(&mut s, ctx, &host).await?;
        immutable(&mut s, ctx, "artifact", &meter.id, &meter).await?;
        attest_content(&mut s, ctx, "artifact", &meter.id, &meter).await?;
        s.put_edge(ctx, "artifact", &meter.id, "artifact", &host.id)
            .await?;
        s.commit().await
    }
    pub async fn register_host(
        ctx: &Context,
        store: &Store,
        host: RegisteredFormalHost,
    ) -> Result<()> {
        ctx.require(&[Role::Admin])?;
        host.validate(ctx)?;
        if host.authorized_by != ctx.actor() {
            return Err(Error::Forbidden);
        }
        let mut s = store.session().await?;
        immutable(&mut s, ctx, "artifact", &host.id, &host).await?;
        attest_content(&mut s, ctx, "artifact", &host.id, &host).await?;
        s.audit(ctx, "formal.host.register", &host.id).await?;
        s.commit().await
    }
    pub async fn register_provider(
        ctx: &Context,
        store: &Store,
        provider: RegisteredExternalProvider,
    ) -> Result<()> {
        ctx.require(&[Role::Admin])?;
        provider.validate(ctx)?;
        if provider.authorized_by != ctx.actor() {
            return Err(Error::Forbidden);
        }
        let mut s = store.session().await?;
        let host: RegisteredFormalHost = attested_read(
            &mut s,
            ctx,
            "artifact",
            &provider.host_registration_id,
            Role::Admin,
        )
        .await?;
        host.validate(ctx)?;
        require_registered_admin(&mut s, ctx, &host).await?;
        if provider.credential_principal != host.host_principal {
            return Err(Error::Forbidden);
        }
        immutable(&mut s, ctx, "artifact", &provider.id, &provider).await?;
        attest_content(&mut s, ctx, "artifact", &provider.id, &provider).await?;
        s.put_edge(ctx, "artifact", &provider.id, "artifact", &host.id)
            .await?;
        s.audit(ctx, "formal.provider.register", &provider.id)
            .await?;
        s.commit().await
    }
    pub async fn freeze_profile(
        ctx: &Context,
        store: &Store,
        profile: FrozenCompilerProfile,
    ) -> Result<()> {
        ctx.require(&[Role::Admin])?;
        profile.validate(ctx)?;
        if profile.authorized_by != ctx.actor() {
            return Err(Error::Forbidden);
        }
        let mut s = store.session().await?;
        let host: RegisteredFormalHost = attested_read(
            &mut s,
            ctx,
            "artifact",
            &profile.host_registration_id,
            Role::Admin,
        )
        .await?;
        let provider: RegisteredExternalProvider = attested_read(
            &mut s,
            ctx,
            "artifact",
            &profile.provider_registration_id,
            Role::Admin,
        )
        .await?;
        host.validate(ctx)?;
        require_registered_admin(&mut s, ctx, &host).await?;
        provider.validate(ctx)?;
        if fingerprint(&host.snapshot)? != fingerprint(&profile.system_snapshot)?
            || fingerprint(&host.capabilities)? != fingerprint(&profile.capabilities)?
            || provider.host_registration_id != host.id
        {
            return Err(Error::Conflict(
                "profile differs from registered host".into(),
            ));
        }
        verify_refs(&mut s, ctx, &profile.dependencies).await?;
        immutable(&mut s, ctx, "artifact", &profile.id, &profile).await?;
        attest_content(&mut s, ctx, "artifact", &profile.id, &profile).await?;
        edges(&mut s, ctx, "artifact", &profile.id, &profile.dependencies).await?;
        for id in [&host.id, &provider.id] {
            s.put_edge(ctx, "artifact", &profile.id, "artifact", id)
                .await?;
        }
        if let Some(id) = &profile.resource_instrumentation_id {
            let meter: RegisteredResourceInstrumentation =
                attested_read(&mut s, ctx, "artifact", id, Role::Admin).await?;
            meter.validate(ctx)?;
            if meter.host_registration_id != host.id {
                return Err(Error::Forbidden);
            }
            s.put_edge(ctx, "artifact", &profile.id, "artifact", id)
                .await?;
        }
        let id = format!("formal-profile-{}", profile.profile_id);
        let pointer = ProfileSelection {
            id: id.clone(),
            schema_version: "rsia.formal_profile_selection.v1".into(),
            profile_id: profile.profile_id.clone(),
            registration_id: profile.id.clone(),
            registration_digest: fingerprint(&profile)?,
        };
        s.put(ctx, "artifact", &id, ctx.actor(), &pointer).await?;
        attest_content(&mut s, ctx, "artifact", &id, &pointer).await?;
        s.audit(ctx, "formal.profile.freeze", &profile.id).await?;
        s.commit().await
    }
    pub async fn authorize_empty_root(
        ctx: &Context,
        store: &Store,
        root: EmptyRootAuthorization,
    ) -> Result<String> {
        ctx.require(&[Role::Admin])?;
        let mut s = store.session().await?;
        let profile: FrozenCompilerProfile = attested_read(
            &mut s,
            ctx,
            "artifact",
            &root.profile_registration_id,
            Role::Admin,
        )
        .await?;
        profile.validate(ctx)?;
        root.validate(ctx, &profile)?;
        if root.authorized_by != ctx.actor() {
            return Err(Error::Forbidden);
        }
        let host: RegisteredFormalHost = attested_read(
            &mut s,
            ctx,
            "artifact",
            &root.host_registration_id,
            Role::Admin,
        )
        .await?;
        host.validate(ctx)?;
        require_registered_admin(&mut s, ctx, &host).await?;
        immutable(&mut s, ctx, "artifact", &root.id, &root).await?;
        attest_content(&mut s, ctx, "artifact", &root.id, &root).await?;
        s.put_edge(ctx, "artifact", &root.id, "artifact", &profile.id)
            .await?;
        let identity = root.digest()?;
        let selection = ParentSelection {
            id: identity.clone(),
            schema_version: "rsia.formal_parent_selection.v1".into(),
            authority: ParentAuthority::EmptyRoot {
                id: root.id.clone(),
            },
        };
        immutable(&mut s, ctx, "formal_parent", &identity, &selection).await?;
        attest_content(&mut s, ctx, "formal_parent", &identity, &selection).await?;
        s.put_edge(ctx, "formal_parent", &identity, "artifact", &root.id)
            .await?;
        s.audit(ctx, "formal.empty_root.authorize", &root.id)
            .await?;
        s.commit().await?;
        Ok(identity)
    }
}
async fn verify_refs(s: &mut Session, ctx: &Context, refs: &[TypedContentRef]) -> Result<()> {
    let mut keys = BTreeSet::new();
    for r in refs {
        if !keys.insert((&r.kind, &r.id)) {
            return Err(Error::Conflict("duplicate typed input".into()));
        }
        let body: Value = s.need(ctx, &r.kind, &r.id).await?;
        if supported_node_schema(&r.kind, &r.id, &body)? != r.schema_version
            || fingerprint(&body)? != r.content_digest
        {
            return Err(Error::Conflict(
                "typed input content/schema mismatch".into(),
            ));
        }
    }
    read_typed_closure(
        s,
        ctx,
        &refs
            .iter()
            .map(|r| (r.kind.clone(), r.id.clone()))
            .collect::<Vec<_>>(),
    )
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerInputRequest {
    pub profile: Profile,
    /// Search parent is distinct from the approved control-plane P.
    pub search_parent: SkillSnapshot,
    pub baseline: SkillSnapshot,
    /// Generation strategy of the search parent, not authority for approved P.
    pub search_parent_strategy: Strategy,
    pub expected_parent_strategy_digest: Option<String>,
    pub baseline_strategy: Strategy,
    pub capabilities: HostCapabilities,
    pub environment_digest: String,
    pub purpose: Purpose,
    pub revoke_watermark: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalCompilerInputBinding {
    pub schema_version: String,
    pub namespace: String,
    pub profile_registration_id: String,
    pub profile_digest: String,
    pub parent_digest: String,
    pub baseline_digest: String,
    pub parent_skill_digest: String,
    pub parent_strategy_digest: String,
    pub search_parent_skill_digest: String,
    pub search_parent_strategy_digest: String,
    pub baseline_skill_digest: String,
    pub baseline_strategy_digest: String,
    pub environment_digest: String,
    pub capabilities_digest: String,
    pub purpose: Purpose,
    pub application_order: Vec<String>,
    pub watermark: Option<(i64, String)>,
    pub live_profile_pointer_digest: Option<String>,
    pub closure: TypedContentClosure,
    pub binding_digest: String,
}
/// No Deserialize, no public fields, no caller/model constructor.
#[derive(Debug, Clone)]
pub struct VerifiedCompilerInputs {
    profile: FrozenCompilerProfile,
    parent_skill: SkillSnapshot,
    parent_strategy: Strategy,
    search_parent_strategy: Strategy,
    binding: FormalCompilerInputBinding,
}
impl VerifiedCompilerInputs {
    pub fn binding(&self) -> &FormalCompilerInputBinding {
        &self.binding
    }
    pub fn parent_skill(&self) -> &SkillSnapshot {
        &self.parent_skill
    }
    pub fn request_for_search_parent(&self, search_parent: SkillSnapshot) -> CompilerInputRequest {
        CompilerInputRequest {
            profile: Profile {
                id: self.profile.profile_id.clone(),
                evolution_enabled: true,
                parent_digest: self.binding.parent_digest.clone(),
                baseline_digest: self.binding.baseline_digest.clone(),
            },
            search_parent,
            baseline: self.profile.baseline_skill.clone(),
            search_parent_strategy: self.search_parent_strategy.clone(),
            expected_parent_strategy_digest: Some(self.binding.parent_strategy_digest.clone()),
            baseline_strategy: self.profile.baseline_strategy.clone(),
            capabilities: self.profile.capabilities.clone(),
            environment_digest: self.profile.environment_digest.clone(),
            purpose: self.profile.purpose,
            revoke_watermark: self
                .binding
                .watermark
                .as_ref()
                .map(|w| w.0 as u64)
                .unwrap_or(0),
        }
    }
    pub fn compile(
        &self,
        skill_patch: &SkillPatch,
        improver_patch: &ImproverPatch,
    ) -> Result<ResolvedBundle> {
        compile_bundle(CompileParts {
            profile: &Profile {
                id: self.profile.profile_id.clone(),
                evolution_enabled: true,
                parent_digest: self.binding.parent_digest.clone(),
                baseline_digest: self.binding.baseline_digest.clone(),
            },
            parent: &self.parent_skill,
            baseline: &self.profile.baseline_skill,
            parent_strategy: &self.parent_strategy,
            baseline_strategy: &self.profile.baseline_strategy,
            skill_patch,
            improver_patch,
            caps: &self.profile.capabilities,
            revoked: &BTreeSet::new(),
        })
    }
    pub fn compile_search_edit(
        &self,
        desired: &SkillSnapshot,
        improver_patch: &ImproverPatch,
    ) -> Result<ResolvedBundle> {
        let profile = Profile {
            id: self.profile.profile_id.clone(),
            evolution_enabled: true,
            parent_digest: self.binding.parent_digest.clone(),
            baseline_digest: self.binding.baseline_digest.clone(),
        };
        let (strategy, _) = evo_core::contract::compile_improver(
            &profile,
            &self.search_parent_strategy,
            &self.profile.baseline_strategy,
            improver_patch,
        )?;
        self.compile(
            &rebase_skill_patch(&self.parent_skill, desired)?,
            &ImproverPatch {
                instruction: evo_core::contract::PatchOp::Set {
                    value: strategy.instruction,
                },
                max_candidates: evo_core::contract::PatchOp::Set {
                    value: strategy.max_candidates,
                },
                max_rounds: evo_core::contract::PatchOp::Set {
                    value: strategy.max_rounds,
                },
            },
        )
    }
    pub fn replay(&self, bundle: &ResolvedBundle) -> Result<()> {
        if bundle.profile_id != self.profile.profile_id
            || bundle.parent_digest != self.binding.parent_digest
            || bundle.baseline_digest != self.binding.baseline_digest
        {
            return Err(Error::Conflict("bundle P/B identity mismatch".into()));
        }
        replay_resolved_bundle(
            bundle,
            &self.parent_skill,
            &self.profile.baseline_skill,
            &self.parent_strategy,
            &self.profile.baseline_strategy,
            &self.profile.capabilities,
            &BTreeSet::new(),
        )
    }
}
pub async fn resolve_compiler_inputs(
    s: &mut Session,
    ctx: &Context,
    request: &CompilerInputRequest,
) -> Result<VerifiedCompilerInputs> {
    ctx.require(&[Role::Worker, Role::Host, Role::Admin, Role::Evaluator])?;
    request.search_parent.validate()?;
    request.baseline.validate()?;
    request.search_parent_strategy.validate()?;
    request.baseline_strategy.validate()?;
    let selection: ProfileSelection = attested_read(
        s,
        ctx,
        "artifact",
        &format!("formal-profile-{}", request.profile.id),
        Role::Admin,
    )
    .await?;
    let profile: FrozenCompilerProfile =
        attested_read(s, ctx, "artifact", &selection.registration_id, Role::Admin).await?;
    profile.validate(ctx)?;
    let watermark = s.watermark(ctx).await?;
    if watermark.is_none() && request.revoke_watermark != 0 {
        return Err(Error::Conflict("missing source revoke watermark".into()));
    }
    let sequence = watermark
        .as_ref()
        .map(|w| u64::try_from(w.0).map_err(|_| Error::Conflict("unknown watermark".into())))
        .transpose()?
        .unwrap_or(0);
    if selection.schema_version != "rsia.formal_profile_selection.v1"
        || selection.profile_id != request.profile.id
        || selection.registration_digest != fingerprint(&profile)?
        || profile.environment_digest != request.environment_digest
        || profile.purpose != request.purpose
        || request.revoke_watermark != sequence
        || fingerprint(&profile.baseline_skill)? != fingerprint(&request.baseline)?
        || fingerprint(&profile.baseline_strategy)? != fingerprint(&request.baseline_strategy)?
        || fingerprint(&profile.capabilities)? != fingerprint(&request.capabilities)?
        || profile.baseline_digest()? != request.profile.baseline_digest
    {
        return Err(Error::Conflict(
            "full P/B profile/environment/version/capability binding mismatch".into(),
        ));
    }
    let pointer_id = format!("profile-pointer-{}", request.profile.id);
    let live_profile_pointer_digest = match s.get::<Value>(ctx, "pointer", &pointer_id).await? {
        Some(body) => {
            require_content_witness(s, ctx, "pointer", &pointer_id, &body, Role::Admin).await?;
            supported_node_schema("pointer", &pointer_id, &body)?;
            require_registered_body_authority(s, ctx, "pointer", &pointer_id, &body).await?;
            Some(content_digest(&body)?)
        }
        None => None,
    };
    let authority = parent_selection(s, ctx, &request.profile.parent_digest).await?;
    let (parent_skill, parent_strategy, parent_root) = match authority {
        ParentAuthority::EmptyRoot { id } => {
            let root: EmptyRootAuthorization =
                attested_read(s, ctx, "artifact", &id, Role::Admin).await?;
            root.validate(ctx, &profile)?;
            if root.digest()? != request.profile.parent_digest {
                return Err(Error::Conflict("empty root digest mismatch".into()));
            }
            (root.skill, root.strategy, ("artifact".into(), id))
        }
        ParentAuthority::ApprovedRelease { release_id } => {
            let release: crate::release_store::PersistentRelease =
                attested_read(s, ctx, "release", &release_id, Role::Admin).await?;
            if !matches!(
                release.state,
                crate::release_store::PersistentReleaseState::Approved
                    | crate::release_store::PersistentReleaseState::Canary
                    | crate::release_store::PersistentReleaseState::Active
                    | crate::release_store::PersistentReleaseState::Superseded
            ) {
                return Err(Error::Forbidden);
            }
            let candidate: crate::release_store::ReleaseCandidateRecord =
                s.need(ctx, "artifact", &release.candidate_id).await?;
            verify_release_principal(s, ctx, &release, &candidate).await?;
            verify_issued_authority(s, ctx, &release.report_id, &candidate).await?;
            if release.profile_id != profile.profile_id
                || release.environment_digest != profile.environment_digest
                || release.bundle_digest != candidate.bundle.digest
                || release.report_digest.is_empty()
                || candidate.bundle.digest != request.profile.parent_digest
            {
                return Err(Error::Conflict("approved P lineage mismatch".into()));
            }
            (
                candidate.bundle.skill,
                candidate.bundle.improver,
                ("release".into(), release.id),
            )
        }
    };
    let parent_strategy_digest = fingerprint(&parent_strategy)?;
    if request
        .expected_parent_strategy_digest
        .as_ref()
        .is_some_and(|expected| expected != &parent_strategy_digest)
    {
        return Err(Error::Conflict("P strategy body substitution".into()));
    }
    let mut roots = vec![
        parent_root,
        ("artifact".into(), profile.id.clone()),
        ("artifact".into(), selection.id),
        (
            "formal_parent".into(),
            request.profile.parent_digest.clone(),
        ),
    ];
    // Both original P and B dependencies are resolved before any Reset/Set.
    for dep in parent_skill
        .dependencies
        .iter()
        .chain(&profile.baseline_skill.dependencies)
        .chain(&request.search_parent.dependencies)
    {
        let run = s.get::<Value>(ctx, "run", dep).await?.is_some();
        let artifact = s.get::<Value>(ctx, "artifact", dep).await?.is_some();
        if run == artifact {
            return Err(Error::Conflict(
                "missing or ambiguous untyped Skill dependency".into(),
            ));
        }
        roots.push((if run { "run" } else { "artifact" }.into(), dep.clone()));
    }
    let closure = read_typed_closure(s, ctx, &roots).await?;
    let mut binding = FormalCompilerInputBinding {
        schema_version: BINDING_SCHEMA.into(),
        namespace: ctx.namespace().into(),
        profile_registration_id: profile.id.clone(),
        profile_digest: fingerprint(&profile)?,
        parent_digest: request.profile.parent_digest.clone(),
        baseline_digest: request.profile.baseline_digest.clone(),
        parent_skill_digest: fingerprint(&parent_skill)?,
        parent_strategy_digest,
        search_parent_skill_digest: fingerprint(&request.search_parent)?,
        search_parent_strategy_digest: fingerprint(&request.search_parent_strategy)?,
        baseline_skill_digest: fingerprint(&profile.baseline_skill)?,
        baseline_strategy_digest: fingerprint(&profile.baseline_strategy)?,
        environment_digest: profile.environment_digest.clone(),
        capabilities_digest: fingerprint(&profile.capabilities)?,
        purpose: request.purpose,
        application_order: profile.application_order.clone(),
        watermark,
        live_profile_pointer_digest,
        closure,
        binding_digest: String::new(),
    };
    binding.binding_digest = fingerprint(&binding)?;
    Ok(VerifiedCompilerInputs {
        profile,
        parent_skill,
        parent_strategy,
        search_parent_strategy: request.search_parent_strategy.clone(),
        binding,
    })
}
/// Stage can authenticate an old pure CompileParts result without changing its wire.
pub async fn resolve_bundle_in_session(
    s: &mut Session,
    ctx: &Context,
    bundle: &ResolvedBundle,
    environment_digest: &str,
    watermark: u64,
    purpose: Purpose,
) -> Result<VerifiedCompilerInputs> {
    let selection: ProfileSelection = attested_read(
        s,
        ctx,
        "artifact",
        &format!("formal-profile-{}", bundle.profile_id),
        Role::Admin,
    )
    .await?;
    let profile: FrozenCompilerProfile =
        attested_read(s, ctx, "artifact", &selection.registration_id, Role::Admin).await?;
    let authority = parent_selection(s, ctx, &bundle.parent_digest).await?;
    let parent_strategy = match authority {
        ParentAuthority::EmptyRoot { id } => {
            attested_read::<EmptyRootAuthorization>(s, ctx, "artifact", &id, Role::Admin)
                .await?
                .strategy
        }
        ParentAuthority::ApprovedRelease { release_id } => {
            let release: crate::release_store::PersistentRelease =
                attested_read(s, ctx, "release", &release_id, Role::Admin).await?;
            s.need::<crate::release_store::ReleaseCandidateRecord>(
                ctx,
                "artifact",
                &release.candidate_id,
            )
            .await?
            .bundle
            .improver
        }
    };
    let verified = resolve_compiler_inputs(
        s,
        ctx,
        &CompilerInputRequest {
            profile: Profile {
                id: bundle.profile_id.clone(),
                evolution_enabled: true,
                parent_digest: bundle.parent_digest.clone(),
                baseline_digest: bundle.baseline_digest.clone(),
            },
            search_parent: bundle.skill.clone(),
            baseline: profile.baseline_skill,
            search_parent_strategy: bundle.improver.clone(),
            expected_parent_strategy_digest: Some(fingerprint(&parent_strategy)?),
            baseline_strategy: profile.baseline_strategy,
            capabilities: profile.capabilities,
            environment_digest: environment_digest.into(),
            purpose,
            revoke_watermark: watermark,
        },
    )
    .await?;
    verified.replay(bundle)?;
    // Set dependencies are new reads and are checked after replay as well.
    let output_roots = bundle
        .skill
        .dependencies
        .iter()
        .map(|id| {
            let found = verified.binding.closure.nodes.iter().find(|r| &r.id == id);
            found
                .map(|r| (r.kind.clone(), r.id.clone()))
                .ok_or_else(|| Error::Conflict("undeclared output dependency".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    read_typed_closure(s, ctx, &output_roots).await?;
    Ok(verified)
}
pub(crate) async fn record_parent_approval(
    s: &mut Session,
    ctx: &Context,
    release: &crate::release_store::PersistentRelease,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    let selection = ParentSelection {
        id: release.bundle_digest.clone(),
        schema_version: "rsia.formal_parent_selection.v1".into(),
        authority: ParentAuthority::ApprovedRelease {
            release_id: release.id.clone(),
        },
    };
    immutable(s, ctx, "formal_parent", &release.bundle_digest, &selection).await?;
    attest_content(s, ctx, "formal_parent", &release.bundle_digest, &selection).await?;
    attest_content(s, ctx, "release", &release.id, release).await?;
    s.put_edge(
        ctx,
        "formal_parent",
        &release.bundle_digest,
        "release",
        &release.id,
    )
    .await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalProviderCaptureRecord {
    pub id: String,
    pub schema_version: String,
    pub provider_registration_id: String,
    pub provider_registration_digest: String,
    pub host_registration_id: String,
    pub host_registration_digest: String,
    pub host_principal: String,
    pub host_pid: u32,
    pub host_uid: u32,
    pub host_code_digest: String,
    pub host_workspace: String,
    pub dispatch_id: String,
    pub request_digest: String,
    pub application_digest: String,
    pub bundle_digest: String,
    pub input_digest: String,
    pub output_digest: String,
    pub provider_request_id: String,
    pub usage_record_id: String,
    pub provider_response_digest: String,
    pub provider_response_utf8: String,
    pub actual_model_digest: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub actual_cost_micros: i64,
    pub currency: String,
    pub pricing_version: String,
}
fn validate_provider_response(c: &FormalProviderCaptureRecord) -> Result<Value> {
    if c.schema_version != "rsia.actual_formal_provider_capture.v1"
        || c.provider_response_utf8.len() > 2 * 1024 * 1024
        || hash(c.provider_response_utf8.as_bytes()) != c.provider_response_digest
    {
        return Err(Error::Forbidden);
    }
    let v: Value = serde_json::from_str(&c.provider_response_utf8).map_err(|_| Error::Forbidden)?;
    let output = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or(Error::Forbidden)?;
    if v["id"].as_str() != Some(c.provider_request_id.as_str())
        || v["usage"]["prompt_tokens"].as_u64() != Some(c.input_tokens)
        || v["usage"]["completion_tokens"].as_u64() != Some(c.output_tokens)
        || hash(output.as_bytes()) != c.output_digest
        || c.usage_record_id
            != format!(
                "usage-{}",
                &fingerprint(&(c.provider_request_id.as_str(), &v["usage"]))?[..32]
            )
    {
        return Err(Error::Conflict(
            "provider original response does not bind capture".into(),
        ));
    }
    Ok(v)
}
async fn verify_provider_registration(
    s: &mut Session,
    ctx: &Context,
    c: &FormalProviderCaptureRecord,
    host: &RegisteredFormalHost,
) -> Result<()> {
    let p: RegisteredExternalProvider =
        attested_read(s, ctx, "artifact", &c.provider_registration_id, Role::Admin).await?;
    p.validate(ctx)?;
    let v = validate_provider_response(c)?;
    let micros = u128::from(c.input_tokens)
        .checked_mul(u128::from(p.input_micros_per_million_tokens))
        .and_then(|n| {
            n.checked_add(
                u128::from(c.output_tokens) * u128::from(p.output_micros_per_million_tokens),
            )
        })
        .and_then(|n| n.checked_add(999_999))
        .ok_or(Error::Budget)?
        / 1_000_000;
    if fingerprint(&p)? != c.provider_registration_digest
        || fingerprint(host)? != c.host_registration_digest
        || p.host_registration_id != host.id
        || p.model_digest != c.actual_model_digest
        || v["model"].as_str() != Some(p.provider_model.as_str())
        || c.currency != p.currency
        || c.pricing_version != p.pricing_version
        || c.output_tokens > u64::from(p.max_output_tokens)
        || i64::try_from(micros).map_err(|_| Error::Budget)? != c.actual_cost_micros
    {
        return Err(Error::Conflict(
            "provider registration/model/pricing changed".into(),
        ));
    }
    Ok(())
}
/// Private fields and no Deserialize. A fixture implementation cannot return a
/// fabricated capture to the broker, even when it labels itself ExternalProvider.
pub struct CapturedFormalCompletion {
    completion: crate::broker::TransportCompletion,
    capture: FormalProviderCaptureRecord,
    ledger_eligible: bool,
}
impl CapturedFormalCompletion {
    pub(crate) fn into_parts(
        self,
    ) -> (
        crate::broker::TransportCompletion,
        FormalProviderCaptureRecord,
        bool,
    ) {
        (self.completion, self.capture, self.ledger_eligible)
    }
}
pub struct RegisteredExternalTransport {
    provider: RegisteredExternalProvider,
    host: RegisteredFormalHost,
    credential: String,
    process: crate::executor::ObservedProcessIdentity,
    client: reqwest::Client,
}
/// No public constructor or Deserialize; a provenance label cannot make one.
pub struct FormalTransportCapability {
    provider_id: String,
    provider_digest: String,
    host_id: String,
    host_digest: String,
    pid: u32,
    uid: u32,
}
impl FormalTransportCapability {
    pub(crate) async fn validate_optimizer_in_session(
        &self,
        s: &mut Session,
        ctx: &Context,
        bound: &crate::model::BoundModelRequest,
    ) -> Result<()> {
        let provider: RegisteredExternalProvider =
            attested_read(s, ctx, "artifact", &self.provider_id, Role::Admin).await?;
        let host: RegisteredFormalHost =
            attested_read(s, ctx, "artifact", &self.host_id, Role::Admin).await?;
        let profile: FrozenCompilerProfile = attested_read(
            s,
            ctx,
            "artifact",
            &bound.compiler_use.binding.profile_registration_id,
            Role::Admin,
        )
        .await?;
        let set = principal_set(s, ctx, &host).await?;
        let observed = crate::executor::observe_registered_process(
            host.host_uid,
            &host.code_digest,
            &host.host_workspace,
        )?;
        if fingerprint(&provider)? != self.provider_digest
            || fingerprint(&host)? != self.host_digest
            || profile.provider_registration_id != provider.id
            || profile.host_registration_id != host.id
            || provider.model_digest != bound.request.model_digest
            || ctx.actor() != set.host.actor
            || observed.uid != self.uid
            || observed.pid != self.pid
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    pub(crate) async fn validate_in_session(
        &self,
        s: &mut Session,
        ctx: &Context,
        request: &crate::model::FormalTaskRequest,
    ) -> Result<()> {
        let provider: RegisteredExternalProvider =
            attested_read(s, ctx, "artifact", &self.provider_id, Role::Admin).await?;
        let host: RegisteredFormalHost =
            attested_read(s, ctx, "artifact", &self.host_id, Role::Admin).await?;
        let profile: FrozenCompilerProfile = attested_read(
            s,
            ctx,
            "artifact",
            &request.profile_registration_id,
            Role::Admin,
        )
        .await?;
        let set = principal_set(s, ctx, &host).await?;
        if ctx.actor() != set.host.actor || ctx.role() != Role::Host {
            return Err(Error::Forbidden);
        }
        let observed = crate::executor::observe_registered_process(
            host.host_uid,
            &host.code_digest,
            &host.host_workspace,
        )?;
        if fingerprint(&provider)? != self.provider_digest
            || fingerprint(&host)? != self.host_digest
            || profile.provider_registration_id != provider.id
            || profile.host_registration_id != host.id
            || observed.pid != self.pid
            || observed.uid != self.uid
            || provider.model_digest != request.model_digest
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
impl RegisteredExternalTransport {
    pub async fn connect(ctx: &Context, store: &Store, registration_id: &str) -> Result<Self> {
        ctx.require(&[Role::Host])?;
        let mut s = store.session().await?;
        let provider: RegisteredExternalProvider =
            attested_read(&mut s, ctx, "artifact", registration_id, Role::Admin).await?;
        let host: RegisteredFormalHost = attested_read(
            &mut s,
            ctx,
            "artifact",
            &provider.host_registration_id,
            Role::Admin,
        )
        .await?;
        provider.validate(ctx)?;
        host.validate(ctx)?;
        let set = principal_set(&mut s, ctx, &host).await?;
        if ctx.actor() != set.host.actor || ctx.role() != Role::Host {
            return Err(Error::Forbidden);
        }
        read_typed_closure(&mut s, ctx, &[("artifact".into(), provider.id.clone())]).await?;
        s.commit().await?;
        let credential =
            std::env::var(&provider.credential_environment_key).map_err(|_| Error::Forbidden)?;
        if credential.is_empty()
            || hash(credential.as_bytes()) != provider.credential_principal
            || provider.credential_principal != host.host_principal
            || hash(credential.as_bytes()) != set.host.credential_digest
        {
            return Err(Error::Forbidden);
        }
        let process = crate::executor::observe_registered_process(
            host.host_uid,
            &host.code_digest,
            &host.host_workspace,
        )?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|_| Error::Internal)?;
        Ok(Self {
            provider,
            host,
            credential,
            process,
            client,
        })
    }
    async fn complete(
        &self,
        messages: Vec<Value>,
        model_digest: &str,
    ) -> Result<(crate::broker::TransportCompletion, String, u64, u64)> {
        if model_digest != self.provider.model_digest {
            return Err(Error::Forbidden);
        }
        // A single Admin-frozen chat-completions protocol. The candidate cannot
        // choose a destination, credentials, tools, filesystem or request fields.
        let mut response = self
            .client
            .post(&self.provider.endpoint)
            .bearer_auth(&self.credential)
            .json(
                &serde_json::json!({"model":self.provider.provider_model,"messages":messages,
                "max_tokens":self.provider.max_output_tokens,"temperature":0}),
            )
            .send()
            .await
            .map_err(|_| Error::Internal)?;
        if !response.status().is_success() {
            return Err(Error::Conflict(
                "registered provider rejected request; usage unresolved".into(),
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > 2 * 1024 * 1024)
        {
            return Err(Error::Conflict(
                "provider response exceeds capture bound; usage unresolved".into(),
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Conflict("provider body interrupted; usage unresolved".into()))?
        {
            append_bounded_body(&mut body, &chunk, 2 * 1024 * 1024)?;
        }
        let value: Value = serde_json::from_slice(&body).map_err(|_| {
            Error::Conflict("provider response unreadable; usage unresolved".into())
        })?;
        let id = value["id"].as_str().ok_or(Error::Internal)?.to_string();
        identifier(&id)?;
        let actual_model = value["model"].as_str().ok_or(Error::Internal)?;
        if actual_model != self.provider.provider_model {
            return Err(Error::Conflict(
                "provider actual model differs; retain unknown usage".into(),
            ));
        }
        let output = value["choices"][0]["message"]["content"]
            .as_str()
            .ok_or(Error::Internal)?
            .to_string();
        evo_core::text(&output, "actual provider output", 256 * 1024)?;
        let input = value["usage"]["prompt_tokens"]
            .as_u64()
            .ok_or(Error::Internal)?;
        let out = value["usage"]["completion_tokens"]
            .as_u64()
            .ok_or(Error::Internal)?;
        let micros = u128::from(input)
            .checked_mul(u128::from(self.provider.input_micros_per_million_tokens))
            .and_then(|n| {
                n.checked_add(
                    u128::from(out) * u128::from(self.provider.output_micros_per_million_tokens),
                )
            })
            .and_then(|n| n.checked_add(999_999))
            .ok_or(Error::Budget)?
            / 1_000_000;
        let actual_cost_micros = i64::try_from(micros).map_err(|_| Error::Budget)?;
        let completion = crate::broker::TransportCompletion {
            response_id: id.clone(),
            provider_request_id: id.clone(),
            usage_record_id: format!(
                "usage-{}",
                &fingerprint(&(id.as_str(), &value["usage"]))?[..32]
            ),
            actual_model_digest: self.provider.model_digest.clone(),
            output,
            actual_cost_micros,
            currency: self.provider.currency.clone(),
            pricing_version: self.provider.pricing_version.clone(),
        };
        Ok((
            completion,
            String::from_utf8(body).map_err(|_| Error::Internal)?,
            input,
            out,
        ))
    }
}
#[async_trait::async_trait]
impl crate::broker::ModelTransport for RegisteredExternalTransport {
    fn formal_capability(&self) -> Option<FormalTransportCapability> {
        Some(FormalTransportCapability {
            provider_id: self.provider.id.clone(),
            provider_digest: fingerprint(&self.provider).ok()?,
            host_id: self.host.id.clone(),
            host_digest: fingerprint(&self.host).ok()?,
            pid: self.process.pid,
            uid: self.process.uid,
        })
    }
    fn provenance(&self) -> Option<crate::model::ModelExecutionProvenance> {
        Some(crate::model::ModelExecutionProvenance::ExternalProvider)
    }
    async fn execute(
        &self,
        request: &evo_core::optimization::ModelRequest,
        _dispatch_id: &str,
    ) -> Result<crate::broker::TransportCompletion> {
        request.validate()?;
        let messages = request.input.iter().map(|part| serde_json::json!({
            "role": if part.role == evo_core::optimization::ModelInputRole::System { "system" } else { "user" },
            "content":part.content
        })).collect();
        self.complete(messages, &request.model_digest)
            .await
            .map(|v| v.0)
    }
    async fn execute_formal(
        &self,
        request: &crate::model::FormalTaskRequest,
        dispatch_id: &str,
    ) -> Result<CapturedFormalCompletion> {
        request.validate()?;
        let process = crate::executor::observe_registered_process(
            self.host.host_uid,
            &self.host.code_digest,
            &self.host.host_workspace,
        )?;
        if process.pid != self.process.pid
            || hash(request.mandatory_context.as_bytes())
                != self.host.snapshot.mandatory_context_digest
            || request.projection.tools != self.host.snapshot.tools
        {
            return Err(Error::Conflict(
                "actual host application/mandatory context differs".into(),
            ));
        }
        let mut messages =
            vec![serde_json::json!({"role":"system","content":request.mandatory_context})];
        for instruction in &request.projection.instructions {
            messages.push(serde_json::json!({"role":"system","content":instruction}));
        }
        messages.push(serde_json::json!({"role":"user","content":request.task_input_utf8}));
        let application_digest = fingerprint(&messages)?;
        let (completion, response_utf8, input_tokens, output_tokens) =
            self.complete(messages, &request.model_digest).await?;
        let capture = FormalProviderCaptureRecord {
            id: format!("formal-capture-{}", &hash(dispatch_id.as_bytes())[..32]),
            schema_version: "rsia.actual_formal_provider_capture.v1".into(),
            provider_registration_id: self.provider.id.clone(),
            provider_registration_digest: fingerprint(&self.provider)?,
            host_registration_id: self.host.id.clone(),
            host_registration_digest: fingerprint(&self.host)?,
            host_principal: self.host.host_principal.clone(),
            host_pid: process.pid,
            host_uid: process.uid,
            host_code_digest: process.executable_digest,
            host_workspace: process.workspace,
            dispatch_id: dispatch_id.into(),
            request_digest: request.task.request_digest.clone(),
            application_digest,
            bundle_digest: request.task.bundle_digest.clone(),
            input_digest: request.task.input_digest.clone(),
            output_digest: hash(completion.output.as_bytes()),
            provider_request_id: completion.provider_request_id.clone(),
            usage_record_id: completion.usage_record_id.clone(),
            provider_response_digest: hash(response_utf8.as_bytes()),
            provider_response_utf8: response_utf8,
            actual_model_digest: completion.actual_model_digest.clone(),
            input_tokens,
            output_tokens,
            actual_cost_micros: completion.actual_cost_micros,
            currency: completion.currency.clone(),
            pricing_version: completion.pricing_version.clone(),
        };
        validate_provider_response(&capture)?;
        // A valid provider usage charge remains known even when JSON escaping
        // makes the full capture exceed the ledger's artifact bound. The broker
        // durably charges it with an unusable digest-only capture, never retries
        // it and never issues formal authority from the incomplete evidence.
        let ledger_eligible = serde_json::to_vec(&capture)
            .map_err(|_| Error::Internal)?
            .len()
            <= 4 * 1024 * 1024;
        Ok(CapturedFormalCompletion {
            completion,
            capture,
            ledger_eligible,
        })
    }
}

pub async fn prepare_formal_task(
    ctx: &Context,
    store: &Store,
    task: crate::streaming_evaluator::BrokerTaskView,
    mandatory_context: String,
    task_input_utf8: String,
) -> Result<crate::model::FormalTaskRequest> {
    ctx.require(&[Role::Host])?;
    let mut s = store.session().await?;
    let (candidate, _) =
        crate::streaming_evaluator::live_task_candidate_in_session(ctx, &mut s, &task).await?;
    let verified = resolve_bundle_in_session(
        &mut s,
        ctx,
        &candidate.bundle,
        &candidate.environment_digest,
        candidate.revoke_watermark,
        Purpose::Development,
    )
    .await?;
    let bundle = match task.side {
        crate::streaming_evaluator::ExecutionSide::Candidate => candidate.bundle.clone(),
        crate::streaming_evaluator::ExecutionSide::Baseline => {
            let reset = evo_core::contract::PatchOp::ResetToBaseline;
            verified.compile(
                &SkillPatch {
                    content: reset,
                    applicability: evo_core::contract::PatchOp::ResetToBaseline,
                    counterexample: evo_core::contract::PatchOp::ResetToBaseline,
                    required_capabilities: evo_core::contract::PatchOp::ResetToBaseline,
                    dependencies: evo_core::contract::PatchOp::ResetToBaseline,
                },
                &ImproverPatch {
                    instruction: evo_core::contract::PatchOp::ResetToBaseline,
                    max_candidates: evo_core::contract::PatchOp::ResetToBaseline,
                    max_rounds: evo_core::contract::PatchOp::ResetToBaseline,
                },
            )?
        }
    };
    if bundle.digest != task.bundle_digest
        || hash(mandatory_context.as_bytes())
            != verified.profile.system_snapshot.mandatory_context_digest
    {
        return Err(Error::Conflict(
            "frozen pair/mandatory host application mismatch".into(),
        ));
    }
    let provider: RegisteredExternalProvider = attested_read(
        &mut s,
        ctx,
        "artifact",
        &verified.profile.provider_registration_id,
        Role::Admin,
    )
    .await?;
    let projection =
        evo_core::contract::project_run(&bundle, &verified.profile.system_snapshot, true)?;
    let request = crate::model::FormalTaskRequest {
        task,
        candidate_id: candidate.id,
        profile_registration_id: verified.profile.id,
        compiler_binding_digest: verified.binding.binding_digest,
        model_digest: provider.model_digest,
        mandatory_context,
        projection,
        task_input_utf8,
    };
    request.validate()?;
    s.commit().await?;
    Ok(request)
}
pub(crate) async fn verify_formal_task_in_session(
    ctx: &Context,
    s: &mut Session,
    request: &crate::model::FormalTaskRequest,
) -> Result<(crate::release_store::ReleaseCandidateRecord, String)> {
    request.validate()?;
    let (candidate, billing) =
        crate::streaming_evaluator::live_task_candidate_in_session(ctx, s, &request.task).await?;
    let inputs = resolve_bundle_in_session(
        s,
        ctx,
        &candidate.bundle,
        &candidate.environment_digest,
        candidate.revoke_watermark,
        Purpose::Development,
    )
    .await?;
    if candidate.id != request.candidate_id
        || inputs.profile.id != request.profile_registration_id
        || inputs.binding.binding_digest != request.compiler_binding_digest
        || hash(request.mandatory_context.as_bytes())
            != inputs.profile.system_snapshot.mandatory_context_digest
    {
        return Err(Error::Conflict(
            "formal request input binding became stale".into(),
        ));
    }
    let bundle = if request.task.side == crate::streaming_evaluator::ExecutionSide::Candidate {
        candidate.bundle.clone()
    } else {
        inputs.compile(
            &SkillPatch {
                content: evo_core::contract::PatchOp::ResetToBaseline,
                applicability: evo_core::contract::PatchOp::ResetToBaseline,
                counterexample: evo_core::contract::PatchOp::ResetToBaseline,
                required_capabilities: evo_core::contract::PatchOp::ResetToBaseline,
                dependencies: evo_core::contract::PatchOp::ResetToBaseline,
            },
            &ImproverPatch {
                instruction: evo_core::contract::PatchOp::ResetToBaseline,
                max_candidates: evo_core::contract::PatchOp::ResetToBaseline,
                max_rounds: evo_core::contract::PatchOp::ResetToBaseline,
            },
        )?
    };
    let projection =
        evo_core::contract::project_run(&bundle, &inputs.profile.system_snapshot, true)?;
    if fingerprint(&projection)? != fingerprint(&request.projection)?
        || bundle.digest != request.task.bundle_digest
    {
        return Err(Error::Conflict(
            "actual projection differs from authenticated bundle".into(),
        ));
    }
    Ok((candidate, billing))
}
pub(crate) async fn persist_formal_capture(
    s: &mut Session,
    ctx: &Context,
    candidate_id: &str,
    capture: &FormalProviderCaptureRecord,
) -> Result<()> {
    ctx.require(&[Role::Host])?;
    immutable(s, ctx, "artifact", &capture.id, capture).await?;
    attest_content(s, ctx, "artifact", &capture.id, capture).await?;
    for id in [
        candidate_id,
        &capture.provider_registration_id,
        &capture.host_registration_id,
    ] {
        s.put_edge(ctx, "artifact", &capture.id, "artifact", id)
            .await?;
    }
    s.audit(ctx, "formal.provider.capture", &capture.id).await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeCostRecord {
    pub id: String,
    pub schema_version: String,
    pub ticket_id: String,
    pub profile_registration_id: String,
    pub measurements: CompleteEpisodeCostMeasurements,
    pub measurement_sources: Vec<TypedContentRef>,
    pub observed_by: String,
    pub observed_pid: u32,
    pub observed_uid: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndependentGradeCapture {
    pub id: String,
    pub schema_version: String,
    pub ticket_id: String,
    pub target_id: String,
    pub grader_receipt_digest: String,
    pub host_registration_id: String,
    pub grader_principal: String,
    pub grader_pid: u32,
    pub grader_uid: u32,
    pub grader_code_digest: String,
    pub grader_workspace: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssuedFormalAuthority {
    pub id: String,
    pub schema_version: String,
    pub report_id: String,
    pub raw_report_digest: String,
    pub ticket_digest: String,
    pub candidate_id: String,
    pub candidate_bundle_digest: String,
    pub environment_digest: String,
    pub compiler_binding: FormalCompilerInputBinding,
    pub billing_scope: String,
    pub root_seal_digest: String,
    pub episode_cost_digest: String,
    pub host_registration_id: String,
    pub principal_set_digest: String,
    pub report_principal_action_id: String,
    pub provider_captures: Vec<TypedContentRef>,
    pub grader_captures: Vec<TypedContentRef>,
    pub closure_roots: Vec<(String, String)>,
    pub closure: TypedContentClosure,
    pub watermark: Option<(i64, String)>,
    pub issued_by: String,
}
pub struct FormalAuthorityIssuer;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredResourceInstrumentation {
    pub id: String,
    pub schema_version: String,
    pub namespace: String,
    pub host_registration_id: String,
    pub endpoint: String,
    pub credential_environment_key: String,
    pub credential_principal: String,
    pub instrumentation_code_digest: String,
    pub authorized_by: String,
}
impl RegisteredResourceInstrumentation {
    fn validate(&self, ctx: &Context) -> Result<()> {
        if self.schema_version != METER_SCHEMA
            || self.namespace != ctx.namespace()
            || self.authorized_by.is_empty()
            || !self.endpoint.starts_with("https://")
            || self.endpoint.contains('@')
        {
            return Err(Error::Forbidden);
        }
        for id in [
            &self.id,
            &self.host_registration_id,
            &self.credential_environment_key,
        ] {
            identifier(id)?;
        }
        digest(&self.credential_principal)?;
        digest(&self.instrumentation_code_digest)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceInstrumentationResponse {
    schema_version: String,
    id: String,
    request_digest: String,
    episode_id: String,
    root_budget_id: String,
    root_snapshot_digest: String,
    instrumentation_code_digest: String,
    complete_episode_root: bool,
    cpu_nanoseconds: Option<u64>,
    storage_byte_seconds: Option<u64>,
    retained_history_bytes: Option<u64>,
    human_nanoseconds: Option<u64>,
    gray_release_nanoseconds: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceMeasurementRecord {
    id: String,
    schema_version: String,
    episode_id: String,
    ticket_id: String,
    billing_scope: String,
    root_snapshot_digest: String,
    host_registration_id: String,
    meter_registration_id: String,
    meter_registration_digest: String,
    observed_pid: u32,
    observed_uid: u32,
    dimension: String,
    unit: String,
    amount: u64,
    response_utf8: String,
    response_digest: String,
    request_digest: String,
}
fn strict_raw_object<T: serde::de::DeserializeOwned>(raw: &[u8], limit: usize) -> Result<T> {
    if raw.len() > limit
        || raw
            .iter()
            .find(|b| !matches!(**b, b' ' | b'\t' | b'\r' | b'\n'))
            != Some(&b'{')
    {
        return Err(Error::Forbidden);
    }
    // Decode original bytes directly: Value would collapse duplicate keys.
    serde_json::from_slice(raw).map_err(|_| Error::Forbidden)
}
fn validate_meter_response(
    r: &ResourceMeasurementRecord,
) -> Result<ResourceInstrumentationResponse> {
    if r.schema_version != "rsia.trusted_resource_meter.v1"
        || r.response_utf8.len() > 512 * 1024
        || hash(r.response_utf8.as_bytes()) != r.response_digest
    {
        return Err(Error::Forbidden);
    }
    let response: ResourceInstrumentationResponse =
        strict_raw_object(r.response_utf8.as_bytes(), 512 * 1024)?;
    if response.schema_version != "rsia.episode_resource_measurements.v1"
        || !response.complete_episode_root
        || response.episode_id != r.episode_id
        || response.root_snapshot_digest != r.root_snapshot_digest
        || response.request_digest != r.request_digest
    {
        return Err(Error::Forbidden);
    }
    Ok(response)
}
#[allow(clippy::too_many_arguments)]
async fn validate_episode_meter_sources(
    s: &mut Session,
    ctx: &Context,
    profile: &FrozenCompilerProfile,
    host: &RegisteredFormalHost,
    ticket: &crate::streaming_evaluator::EvaluationTicketV2,
    control: &crate::streaming_evaluator::RegisteredEvaluationControl,
    seal: &evo_storage::budget::SealedRootBudgetEvidence,
    measurements: &CompleteEpisodeCostMeasurements,
    sources: &[TypedContentRef],
) -> Result<()> {
    measurements.validate_complete()?;
    if measurements.episode_id != control.attempt_id
        || sources.len() != 5
        || !seal.monetary_complete
        || seal.root.billing_scope != control.billing_scope
    {
        return Err(Error::Forbidden);
    }
    let meter: RegisteredResourceInstrumentation = attested_read(
        s,
        ctx,
        "artifact",
        profile
            .resource_instrumentation_id
            .as_deref()
            .ok_or(Error::Forbidden)?,
        Role::Admin,
    )
    .await?;
    meter.validate(ctx)?;
    if meter.host_registration_id != host.id {
        return Err(Error::Forbidden);
    }
    let request = serde_json::json!({"schema_version":"rsia.episode_resource_measurement_request.v1",
        "namespace":ctx.namespace(),"ticket_id":ticket.id,"episode_id":control.attempt_id,
        "billing_scope":control.billing_scope,"root_budget_id":seal.root.root_budget_id,
        "root_snapshot_digest":seal.snapshot_digest,"host_registration_id":host.id,
        "call_count":seal.call_count,"dispatch_group_count":seal.dispatch_group_count,"stage_totals":seal.stage_totals});
    let request_digest = fingerprint(&request)?;
    let mut actual = BTreeMap::new();
    for source in sources {
        if source.kind != "artifact" || source.schema_version != "rsia.trusted_resource_meter.v1" {
            return Err(Error::Forbidden);
        }
        let r: ResourceMeasurementRecord =
            attested_read(s, ctx, "artifact", &source.id, Role::Host).await?;
        let response = validate_meter_response(&r)?;
        if r.ticket_id != ticket.id
            || r.episode_id != control.attempt_id
            || r.billing_scope != control.billing_scope
            || r.root_snapshot_digest != seal.snapshot_digest
            || r.host_registration_id != host.id
            || r.observed_pid == 0
            || r.observed_uid != host.host_uid
            || r.meter_registration_id != meter.id
            || r.meter_registration_digest != fingerprint(&meter)?
            || response.instrumentation_code_digest != meter.instrumentation_code_digest
            || response.root_budget_id != seal.root.root_budget_id
            || r.request_digest != request_digest
            || actual
                .insert(r.dimension.clone(), (r.unit.clone(), r.amount))
                .is_some()
        {
            return Err(Error::Forbidden);
        }
        let amount = match r.dimension.as_str() {
            "cpu" => response.cpu_nanoseconds,
            "storage" => response.storage_byte_seconds,
            "history" => response.retained_history_bytes,
            "human" => response.human_nanoseconds,
            "gray_release" => response.gray_release_nanoseconds,
            _ => return Err(Error::Forbidden),
        };
        if amount != Some(r.amount) {
            return Err(Error::Forbidden);
        }
    }
    for (dimension, unit, amount) in [
        ("cpu", "nanoseconds", measurements.cpu_nanoseconds),
        ("storage", "byte_seconds", measurements.storage_byte_seconds),
        ("history", "bytes", measurements.retained_history_bytes),
        ("human", "nanoseconds", measurements.human_nanoseconds),
        (
            "gray_release",
            "nanoseconds",
            measurements.gray_release_nanoseconds,
        ),
    ] {
        if actual.get(dimension) != amount.map(|n| (unit.to_string(), n)).as_ref() {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
async fn record_meter_sources_and_cost_in_session(
    s: &mut Session,
    ctx: &Context,
    ticket_id: &str,
    profile_id: &str,
    mut measurements: CompleteEpisodeCostMeasurements,
    records: &[ResourceMeasurementRecord],
) -> Result<()> {
    ctx.require(&[Role::Host])?;
    measurements.validate_complete()?;
    if records.len() != 5 {
        return Err(Error::Forbidden);
    }
    // Every original byte stream is strictly typed before the first immutable
    // source/witness/edge is written. The caller commits only after cost succeeds.
    for record in records {
        validate_meter_response(record)?;
    }
    let mut sources = Vec::new();
    for record in records {
        immutable(s, ctx, "artifact", &record.id, record).await?;
        attest_content(s, ctx, "artifact", &record.id, record).await?;
        for id in [&record.meter_registration_id, &record.host_registration_id] {
            s.put_edge(ctx, "artifact", &record.id, "artifact", id)
                .await?;
        }
        sources.push(TypedContentRef {
            kind: "artifact".into(),
            id: record.id.clone(),
            schema_version: record.schema_version.clone(),
            content_digest: content_digest(record)?,
        });
    }
    measurements.measurement_sources = sources.iter().map(|r| r.id.clone()).collect();
    FormalAuthorityIssuer::record_episode_cost_in_session(
        ctx,
        s,
        ticket_id,
        profile_id,
        measurements,
        sources,
    )
    .await
}
impl FormalAuthorityIssuer {
    /// One frozen instrumentation protocol, independent of candidate/model
    /// text. Its authenticated service must measure the entire episode/root.
    /// Missing CPU/storage/history/human/gray measurements refuse, including
    /// ostensibly unused dimensions: no None-to-zero conversion is permitted.
    pub async fn capture_episode_cost(
        ctx: &Context,
        store: &Store,
        ticket_id: &str,
        profile_registration_id: &str,
    ) -> Result<()> {
        ctx.require(&[Role::Host])?;
        let mut s = store.session().await?;
        let profile: FrozenCompilerProfile = attested_read(
            &mut s,
            ctx,
            "artifact",
            profile_registration_id,
            Role::Admin,
        )
        .await?;
        let host: RegisteredFormalHost = attested_read(
            &mut s,
            ctx,
            "artifact",
            &profile.host_registration_id,
            Role::Admin,
        )
        .await?;
        let meter: RegisteredResourceInstrumentation = attested_read(
            &mut s,
            ctx,
            "artifact",
            profile
                .resource_instrumentation_id
                .as_deref()
                .ok_or(Error::Forbidden)?,
            Role::Admin,
        )
        .await?;
        meter.validate(ctx)?;
        host.validate(ctx)?;
        let ticket: crate::streaming_evaluator::EvaluationTicketV2 =
            crate::streaming_evaluator::need_record(&mut s, ctx, "evaluation_ticket_v2", ticket_id)
                .await?;
        let control: crate::streaming_evaluator::RegisteredEvaluationControl =
            crate::streaming_evaluator::need_record(
                &mut s,
                ctx,
                "registered_evaluation_control_v41",
                &ticket.registration_id,
            )
            .await?;
        let seal = s
            .sealed_root_budget_evidence(ctx, &control.billing_scope)
            .await?
            .ok_or(Error::NotFound)?;
        if !seal.monetary_complete || meter.host_registration_id != host.id {
            return Err(Error::Forbidden);
        }
        let observed = observe_host_principal(&mut s, ctx, &host, &profile).await?;
        let request = serde_json::json!({"schema_version":"rsia.episode_resource_measurement_request.v1",
            "namespace":ctx.namespace(),"ticket_id":ticket_id,"episode_id":control.attempt_id,
            "billing_scope":control.billing_scope,"root_budget_id":seal.root.root_budget_id,
            "root_snapshot_digest":seal.snapshot_digest,"host_registration_id":host.id,
            "call_count":seal.call_count,"dispatch_group_count":seal.dispatch_group_count,"stage_totals":seal.stage_totals});
        let request_digest = fingerprint(&request)?;
        s.commit().await?;
        let credential =
            std::env::var(&meter.credential_environment_key).map_err(|_| Error::Forbidden)?;
        if credential.is_empty() || hash(credential.as_bytes()) != meter.credential_principal {
            return Err(Error::Forbidden);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|_| Error::Internal)?;
        let mut response = client
            .post(&meter.endpoint)
            .bearer_auth(credential)
            .json(&serde_json::json!({
            "request":request,"request_digest":request_digest}))
            .send()
            .await
            .map_err(|_| Error::Internal)?;
        if !response.status().is_success() {
            return Err(Error::Conflict(
                "resource instrumentation unavailable".into(),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Internal)? {
            append_bounded_body(&mut bytes, &chunk, 512 * 1024)?;
        }
        let reply: ResourceInstrumentationResponse = strict_raw_object(&bytes, 512 * 1024)?;
        identifier(&reply.id)?;
        if reply.schema_version != "rsia.episode_resource_measurements.v1"
            || reply.request_digest != request_digest
            || reply.episode_id != control.attempt_id
            || reply.root_budget_id != seal.root.root_budget_id
            || reply.root_snapshot_digest != seal.snapshot_digest
            || !reply.complete_episode_root
            || reply.instrumentation_code_digest != meter.instrumentation_code_digest
        {
            return Err(Error::Forbidden);
        }
        let measurements = CompleteEpisodeCostMeasurements {
            episode_id: reply.episode_id,
            cpu_nanoseconds: reply.cpu_nanoseconds,
            storage_byte_seconds: reply.storage_byte_seconds,
            retained_history_bytes: reply.retained_history_bytes,
            human_nanoseconds: reply.human_nanoseconds,
            gray_release_nanoseconds: reply.gray_release_nanoseconds,
            measurement_sources: vec!["pending".into()],
        };
        measurements.validate_complete()?;
        let mut s = store.session().await?;
        let latest = s
            .sealed_root_budget_evidence(ctx, &control.billing_scope)
            .await?
            .ok_or(Error::NotFound)?;
        if fingerprint(&latest)? != fingerprint(&seal)? {
            return Err(Error::Conflict(
                "root changed during resource observation".into(),
            ));
        }
        let current_meter: RegisteredResourceInstrumentation =
            attested_read(&mut s, ctx, "artifact", &meter.id, Role::Admin).await?;
        if fingerprint(&current_meter)? != fingerprint(&meter)? {
            return Err(Error::Forbidden);
        }
        let response_utf8 = String::from_utf8(bytes).map_err(|_| Error::Forbidden)?;
        let mut records = Vec::new();
        for (dimension, amount, unit) in [
            ("cpu", measurements.cpu_nanoseconds, "nanoseconds"),
            ("storage", measurements.storage_byte_seconds, "byte_seconds"),
            ("history", measurements.retained_history_bytes, "bytes"),
            ("human", measurements.human_nanoseconds, "nanoseconds"),
            (
                "gray_release",
                measurements.gray_release_nanoseconds,
                "nanoseconds",
            ),
        ] {
            let record = ResourceMeasurementRecord {
                id: format!("formal-meter-{ticket_id}-{dimension}"),
                schema_version: "rsia.trusted_resource_meter.v1".into(),
                episode_id: measurements.episode_id.clone(),
                ticket_id: ticket_id.into(),
                billing_scope: control.billing_scope.clone(),
                root_snapshot_digest: seal.snapshot_digest.clone(),
                host_registration_id: host.id.clone(),
                meter_registration_id: meter.id.clone(),
                meter_registration_digest: fingerprint(&meter)?,
                observed_pid: observed.pid,
                observed_uid: observed.uid,
                dimension: dimension.into(),
                unit: unit.into(),
                amount: amount.ok_or(Error::Forbidden)?,
                response_digest: hash(response_utf8.as_bytes()),
                response_utf8: response_utf8.clone(),
                request_digest: request_digest.clone(),
            };
            records.push(record);
        }
        record_meter_sources_and_cost_in_session(
            &mut s,
            ctx,
            ticket_id,
            profile_registration_id,
            measurements,
            &records,
        )
        .await?;
        s.commit().await
    }
    /// A registered host meter records actual observed dimensions and their
    /// immutable instrumentation records. Absence remains None and refuses.
    pub async fn record_episode_cost(
        ctx: &Context,
        store: &Store,
        ticket_id: &str,
        profile_id: &str,
        measurements: CompleteEpisodeCostMeasurements,
        sources: Vec<TypedContentRef>,
    ) -> Result<()> {
        ctx.require(&[Role::Host])?;
        measurements.validate_complete()?;
        let mut s = store.session().await?;
        Self::record_episode_cost_in_session(
            ctx,
            &mut s,
            ticket_id,
            profile_id,
            measurements,
            sources,
        )
        .await?;
        s.commit().await
    }
    async fn record_episode_cost_in_session(
        ctx: &Context,
        s: &mut Session,
        ticket_id: &str,
        profile_id: &str,
        measurements: CompleteEpisodeCostMeasurements,
        sources: Vec<TypedContentRef>,
    ) -> Result<()> {
        ctx.require(&[Role::Host])?;
        measurements.validate_complete()?;
        let profile: FrozenCompilerProfile =
            attested_read(s, ctx, "artifact", profile_id, Role::Admin).await?;
        let host: RegisteredFormalHost = attested_read(
            s,
            ctx,
            "artifact",
            &profile.host_registration_id,
            Role::Admin,
        )
        .await?;
        let observed = observe_host_principal(s, ctx, &host, &profile).await?;
        verify_refs(s, ctx, &sources).await?;
        let ids: BTreeSet<_> = sources.iter().map(|r| r.id.as_str()).collect();
        if ids
            != measurements
                .measurement_sources
                .iter()
                .map(String::as_str)
                .collect()
        {
            return Err(Error::Conflict(
                "episode measurements have undeclared instrumentation".into(),
            ));
        }
        let ticket: crate::streaming_evaluator::EvaluationTicketV2 =
            crate::streaming_evaluator::need_record(s, ctx, "evaluation_ticket_v2", ticket_id)
                .await?;
        let control: crate::streaming_evaluator::RegisteredEvaluationControl =
            crate::streaming_evaluator::need_record(
                s,
                ctx,
                "registered_evaluation_control_v41",
                &ticket.registration_id,
            )
            .await?;
        let seal = s
            .sealed_root_budget_evidence(ctx, &control.billing_scope)
            .await?
            .ok_or(Error::NotFound)?;
        validate_episode_meter_sources(
            s,
            ctx,
            &profile,
            &host,
            &ticket,
            &control,
            &seal,
            &measurements,
            &sources,
        )
        .await?;
        let record = EpisodeCostRecord {
            id: format!("formal-cost-{ticket_id}"),
            schema_version: "rsia.complete_episode_cost.v1".into(),
            ticket_id: ticket_id.into(),
            profile_registration_id: profile_id.into(),
            measurements,
            measurement_sources: sources,
            observed_by: ctx.actor().into(),
            observed_pid: observed.pid,
            observed_uid: observed.uid,
        };
        immutable(s, ctx, "artifact", &record.id, &record).await?;
        attest_content(s, ctx, "artifact", &record.id, &record).await?;
        edges(s, ctx, "artifact", &record.id, &record.measurement_sources).await?;
        s.audit(ctx, "formal.episode.cost", &record.id).await
    }
    /// On restore or before live use, unknown/stale schemas fail closed. This
    /// returns only a verifier result, never upgrades a stored fixture record.
    pub async fn revalidate(
        ctx: &Context,
        store: &Store,
        report_id: &str,
        candidate_id: &str,
    ) -> Result<IssuedFormalAuthority> {
        let mut s = store.session().await?;
        let candidate = s.need(ctx, "artifact", candidate_id).await?;
        let result = verify_issued_authority(&mut s, ctx, report_id, &candidate).await?;
        s.commit().await?;
        Ok(result)
    }
}
pub(crate) async fn capture_independent_grade(
    s: &mut Session,
    ctx: &Context,
    ticket: &crate::streaming_evaluator::EvaluationTicketV2,
    target_id: &str,
    receipt_id: &str,
    receipt_digest: &str,
) -> Result<()> {
    ctx.require(&[Role::Evaluator])?;
    let candidate: crate::release_store::ReleaseCandidateRecord =
        s.need(ctx, "artifact", &ticket.candidate_digest).await?;
    let inputs = resolve_bundle_in_session(
        s,
        ctx,
        &candidate.bundle,
        &candidate.environment_digest,
        candidate.revoke_watermark,
        Purpose::Development,
    )
    .await?;
    let host: RegisteredFormalHost = attested_read(
        s,
        ctx,
        "artifact",
        &inputs.profile.host_registration_id,
        Role::Admin,
    )
    .await?;
    host.validate(ctx)?;
    let control: crate::streaming_evaluator::RegisteredEvaluationControl =
        crate::streaming_evaluator::need_record(
            s,
            ctx,
            "registered_evaluation_control_v41",
            &ticket.registration_id,
        )
        .await?;
    let set = verify_candidate_principals(s, ctx, &host.id, &candidate, &control).await?;
    process_principal(ctx, &host, &set, Role::Evaluator)?;
    let credential =
        std::env::var(&host.grader_credential_environment_key).map_err(|_| Error::Forbidden)?;
    if hash(credential.as_bytes()) != host.grader_principal
        || host.grader_principal == host.host_principal
    {
        return Err(Error::Forbidden);
    }
    let observed = crate::executor::observe_registered_process(
        host.grader_uid,
        &host.grader_code_digest,
        &host.grader_workspace,
    )?;
    let record = IndependentGradeCapture {
        id: format!(
            "formal-grade-{}",
            &fingerprint(&(ticket.id.as_str(), target_id))?[..32]
        ),
        schema_version: "rsia.independent_grade_capture.v1".into(),
        ticket_id: ticket.id.clone(),
        target_id: target_id.into(),
        grader_receipt_digest: receipt_digest.into(),
        host_registration_id: host.id.clone(),
        grader_principal: host.grader_principal,
        grader_pid: observed.pid,
        grader_uid: observed.uid,
        grader_code_digest: observed.executable_digest,
        grader_workspace: observed.workspace,
    };
    immutable(s, ctx, "artifact", &record.id, &record).await?;
    attest_content(s, ctx, "artifact", &record.id, &record).await?;
    s.put_edge(ctx, "artifact", &record.id, "artifact", &host.id)
        .await?;
    s.put_edge(
        ctx,
        "artifact",
        &record.id,
        "artifact",
        &crate::streaming_evaluator::storage_id("independent_grader_receipt_v2", receipt_id)?,
    )
    .await?;
    Ok(())
}
pub(crate) async fn issue_complete_authority_in_session(
    s: &mut Session,
    ctx: &Context,
    ticket: &crate::streaming_evaluator::EvaluationTicketV2,
    control: &crate::streaming_evaluator::RegisteredEvaluationControl,
    formal: &crate::streaming_evaluator::FormalEvaluationV2,
) -> Result<()> {
    ctx.require(&[Role::Evaluator])?;
    if control.evidence_scope
        != crate::streaming_evaluator::EvaluationEvidenceScope::RegisteredExternalHost
    {
        return Err(Error::Forbidden);
    }
    let candidate: crate::release_store::ReleaseCandidateRecord =
        s.need(ctx, "artifact", &ticket.candidate_digest).await?;
    let inputs = resolve_bundle_in_session(
        s,
        ctx,
        &candidate.bundle,
        &candidate.environment_digest,
        candidate.revoke_watermark,
        Purpose::Development,
    )
    .await?;
    let host: RegisteredFormalHost = attested_read(
        s,
        ctx,
        "artifact",
        &inputs.profile.host_registration_id,
        Role::Admin,
    )
    .await?;
    host.validate(ctx)?;
    let set = verify_candidate_principals(s, ctx, &host.id, &candidate, control).await?;
    process_principal(ctx, &host, &set, Role::Evaluator)?;
    let report_storage_id =
        crate::streaming_evaluator::storage_id("formal_evaluation_v2", &ticket.id)?;
    record_grader_action(
        s,
        ctx,
        &host,
        "artifact",
        &report_storage_id,
        formal,
        "issue_report",
    )
    .await?;
    let report_principal_action_id =
        principal_action_id("artifact", &report_storage_id, Role::Evaluator)?;
    let observed = crate::executor::observe_registered_process(
        host.grader_uid,
        &host.grader_code_digest,
        &host.grader_workspace,
    )?;
    let seal = s
        .sealed_root_budget_evidence(ctx, &control.billing_scope)
        .await?
        .ok_or(Error::NotFound)?;
    if !seal.monetary_complete || !seal.unresolved.is_empty() {
        return Err(Error::Conflict("complete root cost unresolved".into()));
    }
    let cost: EpisodeCostRecord = attested_read(
        s,
        ctx,
        "artifact",
        &format!("formal-cost-{}", ticket.id),
        Role::Host,
    )
    .await?;
    cost.measurements.validate_complete()?;
    verify_refs(s, ctx, &cost.measurement_sources).await?;
    if cost.ticket_id != ticket.id
        || cost.profile_registration_id != inputs.profile.id
        || cost.observed_uid != host.host_uid
    {
        return Err(Error::Conflict("episode meter scope mismatch".into()));
    }
    let mut provider_captures = Vec::new();
    let mut grader_captures = Vec::new();
    if cost.measurements.episode_id != control.attempt_id
        || seal.root.billing_scope != control.billing_scope
    {
        return Err(Error::Conflict("episode/root identity differs".into()));
    }
    validate_episode_meter_sources(
        s,
        ctx,
        &inputs.profile,
        &host,
        ticket,
        control,
        &seal,
        &cost.measurements,
        &cost.measurement_sources,
    )
    .await?;
    let mut roots = vec![
        ("artifact".into(), candidate.id.clone()),
        ("artifact".into(), cost.id.clone()),
        (
            "artifact".into(),
            crate::streaming_evaluator::storage_id("formal_evaluation_v2", &ticket.id)?,
        ),
        (
            "artifact".into(),
            crate::streaming_evaluator::storage_id("evaluation_ticket_v2", &ticket.id)?,
        ),
        (
            "artifact".into(),
            crate::streaming_evaluator::storage_id(
                "registered_evaluation_control_v41",
                &ticket.registration_id,
            )?,
        ),
        (
            "artifact".into(),
            crate::streaming_evaluator::storage_id("protected_holdout_v41", &ticket.holdout_id)?,
        ),
        ("artifact".into(), set.id.clone()),
        ("artifact".into(), report_principal_action_id.clone()),
    ];
    for target in &ticket.targets {
        let crate::streaming_evaluator::TargetState::Completed {
            candidate_receipt_id,
            baseline_receipt_id,
            grader_receipt_id,
            ..
        } = &target.state
        else {
            return Err(Error::Conflict("formal target incomplete".into()));
        };
        for receipt_id in [candidate_receipt_id, baseline_receipt_id] {
            let receipt: crate::streaming_evaluator::ExecutionReceiptV2 =
                crate::streaming_evaluator::need_record(s, ctx, "execution_receipt_v2", receipt_id)
                    .await?;
            let call = s
                .budget_call(ctx, &control.billing_scope, &receipt.budget_call_id)
                .await?
                .ok_or(Error::NotFound)?;
            if call.execution_provenance
                != Some(evo_storage::budget::BudgetExecutionProvenance::ExternalProvider)
                || !call.execution_closed
                || call.state != evo_storage::budget::BudgetCallState::Finalized
                || call.dispatch_group_id != ticket.id
                || call.response_usable != Some(true)
            {
                return Err(Error::Forbidden);
            }
            let id = format!(
                "formal-capture-{}",
                &hash(call.dispatch_id.as_ref().ok_or(Error::NotFound)?.as_bytes())[..32]
            );
            let capture: FormalProviderCaptureRecord =
                attested_read(s, ctx, "artifact", &id, Role::Host).await?;
            verify_provider_registration(s, ctx, &capture, &host).await?;
            let request_artifact = call.request_artifact.as_ref().ok_or(Error::NotFound)?;
            if request_artifact.schema_version != "rsia.formal_task_request.v1" {
                return Err(Error::Forbidden);
            }
            let actual_request: crate::model::FormalTaskRequest =
                serde_json::from_str(&request_artifact.body).map_err(|_| Error::Forbidden)?;
            actual_request.validate()?;
            let applied_bundle =
                if receipt.side == crate::streaming_evaluator::ExecutionSide::Candidate {
                    candidate.bundle.clone()
                } else {
                    inputs.compile(
                        &SkillPatch {
                            content: evo_core::contract::PatchOp::ResetToBaseline,
                            applicability: evo_core::contract::PatchOp::ResetToBaseline,
                            counterexample: evo_core::contract::PatchOp::ResetToBaseline,
                            required_capabilities: evo_core::contract::PatchOp::ResetToBaseline,
                            dependencies: evo_core::contract::PatchOp::ResetToBaseline,
                        },
                        &ImproverPatch {
                            instruction: evo_core::contract::PatchOp::ResetToBaseline,
                            max_candidates: evo_core::contract::PatchOp::ResetToBaseline,
                            max_rounds: evo_core::contract::PatchOp::ResetToBaseline,
                        },
                    )?
                };
            let projection = evo_core::contract::project_run(
                &applied_bundle,
                &inputs.profile.system_snapshot,
                true,
            )?;
            let mut application = vec![
                serde_json::json!({"role":"system","content":actual_request.mandatory_context}),
            ];
            for instruction in &projection.instructions {
                application.push(serde_json::json!({"role":"system","content":instruction}));
            }
            application
                .push(serde_json::json!({"role":"user","content":actual_request.task_input_utf8}));
            if actual_request.candidate_id != candidate.id
                || actual_request.profile_registration_id != inputs.profile.id
                || actual_request.compiler_binding_digest != inputs.binding.binding_digest
                || actual_request.task.ticket_id != ticket.id
                || actual_request.task.target_id != target.target_id
                || actual_request.task.side != receipt.side
                || actual_request.task.request_digest != receipt.request_digest
                || actual_request.task.input_digest != target.input_digest
                || applied_bundle.digest != receipt.bundle_digest
                || hash(actual_request.mandatory_context.as_bytes())
                    != inputs.profile.system_snapshot.mandatory_context_digest
                || fingerprint(&actual_request.projection)? != fingerprint(&projection)?
                || fingerprint(&application)? != capture.application_digest
            {
                return Err(Error::Conflict(
                    "actual request application differs from frozen P/B".into(),
                ));
            }
            let transport = call.transport_artifact.as_ref().ok_or(Error::NotFound)?;
            if decode::<FormalProviderCaptureRecord>(
                serde_json::from_str(&transport.body).map_err(|_| Error::Internal)?,
            )?
            .id != capture.id
                || fingerprint(&capture)?
                    != fingerprint(&decode::<FormalProviderCaptureRecord>(
                        serde_json::from_str(&transport.body).map_err(|_| Error::Internal)?,
                    )?)?
                || capture.host_registration_id != host.id
                || capture.host_uid != host.host_uid
                || capture.host_pid == observed.pid
                || capture.host_principal != host.host_principal
                || capture.host_code_digest != host.code_digest
                || capture.host_workspace != host.host_workspace
                || capture.request_digest != receipt.request_digest
                || capture.bundle_digest != receipt.bundle_digest
                || capture.output_digest != receipt.output_digest
                || capture.input_digest != target.input_digest
                || call.actual_cost_micros != Some(capture.actual_cost_micros)
                || call.actual_currency.as_deref() != Some(capture.currency.as_str())
                || call.actual_pricing_version.as_deref() != Some(capture.pricing_version.as_str())
                || seal.root.currency != capture.currency
                || seal.root.pricing_version != capture.pricing_version
                || call.output_digest.as_deref() != Some(capture.output_digest.as_str())
                || call.provider_request_id.as_deref() != Some(capture.provider_request_id.as_str())
                || call.usage_record_id.as_deref() != Some(capture.usage_record_id.as_str())
            {
                return Err(Error::Conflict(
                    "actual provider/host/output/usage mismatch".into(),
                ));
            }
            let r = TypedContentRef {
                kind: "artifact".into(),
                id: id.clone(),
                schema_version: capture.schema_version.clone(),
                content_digest: content_digest(&capture)?,
            };
            provider_captures.push(r);
            roots.push(("artifact".into(), id));
            for kind_id in [
                ("execution_receipt_v2", receipt_id.as_str()),
                (
                    "evaluation_execution_output_v1",
                    receipt.output_artifact_id.as_str(),
                ),
            ] {
                roots.push((
                    "artifact".into(),
                    crate::streaming_evaluator::storage_id(kind_id.0, kind_id.1)?,
                ));
            }
        }
        let grade_id = format!(
            "formal-grade-{}",
            &fingerprint(&(ticket.id.as_str(), target.target_id.as_str()))?[..32]
        );
        let grade: IndependentGradeCapture =
            attested_read(s, ctx, "artifact", &grade_id, Role::Evaluator).await?;
        // The capture names the canonical digest of the stored receipt itself:
        // any changed field, or a capture of another digest, fails closed.
        let original: Value = crate::streaming_evaluator::need_record(
            s,
            ctx,
            "independent_grader_receipt_v2",
            grader_receipt_id,
        )
        .await?;
        if grade.grader_receipt_digest != evo_core::canonical_fingerprint(&original)?
            || grade.grader_uid != host.grader_uid
            || grade.grader_principal != host.grader_principal
            || grade.grader_code_digest != host.grader_code_digest
            || grade.grader_workspace != host.grader_workspace
            || grade.ticket_id != ticket.id
            || grade.target_id != target.target_id
        {
            return Err(Error::Forbidden);
        }
        grader_captures.push(TypedContentRef {
            kind: "artifact".into(),
            id: grade_id.clone(),
            schema_version: grade.schema_version.clone(),
            content_digest: content_digest(&grade)?,
        });
        roots.push(("artifact".into(), grade_id));
        roots.push((
            "artifact".into(),
            crate::streaming_evaluator::storage_id(
                "independent_grader_receipt_v2",
                grader_receipt_id,
            )?,
        ));
    }
    let closure = read_typed_closure(s, ctx, &roots).await?;
    let record = IssuedFormalAuthority {
        id: format!("formal-authority-{}", ticket.id),
        schema_version: FORMAL_SCHEMA.into(),
        report_id: ticket.id.clone(),
        raw_report_digest: fingerprint(formal)?,
        ticket_digest: fingerprint(ticket)?,
        candidate_id: candidate.id,
        candidate_bundle_digest: candidate.bundle_digest,
        environment_digest: candidate.environment_digest,
        compiler_binding: inputs.binding,
        billing_scope: control.billing_scope.clone(),
        root_seal_digest: fingerprint(&seal)?,
        episode_cost_digest: fingerprint(&cost)?,
        host_registration_id: host.id,
        principal_set_digest: set.binding_digest()?,
        report_principal_action_id,
        provider_captures,
        grader_captures,
        closure_roots: roots,
        closure,
        watermark: s.watermark(ctx).await?,
        issued_by: ctx.actor().into(),
    };
    immutable(s, ctx, "artifact", &record.id, &record).await?;
    attest_content(s, ctx, "artifact", &record.id, &record).await?;
    edges(s, ctx, "artifact", &record.id, &record.closure.nodes).await?;
    s.audit(ctx, "formal.authority.issue", &record.id).await
}
pub async fn verify_issued_authority(
    s: &mut Session,
    ctx: &Context,
    report_id: &str,
    candidate: &crate::release_store::ReleaseCandidateRecord,
) -> Result<IssuedFormalAuthority> {
    let record: IssuedFormalAuthority = attested_read(
        s,
        ctx,
        "artifact",
        &format!("formal-authority-{report_id}"),
        Role::Evaluator,
    )
    .await?;
    if record.schema_version != FORMAL_SCHEMA
        || record.report_id != report_id
        || record.candidate_id != candidate.id
        || record.candidate_bundle_digest != candidate.bundle_digest
        || record.environment_digest != candidate.environment_digest
        || record.watermark != s.watermark(ctx).await?
        || record.provider_captures.is_empty()
        || record.grader_captures.is_empty()
    {
        return Err(Error::Conflict(
            "unknown/stale formal authority; quarantine".into(),
        ));
    }
    verify_typed_closure(s, ctx, &record.closure_roots, &record.closure).await?;
    let formal: crate::streaming_evaluator::FormalEvaluationV2 =
        crate::streaming_evaluator::need_record(s, ctx, "formal_evaluation_v2", report_id).await?;
    let ticket: crate::streaming_evaluator::EvaluationTicketV2 =
        crate::streaming_evaluator::need_record(s, ctx, "evaluation_ticket_v2", report_id).await?;
    if record.raw_report_digest != fingerprint(&formal)?
        || record.ticket_digest != fingerprint(&ticket)?
    {
        return Err(Error::Conflict("issued report/ticket bytes differ".into()));
    }
    let seal = s
        .sealed_root_budget_evidence(ctx, &record.billing_scope)
        .await?
        .ok_or(Error::NotFound)?;
    if !seal.monetary_complete || fingerprint(&seal)? != record.root_seal_digest {
        return Err(Error::Conflict("root cost seal stale".into()));
    }
    let cost: EpisodeCostRecord = attested_read(
        s,
        ctx,
        "artifact",
        &format!("formal-cost-{report_id}"),
        Role::Host,
    )
    .await?;
    cost.measurements.validate_complete()?;
    if fingerprint(&cost)? != record.episode_cost_digest {
        return Err(Error::Conflict("episode cost changed".into()));
    }
    let control: crate::streaming_evaluator::RegisteredEvaluationControl =
        crate::streaming_evaluator::need_record(
            s,
            ctx,
            "registered_evaluation_control_v41",
            &ticket.registration_id,
        )
        .await?;
    let profile: FrozenCompilerProfile = attested_read(
        s,
        ctx,
        "artifact",
        &record.compiler_binding.profile_registration_id,
        Role::Admin,
    )
    .await?;
    let selection: ProfileSelection = attested_read(
        s,
        ctx,
        "artifact",
        &format!("formal-profile-{}", profile.profile_id),
        Role::Admin,
    )
    .await?;
    let host: RegisteredFormalHost = attested_read(
        s,
        ctx,
        "artifact",
        &record.host_registration_id,
        Role::Admin,
    )
    .await?;
    let set = verify_candidate_principals(s, ctx, &host.id, candidate, &control).await?;
    let report_storage_id =
        crate::streaming_evaluator::storage_id("formal_evaluation_v2", report_id)?;
    require_principal_action(
        s,
        ctx,
        &host,
        "artifact",
        &report_storage_id,
        &formal,
        Role::Evaluator,
    )
    .await?;
    if record.principal_set_digest != set.binding_digest()?
        || record.report_principal_action_id
            != principal_action_id("artifact", &report_storage_id, Role::Evaluator)?
    {
        return Err(Error::Forbidden);
    }
    if selection.registration_id != profile.id
        || fingerprint(&profile)? != record.compiler_binding.profile_digest
        || profile.host_registration_id != host.id
        || cost.profile_registration_id != profile.id
        || cost.ticket_id != ticket.id
        || record.billing_scope != control.billing_scope
    {
        return Err(Error::Forbidden);
    }
    verify_refs(s, ctx, &cost.measurement_sources).await?;
    validate_episode_meter_sources(
        s,
        ctx,
        &profile,
        &host,
        &ticket,
        &control,
        &seal,
        &cost.measurements,
        &cost.measurement_sources,
    )
    .await?;
    for r in &record.provider_captures {
        let capture: FormalProviderCaptureRecord =
            attested_read(s, ctx, &r.kind, &r.id, Role::Host).await?;
        verify_provider_registration(s, ctx, &capture, &host).await?;
    }
    let mut binding = record.compiler_binding.clone();
    let expected = std::mem::take(&mut binding.binding_digest);
    if fingerprint(&binding)? != expected
        || binding.parent_digest != candidate.parent_digest
        || binding.baseline_digest != candidate.bundle.baseline_digest
    {
        return Err(Error::Conflict("compiler input seal mismatch".into()));
    }
    verify_refs(s, ctx, &record.provider_captures).await?;
    verify_refs(s, ctx, &record.grader_captures).await?;
    Ok(record)
}

#[cfg(test)]
mod response_bound_tests {
    use super::*;

    fn audit_f02_control() -> crate::streaming_evaluator::RegisteredEvaluationControl {
        use crate::streaming_evaluator::{
            EvaluationEvidenceScope, FixedGraderMethod, FixedGraderSpec,
            RegisteredEvaluationControl,
        };
        use evo_core::evaluation::{
            ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec, FormalExperimentPlanV41,
            FormalStatisticalUnit, Money, OptimizationBudgetPlan, OptimizerComparisonContract,
            ProfileKind,
        };
        use evo_core::holdout::{AnchorCoverageMatrix, CriticalCapabilityCoverage};
        use evo_core::sequential::{AlphaAllocation, FormalClaimKind, ResearchFamilyAlphaPlan};
        let money = || Money {
            currency: "USD".into(),
            pricing_version: "pricing-v1".into(),
            amount: "0.001".into(),
        };
        let mut plan = ExperimentPlan::first_low_risk("audit-f02", 60).unwrap();
        plan.n_planned = 20;
        plan.profile = ProfileKind::QualityGain;
        plan.monetary_budget = money();
        plan.frozen = true;
        plan.frozen_at = Some(1);
        let alpha = ResearchFamilyAlphaPlan::new(
            "audit-family",
            "0.05",
            vec![AlphaAllocation {
                claim_id: "gain".into(),
                attempt_id: "attempt".into(),
                kind: FormalClaimKind::FixedSampleGain,
                alpha: "0.05".into(),
            }],
        )
        .unwrap();
        let anchors = AnchorCoverageMatrix::new(
            "quality-gain",
            "anchors-v1",
            vec![CriticalCapabilityCoverage {
                capability_id: "safety".into(),
                anchor_ids: vec!["anchor".into()],
            }],
        )
        .unwrap();
        let mut budget = OptimizationBudgetPlan::unfunded("billing", "payer", "USD").unwrap();
        budget.root_total = money();
        budget.admin_authorization_receipt_digest = Some(hash(b"authorization"));
        for stage in &mut budget.stages {
            stage.per_call_limit.pricing_version = "pricing-v1".into();
            stage.experiment_total.pricing_version = "pricing-v1".into();
        }
        let optimizer = |arm, name: &[u8]| FixedOptimizerSpec {
            arm,
            implementation_digest: hash(name),
            initial_s0_digest: hash(b"s0"),
            base_model_digest: hash(b"model"),
            tools_digest: hash(b"tools"),
            authorized_materials_digest: hash(b"materials"),
            task_partition_digest: hash(b"partition"),
            context_limit_tokens: 4096,
            root_budget_scope_id: "billing".into(),
            budget_limit: money(),
        };
        let comparison = OptimizerComparisonContract::new(
            optimizer(FixedOptimizerArm::CSimple, b"simple"),
            optimizer(FixedOptimizerArm::CSkillopt, b"skillopt"),
            FixedOptimizerArm::CSkillopt,
        )
        .unwrap();
        let formal = FormalExperimentPlanV41::new(
            &plan,
            hash(b"preregistration"),
            &alpha,
            &comparison,
            &budget,
            anchors.digest().unwrap(),
            hash(b"sampling"),
            hash(b"mean"),
            FormalStatisticalUnit::IndependentTaskCluster,
            20,
        )
        .unwrap();
        let grader = FixedGraderSpec {
            schema_version: FixedGraderSpec::SCHEMA.into(),
            version: "exact-v1".into(),
            method: FixedGraderMethod::ExactJsonAnswerV1,
        };
        let control = RegisteredEvaluationControl {
            schema_version: RegisteredEvaluationControl::SCHEMA.into(),
            id: "audit-control".into(),
            v1_plan_snapshot: plan,
            formal_plan: formal,
            alpha_plan: alpha,
            optimizer_comparison: comparison,
            optimization_budget: budget,
            early_stop_plan: None,
            anchor_matrix: anchors,
            attempt_id: "attempt".into(),
            fixed_sample_claim_id: "gain".into(),
            billing_scope: "billing".into(),
            executor_actor: "host".into(),
            evaluator_actor: "grader".into(),
            proposer_actor: "worker".into(),
            approver_actor: "admin".into(),
            oracle_digest: hash(b"oracle"),
            grader_digest: fingerprint(&grader).unwrap(),
            fixed_grader: grader,
            evidence_scope: EvaluationEvidenceScope::RegisteredExternalHost,
            created_at_unix_seconds: 2,
        };
        control.validate().unwrap();
        control
    }

    fn audit_f02_principals(uid: u32, actual_role: Role) -> RegisteredPrincipalSet {
        let principal = |uid, actor: &str, credential: &str| RegisteredPrincipal {
            uid,
            actor: actor.into(),
            principal: hash(credential.as_bytes()),
            credential_digest: hash(credential.as_bytes()),
        };
        let mut set = RegisteredPrincipalSet {
            id: "formal-principals-host".into(),
            schema_version: PRINCIPALS_SCHEMA.into(),
            namespace: "n".into(),
            host_registration_id: "host".into(),
            version: 1,
            host: principal(uid + 1, "host", "host-private-fixture-credential-32-bytes"),
            grader: principal(
                uid + 2,
                "grader",
                "grader-private-fixture-credential-32-bytes",
            ),
            administrator: principal(
                uid + 3,
                "admin",
                "administrator-private-fixture-credential-32-bytes",
            ),
            worker: principal(
                uid + 4,
                "worker",
                "worker-private-fixture-credential-32-bytes",
            ),
        };
        match actual_role {
            Role::Host => set.host.uid = uid,
            Role::Evaluator => set.grader.uid = uid,
            Role::Admin => set.administrator.uid = uid,
            Role::Worker => set.worker.uid = uid,
            _ => unreachable!(),
        }
        set
    }

    #[tokio::test]
    async fn audit_f02_external_control_rejects_context_without_authenticated_peer_before_budget() {
        use crate::streaming_evaluator::IndependentEvaluationControl;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let store = Store::open(&db).await.unwrap();
        let control = audit_f02_control();
        for (role, actor) in [
            (Role::Admin, "admin"),
            (Role::Worker, "worker"),
            (Role::Host, "host"),
            (Role::Evaluator, "grader"),
        ] {
            let ctx = Context::new("n", actor, role).unwrap();
            assert!(matches!(
                IndependentEvaluationControl::register_control(&ctx, &store, control.clone()).await,
                Err(Error::Forbidden)
            ));
            let mut s = store.session().await.unwrap();
            assert_eq!(s.namespace_object_count(&ctx, "artifact").await.unwrap(), 0);
            drop(s);
            // Reuse the existing Python/SQLite test tooling; the engine does
            // not add a database-driver dependency or bypass Store in product.
            let read = std::process::Command::new("python3")
                .args([
                    "-B", "-c",
                    "import json,sqlite3,sys,urllib.parse; c=sqlite3.connect('file:'+urllib.parse.quote(sys.argv[1],safe='/')+'?mode=ro',uri=True); print(json.dumps(c.execute('SELECT (SELECT COUNT(*) FROM objects),(SELECT COUNT(*) FROM audit),(SELECT COUNT(*) FROM dependencies),(SELECT COUNT(*) FROM root_budget_calls)').fetchone())); c.close()",
                ])
                .arg(&db)
                .output()
                .unwrap();
            assert!(
                read.status.success(),
                "{}",
                String::from_utf8_lossy(&read.stderr)
            );
            let counts: Vec<u64> = serde_json::from_slice(&read.stdout).unwrap();
            assert_eq!(counts, [0, 0, 0, 0]);
        }
        store.close().await;
    }

    #[tokio::test]
    async fn audit_f02_capability_binds_role_actor_namespace_peer_and_exact_command() {
        let (left, _right) = tokio::net::UnixStream::pair().unwrap();
        let peer = crate::executor::observe_kernel_peer(&left).unwrap();
        let set = audit_f02_principals(peer.uid(), Role::Evaluator);
        let control = audit_f02_control();
        let ctx = Context::new("n", "grader", Role::Evaluator).unwrap();
        let cap = OperatorCredential {
            namespace: "n".into(),
            actor: "grader".into(),
            role: Role::Evaluator,
            host_digest: hash(b"test-only-host-binding"),
            principals: set,
            peer: peer.clone(),
            // The pure capability preflight is tested here; no process
            // registration, persistence or formal issuance is claimed.
            authority: crate::executor::ObservedProcessIdentity {
                pid: std::process::id(),
                uid: peer.uid(),
                executable_digest: hash(b"test-authority"),
                workspace: "test-only".into(),
            },
            operation: "register_control".into(),
            request_digest: control_request_digest(&control).unwrap(),
        };
        OPERATOR_CREDENTIAL
            .scope(cap, async {
                authenticate_control_registration(&ctx, &control).unwrap();
                for bad_ctx in [
                    Context::new("n", "admin", Role::Admin).unwrap(),
                    Context::new("n", "other", Role::Evaluator).unwrap(),
                    Context::new("other", "grader", Role::Evaluator).unwrap(),
                ] {
                    assert!(matches!(
                        authenticate_control_registration(&bad_ctx, &control),
                        Err(Error::Forbidden)
                    ));
                }
                let mut changed = control.clone();
                changed.id.push_str("-changed");
                changed.validate().unwrap();
                assert!(matches!(
                    authenticate_control_registration(&ctx, &changed),
                    Err(Error::Forbidden)
                ));
            })
            .await;
    }

    #[test]
    fn audit_f02_control_witness_has_new_identity_and_preserves_legacy_wire_support() {
        let control = audit_f02_control();
        let id = crate::streaming_evaluator::storage_id(
            "registered_evaluation_control_v41",
            &control.id,
        )
        .unwrap();
        assert_ne!(
            control_principal_action_id("artifact", &id).unwrap(),
            principal_action_id("artifact", &id, Role::Evaluator).unwrap()
        );
        assert!(compiler_dependency_schema(
            "artifact",
            PRINCIPAL_ACTION_SCHEMA
        ));
        assert!(compiler_dependency_schema(
            "artifact",
            CONTROL_PRINCIPAL_ACTION_SCHEMA
        ));
        assert_ne!(
            control_request_digest(&control).unwrap(),
            content_digest(&control).unwrap()
        );
        let command = FormalOperatorCommand::RegisterControl {
            control: Box::new(control),
        };
        let wire = serde_json::to_value(&command).unwrap();
        let decoded: FormalOperatorCommand = serde_json::from_value(wire).unwrap();
        assert_eq!(
            command.authentication_digest().unwrap(),
            decoded.authentication_digest().unwrap()
        );
    }

    #[tokio::test]
    async fn audit_f02_admin_worker_and_host_cannot_authenticate_control_registration() {
        let (left, _right) = tokio::net::UnixStream::pair().unwrap();
        let peer = crate::executor::observe_kernel_peer(&left).unwrap();
        let command = FormalOperatorCommand::RegisterControl {
            control: Box::new(audit_f02_control()),
        };
        for (role, credential) in [
            (
                Role::Admin,
                "administrator-private-fixture-credential-32-bytes",
            ),
            (Role::Worker, "worker-private-fixture-credential-32-bytes"),
            (Role::Host, "host-private-fixture-credential-32-bytes"),
        ] {
            let set = audit_f02_principals(peer.uid(), role);
            assert!(
                matches!(
                    authenticate_operator_peer(&set, &peer, credential, &command),
                    Err(Error::Forbidden)
                ),
                "unauthorized role {role:?}"
            );
        }
    }

    #[tokio::test]
    async fn audit_f02_registered_evaluator_authenticates_control_with_own_peer_and_credential() {
        let (left, _right) = tokio::net::UnixStream::pair().unwrap();
        let peer = crate::executor::observe_kernel_peer(&left).unwrap();
        let set = audit_f02_principals(peer.uid(), Role::Evaluator);
        let command = FormalOperatorCommand::RegisterControl {
            control: Box::new(audit_f02_control()),
        };
        let principal = authenticate_operator_peer(
            &set,
            &peer,
            "grader-private-fixture-credential-32-bytes",
            &command,
        )
        .unwrap();
        assert_eq!(command.role(), Role::Evaluator);
        assert_eq!(principal.actor, "grader");
        assert_eq!(principal.uid, peer.uid());
        assert!(matches!(
            authenticate_operator_peer(
                &set,
                &peer,
                "administrator-private-fixture-credential-32-bytes",
                &command
            ),
            Err(Error::Forbidden)
        ));
        let mut wrong_uid = set.clone();
        wrong_uid.grader.uid += 10;
        assert!(matches!(
            authenticate_operator_peer(
                &wrong_uid,
                &peer,
                "grader-private-fixture-credential-32-bytes",
                &command
            ),
            Err(Error::Forbidden)
        ));
    }

    #[test]
    fn cumulative_provider_body_bound_precedes_extension_and_keeps_partial_capture() {
        let limit = 2 * 1024 * 1024;
        let mut body = Vec::new();
        append_bounded_body(&mut body, &vec![b'a'; limit - 1], limit).unwrap();
        append_bounded_body(&mut body, b"b", limit).unwrap();
        let retained_hash = hash(&body);
        assert!(append_bounded_body(&mut body, b"c", limit).is_err());
        assert_eq!(body.len(), limit);
        assert_eq!(hash(&body), retained_hash);
        assert_eq!(body.last(), Some(&b'b'));
    }

    #[test]
    fn one_oversized_chunk_and_meter_limit_refuse_without_allocating_the_chunk_into_capture() {
        let mut body = Vec::new();
        assert!(append_bounded_body(&mut body, &vec![b'x'; 512 * 1024 + 1], 512 * 1024).is_err());
        assert!(body.is_empty());
        assert_eq!(body.capacity(), 0);
        append_bounded_body(&mut body, b"known partial", 512 * 1024).unwrap();
        let retained = body.clone();
        assert!(append_bounded_body(&mut body, &vec![b'x'; 512 * 1024], 512 * 1024).is_err());
        assert_eq!(body, retained);
    }
}

#[cfg(test)]
mod principal_and_meter_repair_tests {
    use super::*;
    fn meter_reply() -> ResourceInstrumentationResponse {
        ResourceInstrumentationResponse {
            schema_version: "rsia.episode_resource_measurements.v1".into(),
            id: "reading".into(),
            request_digest: hash(b"request"),
            episode_id: "episode".into(),
            root_budget_id: "root".into(),
            root_snapshot_digest: hash(b"root"),
            instrumentation_code_digest: hash(b"meter"),
            complete_episode_root: true,
            cpu_nanoseconds: Some(1),
            storage_byte_seconds: Some(2),
            retained_history_bytes: Some(3),
            human_nanoseconds: Some(4),
            gray_release_nanoseconds: Some(5),
        }
    }
    #[test]
    fn original_meter_object_rejects_duplicate_unknown_array_and_trailing_fields() {
        let raw = serde_json::to_string(&meter_reply()).unwrap();
        let good: ResourceInstrumentationResponse =
            strict_raw_object(raw.as_bytes(), 512 * 1024).unwrap();
        assert_eq!(good.cpu_nanoseconds, Some(1));
        assert_eq!(good.human_nanoseconds, Some(4));
        for bad in [
            raw.replacen("{", "{\"cpu_nanoseconds\":7,", 1),
            raw.replacen("{", "{\"unknown_field\":7,", 1),
            format!("[{raw}]"),
            format!("{raw} true"),
            raw.replacen("\"id\":\"reading\",", "", 1),
        ] {
            assert!(
                strict_raw_object::<ResourceInstrumentationResponse>(bad.as_bytes(), 512 * 1024)
                    .is_err()
            );
        }
        assert!(
            strict_raw_object::<ResourceInstrumentationResponse>(raw.as_bytes(), raw.len() - 1)
                .is_err()
        );
    }
    fn records(raw: &str) -> Vec<ResourceMeasurementRecord> {
        [
            ("cpu", 1, "nanoseconds"),
            ("storage", 2, "byte_seconds"),
            ("history", 3, "bytes"),
            ("human", 4, "nanoseconds"),
            ("gray_release", 5, "nanoseconds"),
        ]
        .into_iter()
        .map(|(dimension, amount, unit)| ResourceMeasurementRecord {
            id: format!("formal-meter-ticket-{dimension}"),
            schema_version: "rsia.trusted_resource_meter.v1".into(),
            episode_id: "episode".into(),
            ticket_id: "ticket".into(),
            billing_scope: "scope".into(),
            root_snapshot_digest: hash(b"root"),
            host_registration_id: "host".into(),
            meter_registration_id: "meter".into(),
            meter_registration_digest: hash(b"meter"),
            observed_pid: std::process::id(),
            observed_uid: 0,
            dimension: dimension.into(),
            unit: unit.into(),
            amount,
            response_utf8: raw.into(),
            response_digest: hash(raw.as_bytes()),
            request_digest: hash(b"request"),
        })
        .collect()
    }
    fn measurements() -> CompleteEpisodeCostMeasurements {
        CompleteEpisodeCostMeasurements {
            episode_id: "episode".into(),
            cpu_nanoseconds: Some(1),
            storage_byte_seconds: Some(2),
            retained_history_bytes: Some(3),
            human_nanoseconds: Some(4),
            gray_release_nanoseconds: Some(5),
            measurement_sources: vec!["pending".into()],
        }
    }
    #[tokio::test]
    async fn late_cost_validation_failure_rolls_back_all_five_sources_witnesses_and_edges() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db")).await.unwrap();
        let host = Context::new("n", "host", Role::Host).unwrap();
        let raw = serde_json::to_string(&meter_reply()).unwrap();
        let records = records(&raw);
        let mut s = store.session().await.unwrap();
        for id in ["host", "meter"] {
            s.put(
                &host,
                "artifact",
                id,
                host.actor(),
                &serde_json::json!({"schema_version":"rsia.fixture.v1"}),
            )
            .await
            .unwrap();
        }
        s.commit().await.unwrap();
        let mut s = store.session().await.unwrap();
        // The absent profile makes final cost validation fail after the five
        // source writes. Dropping is the production capture's error path.
        assert!(
            record_meter_sources_and_cost_in_session(
                &mut s,
                &host,
                "ticket",
                "absent-profile",
                measurements(),
                &records
            )
            .await
            .is_err()
        );
        drop(s);
        let mut s = store.session().await.unwrap();
        for record in &records {
            assert!(
                s.get::<Value>(&host, "artifact", &record.id)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                s.dependency_edges(&host, "artifact", &record.id)
                    .await
                    .unwrap()
                    .is_empty()
            );
            let op = format!(
                "formal.host.content.v1.{}",
                fingerprint(&(host.namespace(), "artifact", record.id.as_str())).unwrap()
            );
            let entity = fingerprint(&(
                host.namespace(),
                "artifact",
                record.id.as_str(),
                content_digest(record).unwrap(),
            ))
            .unwrap();
            assert!(
                !s.has_role_audit_witness(&host, Role::Host, &op, &entity)
                    .await
                    .unwrap()
            );
            // A healthy typed reading for the same id is not poisoned by an
            // abandoned immutable source. This is no formal-cost/P approval.
            immutable(&mut s, &host, "artifact", &record.id, record)
                .await
                .unwrap();
        }
        assert!(
            s.get::<Value>(&host, "artifact", "formal-cost-ticket")
                .await
                .unwrap()
                .is_none()
        );
        drop(s);
        store.close().await;
    }
    fn principal(uid: u32, actor: &str, credential: &str) -> RegisteredPrincipal {
        RegisteredPrincipal {
            uid,
            actor: actor.into(),
            principal: hash(credential.as_bytes()),
            credential_digest: hash(credential.as_bytes()),
        }
    }
    #[tokio::test]
    async fn real_kernel_peer_plus_private_credential_authenticates_only_the_fixed_subject_operation()
     {
        let (left, right) = tokio::net::UnixStream::pair().unwrap();
        let peer = crate::executor::observe_kernel_peer(&left).unwrap();
        let uid = peer.uid();
        let secret = "administrator-private-fixture-credential-32-bytes";
        let other = uid.checked_add(1).unwrap();
        let set = RegisteredPrincipalSet {
            id: "formal-principals-host".into(),
            schema_version: PRINCIPALS_SCHEMA.into(),
            namespace: "n".into(),
            host_registration_id: "host".into(),
            version: 1,
            host: principal(other, "host", "host-private-fixture-credential-32-bytes"),
            grader: principal(
                other + 1,
                "grader",
                "grader-private-fixture-credential-32-bytes",
            ),
            administrator: principal(uid, "admin", secret),
            worker: principal(
                other + 2,
                "worker",
                "worker-private-fixture-credential-32-bytes",
            ),
        };
        let actual =
            authenticate_operator_peer(&set, &peer, secret, &FormalOperatorCommand::Bootstrap)
                .unwrap();
        assert_eq!(actual.uid, uid);
        assert_eq!(actual.actor, "admin");
        assert!(
            authenticate_operator_peer(
                &set,
                &peer,
                "wrong-private-fixture-credential-32-bytes",
                &FormalOperatorCommand::Bootstrap
            )
            .is_err()
        );
        let mut claimed_worker = set.clone();
        claimed_worker.administrator = set.worker.clone();
        assert!(
            authenticate_operator_peer(
                &claimed_worker,
                &peer,
                "worker-private-fixture-credential-32-bytes",
                &FormalOperatorCommand::Bootstrap
            )
            .is_err()
        );
        let raw=br#"{"credential":"private","uid":0,"role":"admin","command":{"operation":"bootstrap"}}"#;
        assert!(strict_raw_object::<OperatorEnvelope>(raw, 1024 * 1024).is_err());
        assert!(OPERATOR_CREDENTIAL.try_with(|_| ()).is_err());
        drop(right);
        // This is actual local peer authentication only, not four observed OS
        // processes, a provider run, a formal report or ordinary-P acceptance.
    }
    #[test]
    fn no_role_actor_or_json_field_can_deserialize_an_operator_capability() {
        assert!(strict_raw_object::<OperatorEnvelope>(br#"{"credential":"x","command":{"operation":"bootstrap","role":"admin","uid":501}}"#,1024*1024).is_err());
        assert!(OPERATOR_CREDENTIAL.try_with(|_| ()).is_err());
    }

    #[test]
    fn operator_commands_keep_the_original_wire_and_reject_nested_unknown_or_duplicate_fields() {
        let bootstrap: OperatorEnvelope = strict_raw_object(
            br#"{"credential":"private","command":{"operation":"bootstrap"}}"#,
            1024 * 1024,
        )
        .unwrap();
        assert!(matches!(
            &bootstrap.command,
            FormalOperatorCommand::Bootstrap
        ));
        assert_eq!(
            serde_json::to_string(&bootstrap.command).unwrap(),
            r#"{"operation":"bootstrap"}"#
        );
        assert_eq!(
            bootstrap.command.authentication_digest().unwrap(),
            FormalOperatorCommand::Bootstrap
                .authentication_digest()
                .unwrap()
        );
        let canary: OperatorEnvelope = strict_raw_object(
            br#"{"credential":"private","command":{"operation":"canary","release_id":"release-a"}}"#,
            1024 * 1024,
        ).unwrap();
        assert_eq!(
            serde_json::to_string(&canary.command).unwrap(),
            r#"{"operation":"canary","release_id":"release-a"}"#
        );
        assert_eq!(
            canary.command.authentication_digest().unwrap(),
            FormalOperatorCommand::Canary {
                release_id: "release-a".into()
            }
            .authentication_digest()
            .unwrap()
        );
        let invalid: &[&[u8]] = &[
            br#"{"credential":"private","command":{"role":"admin","operation":"bootstrap"}}"#,
            br#"{"credential":"private","command":{"operation":"bootstrap","actor":"admin"}}"#,
            br#"{"credential":"private","command":{"operation":"bootstrap","uid":0}}"#,
            br#"{"credential":"private","command":{"operation":"bootstrap","operation":"bootstrap"}}"#,
            br#"{"credential":"private","credential":"other","command":{"operation":"bootstrap"}}"#,
            br#"{"credential":"private","command":{"operation":"canary","release_id":"release-a","release_id":"release-b"}}"#,
            br#"{"credential":"private","command":{"operation":"canary","release_id":"release-a","uid":0}}"#,
            br#"{"credential":"private","command":["bootstrap"]}"#,
            br#"["private",{"operation":"bootstrap"}]"#,
            br#"{"credential":"private","command":{"operation":"bootstrap"}} {}"#,
        ];
        for raw in invalid {
            assert!(
                strict_raw_object::<OperatorEnvelope>(raw, 1024 * 1024).is_err(),
                "accepted invalid operator fixture: {}",
                String::from_utf8_lossy(raw),
            );
        }
        assert!(
            serde_json::from_slice::<FormalOperatorCommand>(
                br#"{"operation":"bootstrap","role":"admin"}"#
            )
            .is_err()
        );
        assert!(OPERATOR_CREDENTIAL.try_with(|_| ()).is_err());
    }

    #[test]
    fn ag129_operator_clock_wire_is_closed_and_ordinary_responses_stay_exact() {
        // This unit sample validates the boundary; it is not a real host clock failure.
        let clock = evo_core::clock::unix_seconds_at(
            std::time::UNIX_EPOCH - std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        let packet = operator_response(&Err(clock)).unwrap();
        assert_eq!(
            packet,
            serde_json::json!({"status":"refused","error":{
                "schema_version":"rsia.formal_operator_refusal.v1","code":"clock_regressed"
            }})
        );
        assert_eq!(
            decode_formal_operator_response(&serde_json::to_vec(&packet).unwrap())
                .unwrap_err()
                .to_string(),
            "clock_regressed"
        );
        let value = serde_json::json!({"old":"value","counter":7});
        let healthy = operator_response(&Ok(value.clone())).unwrap();
        assert_eq!(
            healthy,
            serde_json::json!({"status":"complete","value":value})
        );
        assert_eq!(
            decode_formal_operator_response(&serde_json::to_vec(&healthy).unwrap()).unwrap(),
            healthy
        );
        for error in [
            Error::Forbidden,
            Error::NotFound,
            Error::Internal,
            Error::Budget,
            Error::Cancelled,
            Error::Conflict("old error".into()),
            Error::Invalid("clock_regressed is only text".into()),
        ] {
            let refused = operator_response(&Err(error)).unwrap();
            assert_eq!(refused, serde_json::json!({"status":"refused"}));
            assert!(matches!(
                decode_formal_operator_response(&serde_json::to_vec(&refused).unwrap()),
                Err(Error::Forbidden)
            ));
        }
    }

    #[test]
    fn ag129_operator_never_maps_text_or_malformed_clock_packets_to_clock() {
        for bytes in [
            br#"{"status":"refused","reason":"clock_regressed"}"#.as_slice(),
            br#"{"status":"refused","error":{"code":"clock_regressed"}}"#,
            br#"{"status":"refused","error":{"schema_version":"other","code":"clock_regressed"}}"#,
            br#"{"status":"refused","error":{"schema_version":"rsia.formal_operator_refusal.v1","code":"other"}}"#,
            br#"{"status":"refused","error":{"schema_version":"rsia.formal_operator_refusal.v1","code":"clock_regressed","message":"x"}}"#,
            br#"{"status":"refused","error":{"schema_version":"rsia.formal_operator_refusal.v1","code":"clock_regressed"},"extra":0}"#,
            br#"{"status":"refused","status":"refused","error":{"schema_version":"rsia.formal_operator_refusal.v1","code":"clock_regressed"}}"#,
            br#"{"status":"refused","error":{"schema_version":"rsia.formal_operator_refusal.v1","code":"clock_regressed","code":"clock_regressed"}}"#,
        ] {
            assert!(matches!(
                decode_formal_operator_response(bytes),
                Err(Error::Forbidden)
            ));
        }
    }

    struct BrokenPipeOperatorResponse {
        successful_writes_before_failure: usize,
        writes: usize,
    }
    impl tokio::io::AsyncWrite for BrokenPipeOperatorResponse {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            self.writes += 1;
            if self.writes <= self.successful_writes_before_failure {
                std::task::Poll::Ready(Ok(bytes.len()))
            } else {
                std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
            }
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn ag129_r8_clock_refusal_survives_broken_pipe_response() {
        // Injected native boundary unit: neither a real system bad clock nor a host run.
        for successful_writes_before_failure in [0, 1] {
            let clock = evo_core::clock::unix_seconds_at(
                std::time::UNIX_EPOCH - std::time::Duration::from_secs(1),
            )
            .unwrap_err();
            let result = Err(clock);
            let mut stream = BrokenPipeOperatorResponse {
                successful_writes_before_failure,
                writes: 0,
            };
            let response = write_operator_response(&mut stream, &result).await;
            assert!(matches!(&response, Err(Error::Internal)));
            assert_eq!(stream.writes, successful_writes_before_failure + 1);
            let refused = operator_result_after_response(result, response).unwrap_err();
            assert!(matches!(&refused, Error::ClockRegressed));
            assert_eq!(refused.to_string(), "clock_regressed");
        }
    }

    #[tokio::test]
    async fn ag129_r8_ordinary_response_io_failure_is_not_swallowed() {
        // The fixed BrokenPipe writer is a unit fixture, not a real host/provider receipt.
        for successful_writes_before_failure in [0, 1] {
            for result in [
                Ok(serde_json::json!({"old":"value","counter":7})),
                Err(Error::Forbidden),
                Err(Error::Budget),
                Err(Error::Cancelled),
                Err(Error::Invalid("clock_regressed is only text".into())),
            ] {
                let mut stream = BrokenPipeOperatorResponse {
                    successful_writes_before_failure,
                    writes: 0,
                };
                let response = write_operator_response(&mut stream, &result).await;
                assert!(matches!(&response, Err(Error::Internal)));
                assert_eq!(stream.writes, successful_writes_before_failure + 1);
                assert!(matches!(
                    operator_result_after_response(result, response),
                    Err(Error::Internal)
                ));
            }
        }
    }
}
