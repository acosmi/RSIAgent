//! Persistent release control. Production transitions consume stored evidence by id.

use crate::evidence::{load_stored_source, validate_stored_sources};
use crate::hosts::CLAUDE_CODE_TARGET;
use crate::releases::validate_resolved_bundle_identity;
use crate::streaming_evaluator::{
    AnchorEvidenceStatus, CostEvidenceScope, DependencyEvidenceStatus, EvaluationEvidenceScope,
    VerifiedReportVariant, verified_report_view_for_host_in_session,
    verified_report_view_in_session,
};
use evo_core::contract::{
    AppliedReceipt, CapabilityLevel, HostCapabilities, HostSurfaceDeclarationV2,
    HostSurfaceManifest, HostSurfaceManifestV2, HostSurfacePredecessorV2,
    HostSurfaceRegistrationV2, HostSurfaceUpgradeDiffV2, ResolvedBundle,
    ReviewedHostSourceEnvelopeV1, RunProjection, SystemSnapshot, host_surface_upgrade_diff_v2,
    host_surface_v2_allows_field, host_surface_v2_from_source,
};
use evo_core::evaluation::Verdict;
use evo_core::{Context, Error, Result, Role, Validate, fingerprint, identifier};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const RELEASE_CANDIDATE_SCHEMA: &str = "rsia.release_candidate.v1";
pub const PERSISTENT_RELEASE_SCHEMA: &str = "rsia.persistent_release.v1";
pub const PROFILE_POINTER_SCHEMA: &str = "rsia.profile_pointer.v1";
pub const HOST_SURFACE_RECORD_SCHEMA: &str = "rsia.host_surface_record.v1";
pub const RUN_APPLICATION_SCHEMA: &str = "rsia.run_application.v1";
pub const HOST_APPLICATION_SCHEMA: &str = "rsia.host_application.v1";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedSourceRef {
    pub kind: String,
    pub id: String,
    pub content_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageBundleRequest {
    pub candidate_id: String,
    pub bundle: ResolvedBundle,
    pub environment_digest: String,
    pub proposer_actor: String,
    pub sources: Vec<TypedSourceRef>,
    pub revoke_watermark: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseCandidateRecord {
    pub id: String,
    pub schema_version: String,
    pub bundle: ResolvedBundle,
    pub bundle_digest: String,
    pub environment_digest: String,
    pub profile_id: String,
    pub parent_digest: String,
    pub proposer_actor: String,
    pub sources: Vec<TypedSourceRef>,
    pub revoke_watermark: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistentReleaseState {
    Approved,
    Canary,
    Active,
    Superseded,
    RolledBack,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistentRelease {
    pub id: String,
    pub schema_version: String,
    pub candidate_id: String,
    pub bundle_digest: String,
    pub environment_digest: String,
    pub profile_id: String,
    pub parent_digest: String,
    pub baseline_bundle_digest: String,
    pub report_id: String,
    pub report_digest: String,
    pub proposer_actor: String,
    pub evaluator_actor: String,
    pub approved_by: String,
    pub expected_parent_release_id: Option<String>,
    pub expected_pointer_epoch: u64,
    pub state: PersistentReleaseState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistentProfilePointer {
    pub id: String,
    pub schema_version: String,
    pub profile_id: String,
    pub active_release_id: Option<String>,
    pub active_bundle_digest: Option<String>,
    pub epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSurfaceRecord {
    pub id: String,
    #[serde(deserialize_with = "deserialize_host_surface_record_schema")]
    pub schema_version: String,
    pub manifest: HostSurfaceManifest,
    pub manifest_digest: String,
    pub extracted: Vec<String>,
}

// Validate every typed read while leaving v1 serialized fields and order
// unchanged. Historical conflicting v1 bodies remain readable; their manifest
// validation at prepare/live use refuses them without rewriting the record.
fn deserialize_host_surface_record_schema<'de, D>(
    deserializer: D,
) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let schema = String::deserialize(deserializer)?;
    if schema != HOST_SURFACE_RECORD_SCHEMA {
        return Err(serde::de::Error::custom(
            "unsupported host surface record schema",
        ));
    }
    Ok(schema)
}

pub const HOST_SURFACE_RECORD_SCHEMA_V2: &str = "rsia.host_surface_record.v2";
pub const RUN_APPLICATION_SCHEMA_V2: &str = "rsia.run_application.v2";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSurfaceRecordV2 {
    pub id: String,
    #[serde(deserialize_with = "deserialize_host_surface_record_v2_schema")]
    pub schema_version: String,
    pub manifest: HostSurfaceManifestV2,
    pub manifest_digest: String,
    pub source: ReviewedHostSourceEnvelopeV1,
    pub extracted: Vec<String>,
    pub extracted_typed: Vec<HostSurfaceDeclarationV2>,
    pub predecessor_surface_id: Option<String>,
    pub upgrade_diff: HostSurfaceUpgradeDiffV2,
}

fn deserialize_host_surface_record_v2_schema<'de, D>(
    deserializer: D,
) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value != HOST_SURFACE_RECORD_SCHEMA_V2 {
        return Err(serde::de::Error::custom(
            "unsupported current host surface schema",
        ));
    }
    Ok(value)
}

/// Source equality validates content only. Actual callers also re-read the
/// predecessor and require the original Admin content/registered authority.
pub fn validate_host_record_v2_source(record: &HostSurfaceRecordV2) -> Result<()> {
    identifier(&record.id)?;
    if record.schema_version != HOST_SURFACE_RECORD_SCHEMA_V2
        || record.predecessor_surface_id.as_deref() == Some(record.id.as_str())
        || record.manifest_digest != fingerprint(&record.manifest)?
    {
        return Err(Error::Conflict(
            "current host surface identity/source differs".into(),
        ));
    }
    if let Some(id) = &record.predecessor_surface_id {
        identifier(id)?;
    }
    evo_core::contract::validate_host_surface_v2_source(
        &record.manifest,
        &record.source,
        &record.extracted,
        &record.extracted_typed,
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunApplicationSnapshotV2 {
    pub id: String,
    #[serde(deserialize_with = "deserialize_run_application_v2_schema")]
    pub schema_version: String,
    pub run_id: String,
    pub profile_id: String,
    pub pointer_epoch: u64,
    pub release_id: Option<String>,
    pub bundle_digest: Option<String>,
    pub environment_digest: String,
    pub system_snapshot: SystemSnapshot,
    pub host_surface_id: String,
    pub host_surface_digest: String,
    pub host_capabilities_digest: String,
    pub host_capabilities: HostCapabilities,
    pub projection: RunProjection,
    pub request_material: AppliedRequestMaterial,
    pub request_digest: String,
    pub capability_level: CapabilityLevel,
    pub evolution_enabled: bool,
}

fn deserialize_run_application_v2_schema<'de, D>(
    deserializer: D,
) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value != RUN_APPLICATION_SCHEMA_V2 {
        return Err(serde::de::Error::custom(
            "unsupported current run application schema",
        ));
    }
    Ok(value)
}

pub(crate) fn validate_run_application_v2_binding(record: &RunApplicationSnapshotV2) -> Result<()> {
    identifier(&record.run_id)?;
    identifier(&record.profile_id)?;
    identifier(&record.host_surface_id)?;
    validate_digest(
        &record.request_material.task_input_digest,
        "task input digest",
    )?;
    for capability in record
        .host_capabilities
        .available
        .iter()
        .chain(&record.host_capabilities.granted)
    {
        identifier(capability)?;
    }
    if record.schema_version != RUN_APPLICATION_SCHEMA_V2
        || record.id != format!("run-snapshot-{}", record.run_id)
        || record.host_capabilities_digest != fingerprint(&record.host_capabilities)?
        || record.system_snapshot.profile_id != record.profile_id
        || record.environment_digest != record.system_snapshot.digest()?
        || record.request_digest != fingerprint(&record.request_material)?
        || record.request_material.instructions != record.projection.instructions
        || record.request_material.tools != record.projection.tools
        || record.request_material.mandatory_context_digest
            != record.system_snapshot.mandatory_context_digest
    {
        return Err(Error::Conflict(
            "current run source/capability binding differs".into(),
        ));
    }
    Ok(())
}

/// Non-serializable, read-only field view. It cannot stand in for either wire.
#[derive(Debug, Clone)]
pub struct RunApplicationView {
    pub id: String,
    pub schema_version: String,
    pub run_id: String,
    pub profile_id: String,
    pub pointer_epoch: u64,
    pub release_id: Option<String>,
    pub bundle_digest: Option<String>,
    pub environment_digest: String,
    pub system_snapshot: SystemSnapshot,
    pub host_surface_id: String,
    pub host_surface_digest: String,
    pub host_capabilities_digest: String,
    pub projection: RunProjection,
    pub request_material: AppliedRequestMaterial,
    pub request_digest: String,
    pub capability_level: CapabilityLevel,
    pub evolution_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RunApplicationWire {
    Legacy(RunApplicationSnapshot),
    Current(RunApplicationSnapshotV2),
}

/// Wire and cached view are constructed together and never exposed mutably.
/// fingerprint()/Session serialization always see the complete original wire.
#[derive(Debug, Clone)]
pub struct LoadedRunApplicationSnapshot {
    wire: RunApplicationWire,
    view: RunApplicationView,
}

impl LoadedRunApplicationSnapshot {
    fn from_wire(wire: RunApplicationWire) -> Result<Self> {
        match &wire {
            RunApplicationWire::Legacy(value) if value.schema_version != RUN_APPLICATION_SCHEMA => {
                return Err(Error::Invalid(
                    "unsupported legacy run application schema".into(),
                ));
            }
            RunApplicationWire::Current(value) => validate_run_application_v2_binding(value)?,
            _ => {}
        }
        macro_rules! field_view {
            ($value:expr) => {{
                let value = $value;
                RunApplicationView {
                    id: value.id.clone(),
                    schema_version: value.schema_version.clone(),
                    run_id: value.run_id.clone(),
                    profile_id: value.profile_id.clone(),
                    pointer_epoch: value.pointer_epoch,
                    release_id: value.release_id.clone(),
                    bundle_digest: value.bundle_digest.clone(),
                    environment_digest: value.environment_digest.clone(),
                    system_snapshot: value.system_snapshot.clone(),
                    host_surface_id: value.host_surface_id.clone(),
                    host_surface_digest: value.host_surface_digest.clone(),
                    host_capabilities_digest: value.host_capabilities_digest.clone(),
                    projection: value.projection.clone(),
                    request_material: value.request_material.clone(),
                    request_digest: value.request_digest.clone(),
                    capability_level: value.capability_level,
                    evolution_enabled: value.evolution_enabled,
                }
            }};
        }
        let view = match &wire {
            RunApplicationWire::Legacy(value) => field_view!(value),
            RunApplicationWire::Current(value) => field_view!(value),
        };
        Ok(Self { wire, view })
    }
    pub fn current_v2(&self) -> Option<&RunApplicationSnapshotV2> {
        match &self.wire {
            RunApplicationWire::Current(value) => Some(value),
            _ => None,
        }
    }
}

impl std::ops::Deref for LoadedRunApplicationSnapshot {
    type Target = RunApplicationView;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}

impl Serialize for LoadedRunApplicationSnapshot {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LoadedRunApplicationSnapshot {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let wire = RunApplicationWire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(serde::de::Error::custom)
    }
}
#[derive(Debug, Clone)]
pub struct PrepareRunRequest {
    pub run_id: String,
    pub profile_id: String,
    pub system_snapshot: SystemSnapshot,
    pub host_surface_id: String,
    pub host_capabilities: HostCapabilities,
    pub task_input_digest: String,
    pub evolution_enabled: bool,
    pub capability_level: CapabilityLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedRequestMaterial {
    pub mandatory_context_digest: String,
    pub task_input_digest: String,
    pub instructions: Vec<String>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunApplicationSnapshot {
    pub id: String,
    pub schema_version: String,
    pub run_id: String,
    pub profile_id: String,
    pub pointer_epoch: u64,
    pub release_id: Option<String>,
    pub bundle_digest: Option<String>,
    pub environment_digest: String,
    pub system_snapshot: SystemSnapshot,
    pub host_surface_id: String,
    pub host_surface_digest: String,
    pub host_capabilities_digest: String,
    pub projection: RunProjection,
    pub request_material: AppliedRequestMaterial,
    pub request_digest: String,
    pub capability_level: CapabilityLevel,
    pub evolution_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct TrustedHostExecutionEvidence {
    pub actual_request_material: AppliedRequestMaterial,
    pub environment_digest: String,
    pub host_surface_digest: String,
    pub host_capabilities_digest: String,
    pub offered: Vec<String>,
    pub attached: Vec<String>,
    pub used: Vec<String>,
    pub execution_receipt_id: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostApplicationRecord {
    pub id: String,
    pub schema_version: String,
    pub run_id: String,
    pub release_id: Option<String>,
    pub actual_request_digest: String,
    pub execution_receipt_id: Option<String>,
    pub receipt: Option<AppliedReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedHostExecutionReceipt {
    pub id: String,
    pub schema_version: String,
    pub run_id: String,
    pub request_digest: String,
    pub environment_digest: String,
    pub host_surface_digest: String,
    pub output_digest: String,
    pub used_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedCompilerBinding {
    pub id: String,
    pub schema_version: String,
    pub candidate_id: String,
    pub binding: crate::formal_authority::FormalCompilerInputBinding,
}

pub struct ReleaseStore;

impl ReleaseStore {
    /// A read-only Admin gate; staging and release transitions retain their
    /// existing identity rules. Reviewers need not be the candidate's proposer.
    pub(crate) async fn read_review_candidate_in_session(
        ctx: &Context,
        session: &mut Session,
        id: &str,
    ) -> Result<ReleaseCandidateRecord> {
        ctx.require(&[Role::Admin])?;
        identifier(id)?;
        let candidate: ReleaseCandidateRecord = session.need(ctx, "artifact", id).await?;
        if candidate.schema_version != RELEASE_CANDIDATE_SCHEMA {
            return Err(Error::NotFound);
        }
        if candidate.id != id
            || candidate.bundle_digest != candidate.bundle.digest
            || candidate.profile_id != candidate.bundle.profile_id
            || candidate.parent_digest != candidate.bundle.parent_digest
            || candidate.sources.is_empty()
        {
            return Err(Error::Invalid("invalid review candidate material".into()));
        }
        identifier(&candidate.profile_id)?;
        identifier(&candidate.proposer_actor)?;
        validate_digest(&candidate.bundle_digest, "bundle digest")?;
        validate_digest(&candidate.environment_digest, "environment digest")?;
        validate_digest(&candidate.parent_digest, "parent digest")?;
        validate_digest(&candidate.bundle.baseline_digest, "baseline digest")?;
        validate_resolved_bundle_identity(&candidate.bundle)?;
        crate::formal_authority::require_builtin_parent_strategy(&candidate.bundle.improver)?;
        candidate.bundle.skill.validate()?;
        if canonical_sources(candidate.sources.clone())? != candidate.sources {
            return Err(Error::Invalid("noncanonical review sources".into()));
        }
        validate_candidate_sources(session, ctx, &candidate).await?;
        Ok(candidate)
    }

    pub async fn stage_bundle(
        ctx: &Context,
        store: &Store,
        request: StageBundleRequest,
    ) -> Result<ReleaseCandidateRecord> {
        let mut session = store.session().await?;
        let candidate = Self::stage_bundle_in_session(ctx, &mut session, request).await?;
        session.commit().await?;
        Ok(candidate)
    }

    pub(crate) async fn stage_bundle_in_session(
        ctx: &Context,
        session: &mut Session,
        request: StageBundleRequest,
    ) -> Result<ReleaseCandidateRecord> {
        ctx.require(&[Role::Worker, Role::Admin])?;
        validate_stage_request(ctx, &request)?;
        let candidate = ReleaseCandidateRecord {
            id: request.candidate_id,
            schema_version: RELEASE_CANDIDATE_SCHEMA.into(),
            bundle_digest: request.bundle.digest.clone(),
            environment_digest: request.environment_digest,
            profile_id: request.bundle.profile_id.clone(),
            parent_digest: request.bundle.parent_digest.clone(),
            proposer_actor: request.proposer_actor,
            sources: canonical_sources(request.sources)?,
            revoke_watermark: request.revoke_watermark,
            bundle: request.bundle,
        };
        let existing = session
            .get::<ReleaseCandidateRecord>(ctx, "artifact", &candidate.id)
            .await?;
        if let Some(existing) = &existing
            && fingerprint(&existing)? != fingerprint(&candidate)?
        {
            return Err(Error::Conflict(
                "candidate id reused with different content".into(),
            ));
        }
        if let Some(existing) = existing {
            validate_candidate_read_in_session(session, ctx, &existing).await?;
            return Ok(existing);
        }
        Err(Error::Conflict(
            "legacy stage cannot create a new candidate; use Current source-bound staging".into(),
        ))
    }

    pub async fn approve_verified(
        ctx: &Context,
        store: &Store,
        candidate_id: &str,
        report_id: &str,
    ) -> Result<PersistentRelease> {
        ctx.require(&[Role::Admin])?;
        identifier(candidate_id)?;
        identifier(report_id)?;
        let mut session = store.session().await?;
        let candidate: ReadCandidate = session.need(ctx, "artifact", candidate_id).await?;
        if candidate.proposer_actor == ctx.actor() {
            return Err(Error::Forbidden);
        }
        validate_read_candidate_sources(&mut session, ctx, &candidate).await?;
        let read = crate::streaming_evaluator::read_verified_formal_report_in_session(ctx, &mut session, report_id).await?;
        // A historical report first retains all original negative diagnostics.
        // Healthy legacy material then refuses every new approval before DML.
        validate_read_verified_report_identity(&candidate, read.historical_view())?;
        validate_read_approval_actor(ctx, &candidate, read.historical_view())?;
        if !candidate.is_current() {
            return Err(Error::Conflict("source_legacy: formal report is not Current input".into()));
        }
        candidate.current()?;
        let view=read.current_view()?.clone();
        let pointer = load_pointer(&mut session, ctx, &candidate.profile_id).await?;
        let release_id = format!(
            "release-{}",
            &fingerprint(&(candidate_id, report_id, &view.report_digest))?[..32]
        );
        let release = PersistentRelease {
            id: release_id.clone(),
            schema_version: PERSISTENT_RELEASE_SCHEMA.into(),
            candidate_id: candidate.id.clone(),
            bundle_digest: candidate.bundle_digest.clone(),
            environment_digest: candidate.environment_digest.clone(),
            profile_id: candidate.profile_id.clone(),
            parent_digest: candidate.parent_digest.clone(),
            baseline_bundle_digest: view.baseline_bundle_digest.clone(),
            report_id: view.report_id,
            report_digest: view.report_digest,
            proposer_actor: candidate.proposer_actor.clone(),
            evaluator_actor: view.evaluator_actor,
            approved_by: ctx.actor().into(),
            expected_parent_release_id: pointer.active_release_id,
            expected_pointer_epoch: pointer.epoch,
            state: PersistentReleaseState::Approved,
        };
        let release = put_release_approval_idempotent(&mut session, ctx, release).await?;
        crate::formal_authority::record_parent_approval(&mut session, ctx, &release).await?;
        if release.state != PersistentReleaseState::Approved {
            session.commit().await?;
            return Ok(release);
        }
        session
            .put_edge(ctx, "release", &release.id, "artifact", &candidate.id)
            .await?;
        session
            .put_edge(
                ctx,
                "release",
                &release.id,
                "artifact",
                &crate::streaming_evaluator::storage_id(
                    "formal_evaluation_v2",
                    &release.report_id,
                )?,
            )
            .await?;
        session.audit(ctx, "release.approve", &release.id).await?;
        session.commit().await?;
        Ok(release)
    }

    pub async fn advance_canary(
        ctx: &Context,
        store: &Store,
        release_id: &str,
    ) -> Result<PersistentRelease> {
        ctx.require(&[Role::Admin])?;
        let mut session = store.session().await?;
        let mut release: PersistentRelease = session.need(ctx, "release", release_id).await?;
        if release.state != PersistentReleaseState::Approved || release.approved_by != ctx.actor() {
            return Err(Error::Forbidden);
        }
        revalidate_current_release(&mut session, ctx, &release).await?;
        release.state = PersistentReleaseState::Canary;
        write_release_state(&mut session, ctx, &release).await?;
        session.audit(ctx, "release.canary", release_id).await?;
        session.commit().await?;
        Ok(release)
    }

    pub async fn activate(
        ctx: &Context,
        store: &Store,
        release_id: &str,
        expected_pointer_epoch: u64,
    ) -> Result<PersistentRelease> {
        ctx.require(&[Role::Admin])?;
        let mut session = store.session().await?;
        let mut release: PersistentRelease = session.need(ctx, "release", release_id).await?;
        if release.state != PersistentReleaseState::Canary || release.approved_by != ctx.actor() {
            return Err(Error::Forbidden);
        }
        revalidate_current_release(&mut session, ctx, &release).await?;
        cas_pointer_in_session(
            &mut session,
            ctx,
            &release.profile_id,
            expected_pointer_epoch,
            release.expected_parent_release_id.as_deref(),
            release.id.clone(),
            release.bundle_digest.clone(),
        )
        .await?;
        if let Some(previous_id) = &release.expected_parent_release_id {
            let mut previous: PersistentRelease = session.need(ctx, "release", previous_id).await?;
            if previous.state != PersistentReleaseState::Active {
                return Err(Error::Conflict(
                    "expected parent release is not active".into(),
                ));
            }
            require_body_witness(
                &mut session,
                ctx,
                "release",
                previous_id,
                &previous,
                Role::Admin,
            )
            .await?;
            previous.state = PersistentReleaseState::Superseded;
            write_release_state(&mut session, ctx, &previous).await?;
        }
        release.state = PersistentReleaseState::Active;
        write_release_state(&mut session, ctx, &release).await?;
        session.audit(ctx, "release.activate", release_id).await?;
        session.commit().await?;
        Ok(release)
    }

    pub async fn register_host_surface(
        ctx: &Context,
        store: &Store,
        id: &str,
        manifest: HostSurfaceManifest,
        extracted: Vec<String>,
    ) -> Result<HostSurfaceRecord> {
        ctx.require(&[Role::Admin])?;
        identifier(id)?;
        // Controller F21: the guard is a family check, not an exact string match, so a
        // relabelled version ("Unverified-...") or host id ("claude-code-mcp-tool-only:v1",
        // "Claude-Code") cannot register an unverified Claude surface as supported.
        let host = manifest.host.to_ascii_lowercase();
        let host_version = manifest.host_version.to_ascii_lowercase();
        if host_version.starts_with("unverified-")
            || host == CLAUDE_CODE_TARGET
            || host.contains("claude")
        {
            return Err(Error::Invalid(
                "unverified host surface candidate cannot be registered as supported".into(),
            ));
        }
        manifest.validate_against_extraction(&extracted)?;
        let record = HostSurfaceRecord {
            id: id.into(),
            schema_version: HOST_SURFACE_RECORD_SCHEMA.into(),
            manifest_digest: fingerprint(&manifest)?,
            manifest,
            extracted,
        };
        let mut session = store.session().await?;
        if let Some(existing) = session
            .get::<HostSurfaceRecord>(ctx, "artifact", id)
            .await?
        {
            if fingerprint(&existing)? != fingerprint(&record)? {
                return Err(Error::Conflict(
                    "host surface id reused with different content".into(),
                ));
            }
            require_body_witness(&mut session, ctx, "artifact", id, &existing, Role::Admin).await?;
            session.commit().await?;
            return Ok(existing);
        }
        // The frozen v1 API has no original source material. Existing,
        // authenticated records above remain read-only/idempotent; a new
        // registration must use the source-bound writer instead of trusting
        // a caller-provided digest.
        Err(Error::Conflict(
            "new v1 host surface registration lacks original source; use source-bound registration"
                .into(),
        ))
    }

    /// Admin source-based registration. Classification metadata cannot inject
    /// extraction/digests/diff or confer host execution/smoke authority.
    pub async fn register_host_surface_from_source(
        ctx: &Context,
        store: &Store,
        id: &str,
        source: ReviewedHostSourceEnvelopeV1,
        registration: HostSurfaceRegistrationV2,
        predecessor_surface_id: Option<String>,
    ) -> Result<HostSurfaceRecordV2> {
        ctx.require(&[Role::Admin])?;
        identifier(id)?;
        let (manifest, extracted_typed) = host_surface_v2_from_source(&source, &registration)?;
        crate::hosts::validate_registerable_host_v2(&manifest.host, &manifest.host_version)?;
        if predecessor_surface_id.as_deref() == Some(id) {
            return Err(Error::Conflict("self predecessor".into()));
        }
        let mut session = store.session().await?;
        let predecessor = match predecessor_surface_id.as_deref() {
            Some(id) => Some(load_host_surface_predecessor_v2(&mut session, ctx, id).await?),
            None => None,
        };
        if predecessor
            .as_ref()
            .is_some_and(|previous| previous.host() != manifest.host)
        {
            return Err(Error::Conflict(
                "host surface predecessor belongs to another host".into(),
            ));
        }
        let upgrade_diff = host_surface_upgrade_diff_v2(
            &manifest,
            predecessor.as_ref().map(|value| value.manifest()),
        )?;
        let record = HostSurfaceRecordV2 {
            id: id.into(),
            schema_version: HOST_SURFACE_RECORD_SCHEMA_V2.into(),
            manifest_digest: fingerprint(&manifest)?,
            source,
            extracted: extracted_typed
                .iter()
                .map(|item| item.name.clone())
                .collect(),
            extracted_typed,
            manifest,
            predecessor_surface_id,
            upgrade_diff,
        };
        validate_host_record_v2_source(&record)?;
        validate_host_surface_v2_predecessor(&record, predecessor.as_ref())?;
        if let Some(existing) = session
            .get::<serde_json::Value>(ctx, "artifact", id)
            .await?
        {
            let existing: HostSurfaceRecordV2 = serde_json::from_value(existing).map_err(|_| {
                Error::Conflict("host surface id reused with different content".into())
            })?;
            if fingerprint(&existing)? != fingerprint(&record)? {
                return Err(Error::Conflict(
                    "host surface id reused with different content".into(),
                ));
            }
            let existing = validate_host_surface_v2_live_in_session(&mut session, ctx, id).await?;
            session.commit().await?;
            return Ok(existing);
        }
        // Every new source/typed/upgrade guard above precedes this first DML.
        session
            .put(ctx, "artifact", id, ctx.actor(), &record)
            .await?;
        if let Some(predecessor_id) = &record.predecessor_surface_id {
            session
                .put_edge(ctx, "artifact", id, "artifact", predecessor_id)
                .await?;
        }
        crate::formal_authority::attest_content(&mut session, ctx, "artifact", id, &record).await?;
        session.audit(ctx, "host_surface.register", id).await?;
        session.commit().await?;
        Ok(record)
    }

    pub async fn prepare_run(
        ctx: &Context,
        store: &Store,
        request: PrepareRunRequest,
    ) -> Result<LoadedRunApplicationSnapshot> {
        ctx.require(&[Role::Host])?;
        validate_prepare_request(&request)?;
        let snapshot_id = format!("run-snapshot-{}", request.run_id);
        let requested_environment_digest = request.system_snapshot.digest()?;
        let requested_caps_digest = fingerprint(&request.host_capabilities)?;
        let mut session = store.session().await?;
        if let Some(existing) = session
            .get::<LoadedRunApplicationSnapshot>(ctx, "artifact", &snapshot_id)
            .await?
        {
            if existing.profile_id != request.profile_id
                || existing.environment_digest != requested_environment_digest
                || existing.host_surface_id != request.host_surface_id
                || existing.host_capabilities_digest != requested_caps_digest
                || existing.request_material.task_input_digest != request.task_input_digest
                || existing.capability_level != request.capability_level
                || existing.evolution_enabled != request.evolution_enabled
            {
                return Err(Error::Conflict(
                    "run id reused with different frozen inputs".into(),
                ));
            }
            // Replays retain their actual wire. Caller capabilities never
            // upgrade or fill a historical v1 snapshot.
            let existing =
                Self::validate_run_snapshot_live_in_session(ctx, &mut session, &request.run_id)
                    .await?;
            require_current_run_source(&mut session, ctx, &existing).await?;
            session.commit().await?;
            return Ok(existing);
        }
        let surface =
            validate_host_surface_v2_live_in_session(&mut session, ctx, &request.host_surface_id)
                .await?;
        if surface.manifest.host != request.system_snapshot.host_id
            || surface.manifest.host_version != request.system_snapshot.host_version
        {
            return Err(Error::Conflict(
                "host surface does not match system snapshot".into(),
            ));
        }
        let environment_digest = requested_environment_digest;
        let caps_digest = requested_caps_digest;
        let pointer = load_pointer(&mut session, ctx, &request.profile_id).await?;
        let (release_id, bundle_digest, projection) = if request.evolution_enabled {
            if let Some(release_id) = &pointer.active_release_id {
                let release: PersistentRelease = session.need(ctx, "release", release_id).await?;
                if release.state != PersistentReleaseState::Active
                    || release.bundle_digest.as_str()
                        != pointer.active_bundle_digest.as_deref().unwrap_or("")
                    || release.environment_digest != environment_digest
                {
                    return Err(Error::Conflict("active release is incompatible".into()));
                }
                revalidate_current_release(&mut session, ctx, &release).await?;
                let candidate: ReadCandidate =
                    session.need(ctx, "artifact", &release.candidate_id).await?;
                let projection = project_read_run_surface_v2(
                    &surface,
                    &request.host_capabilities,
                    &candidate.bundle,
                    &request.system_snapshot,
                )?;
                (Some(release.id), Some(release.bundle_digest), projection)
            } else {
                (None, None, baseline_projection(&request.system_snapshot))
            }
        } else {
            (None, None, baseline_projection(&request.system_snapshot))
        };
        let material = AppliedRequestMaterial {
            mandatory_context_digest: request.system_snapshot.mandatory_context_digest.clone(),
            task_input_digest: request.task_input_digest,
            instructions: projection.instructions.clone(),
            tools: projection.tools.clone(),
        };
        let current = RunApplicationSnapshotV2 {
            id: snapshot_id.clone(),
            schema_version: RUN_APPLICATION_SCHEMA_V2.into(),
            run_id: request.run_id,
            profile_id: request.profile_id,
            pointer_epoch: pointer.epoch,
            release_id,
            bundle_digest,
            environment_digest,
            system_snapshot: request.system_snapshot,
            host_surface_id: surface.id,
            host_surface_digest: surface.manifest_digest,
            host_capabilities_digest: caps_digest,
            host_capabilities: request.host_capabilities,
            projection,
            request_digest: fingerprint(&material)?,
            request_material: material,
            capability_level: request.capability_level,
            evolution_enabled: request.evolution_enabled,
        };
        let record = LoadedRunApplicationSnapshot::from_wire(RunApplicationWire::Current(current))?;
        session
            .put(ctx, "artifact", &snapshot_id, ctx.actor(), &record)
            .await?;
        if let Some(release_id) = &record.release_id {
            session
                .put_edge(ctx, "artifact", &snapshot_id, "release", release_id)
                .await?;
        }
        session
            .put_edge(
                ctx,
                "artifact",
                &snapshot_id,
                "artifact",
                &record.host_surface_id,
            )
            .await?;
        crate::formal_authority::attest_content(
            &mut session,
            ctx,
            "artifact",
            &snapshot_id,
            &record,
        )
        .await?;
        session
            .audit(ctx, "release.prepare_run", &snapshot_id)
            .await?;
        session.commit().await?;
        Ok(record)
    }

    pub async fn read_run_snapshot(
        ctx: &Context,
        store: &Store,
        run_id: &str,
    ) -> Result<LoadedRunApplicationSnapshot> {
        Self::validate_run_snapshot_live(ctx, store, run_id).await
    }

    pub async fn validate_run_snapshot_live(
        ctx: &Context,
        store: &Store,
        run_id: &str,
    ) -> Result<LoadedRunApplicationSnapshot> {
        let mut session = store.session().await?;
        let snapshot =
            Self::validate_run_snapshot_live_in_session(ctx, &mut session, run_id).await?;
        session.commit().await?;
        Ok(snapshot)
    }

    pub async fn validate_run_snapshot_live_in_session(
        ctx: &Context,
        session: &mut Session,
        run_id: &str,
    ) -> Result<LoadedRunApplicationSnapshot> {
        ctx.require(&[Role::Host, Role::Admin])?;
        identifier(run_id)?;
        let snapshot_id = format!("run-snapshot-{run_id}");
        let snapshot: LoadedRunApplicationSnapshot =
            session.need(ctx, "artifact", &snapshot_id).await?;
        require_body_witness(
            session,
            ctx,
            "artifact",
            &snapshot_id,
            &snapshot,
            Role::Host,
        )
        .await?;
        if snapshot.id != snapshot_id
            || snapshot.run_id != run_id
            || snapshot.system_snapshot.profile_id != snapshot.profile_id
            || snapshot.environment_digest != snapshot.system_snapshot.digest()?
            || snapshot.request_digest != fingerprint(&snapshot.request_material)?
            || snapshot.request_material.instructions != snapshot.projection.instructions
            || snapshot.request_material.tools != snapshot.projection.tools
            || snapshot.request_material.mandatory_context_digest
                != snapshot.system_snapshot.mandatory_context_digest
        {
            return Err(Error::Conflict(
                "restored run application binding differs".into(),
            ));
        }
        let current_surface = match &snapshot.wire {
            RunApplicationWire::Legacy(_) => {
                // Original v1 historical path: no new fields, fabricated raw
                // capabilities, source upgrade or current-wire fallback.
                let surface: HostSurfaceRecord = session
                    .need(ctx, "artifact", &snapshot.host_surface_id)
                    .await?;
                require_body_witness(session, ctx, "artifact", &surface.id, &surface, Role::Admin)
                    .await?;
                surface
                    .manifest
                    .validate_against_extraction(&surface.extracted)?;
                if surface.manifest_digest != snapshot.host_surface_digest
                    || surface.manifest_digest != fingerprint(&surface.manifest)?
                {
                    return Err(Error::Conflict("host surface changed".into()));
                }
                None
            }
            RunApplicationWire::Current(current) => {
                validate_run_application_v2_binding(current)?;
                let surface = validate_host_surface_v2_live_in_session(
                    session,
                    ctx,
                    &snapshot.host_surface_id,
                )
                .await?;
                if surface.manifest_digest != snapshot.host_surface_digest
                    || surface.manifest.host != snapshot.system_snapshot.host_id
                    || surface.manifest.host_version != snapshot.system_snapshot.host_version
                    || (!snapshot.evolution_enabled && snapshot.release_id.is_some())
                {
                    return Err(Error::Conflict(
                        "current host surface/run source changed".into(),
                    ));
                }
                Some(surface)
            }
        };
        if let Some(release_id) = &snapshot.release_id {
            let release: PersistentRelease = session.need(ctx, "release", release_id).await?;
            if !matches!(
                release.state,
                PersistentReleaseState::Active | PersistentReleaseState::Superseded
            ) || release.profile_id != snapshot.profile_id
                || release.bundle_digest.as_str() != snapshot.bundle_digest.as_deref().unwrap_or("")
                || release.environment_digest != snapshot.environment_digest
            {
                return Err(Error::Forbidden);
            }
            let candidate: ReadCandidate =
                session.need(ctx, "artifact", &release.candidate_id).await?;
            if candidate.bundle_digest != release.bundle_digest
                || candidate.environment_digest != release.environment_digest
            {
                return Err(Error::Conflict(
                    "run release candidate identity changed".into(),
                ));
            }
            if candidate.is_current() {
                revalidate_current_release(session, ctx, &release).await?;
            } else {
                revalidate_release(session, ctx, &release).await?;
            }
            if let (Some(surface), Some(current)) = (&current_surface, snapshot.current_v2()) {
                let projection = project_read_run_surface_v2(
                    surface,
                    &current.host_capabilities,
                    &candidate.bundle,
                    &snapshot.system_snapshot,
                )?;
                if fingerprint(&projection)? != fingerprint(&snapshot.projection)? {
                    return Err(Error::Conflict(
                        "current projection/capability intersection changed".into(),
                    ));
                }
            }
        } else if snapshot.bundle_digest.is_some() {
            return Err(Error::Conflict(
                "run snapshot has a bundle without an active release".into(),
            ));
        } else if current_surface.is_some()
            && fingerprint(&snapshot.projection)?
                != fingerprint(&baseline_projection(&snapshot.system_snapshot))?
        {
            return Err(Error::Conflict(
                "current baseline projection changed".into(),
            ));
        }
        Ok(snapshot)
    }

    pub async fn record_applied_request(
        ctx: &Context,
        store: &Store,
        run_id: &str,
        evidence: TrustedHostExecutionEvidence,
    ) -> Result<HostApplicationRecord> {
        ctx.require(&[Role::Host])?;
        identifier(run_id)?;
        let mut session = store.session().await?;
        let snapshot =
            Self::validate_run_snapshot_live_in_session(ctx, &mut session, run_id).await?;
        // Historical snapshots remain readable and replayable, but cannot
        // authorize a new application record, including zero-skill baselines
        // whose release_id is None.
        if snapshot.current_v2().is_none() {
            return Err(Error::Conflict(
                "legacy run snapshot cannot authorize a new applied request".into(),
            ));
        }
        require_current_run_source(&mut session, ctx, &snapshot).await?;
        let actual_request_digest = fingerprint(&evidence.actual_request_material)?;
        if actual_request_digest != snapshot.request_digest
            || evidence.actual_request_material != snapshot.request_material
            || evidence.environment_digest != snapshot.environment_digest
            || evidence.host_surface_digest != snapshot.host_surface_digest
            || evidence.host_capabilities_digest != snapshot.host_capabilities_digest
        {
            return Err(Error::Conflict(
                "actual applied request differs from frozen run".into(),
            ));
        }
        let receipt = match &snapshot.bundle_digest {
            Some(bundle_digest) => {
                let receipt = AppliedReceipt {
                    offered: evidence.offered,
                    attached: evidence.attached,
                    used: evidence.used,
                    verified_benefit: Vec::new(),
                    bundle_digest: bundle_digest.clone(),
                    request_digest: actual_request_digest.clone(),
                    capability_level: snapshot.capability_level,
                    truncated: evidence.truncated,
                    attested_by: ctx.actor().into(),
                };
                receipt.validate()?;
                if !receipt.used.is_empty() {
                    let execution_id =
                        evidence.execution_receipt_id.as_deref().ok_or_else(|| {
                            Error::Invalid("used requires a trusted execution receipt id".into())
                        })?;
                    let execution: TrustedHostExecutionReceipt =
                        session.need(ctx, "artifact", execution_id).await?;
                    require_body_witness(
                        &mut session,
                        ctx,
                        "artifact",
                        execution_id,
                        &execution,
                        Role::Host,
                    )
                    .await?;
                    if execution.run_id != run_id
                        || execution.request_digest != actual_request_digest
                        || execution.environment_digest != snapshot.environment_digest
                        || execution.host_surface_digest != snapshot.host_surface_digest
                        || !receipt
                            .used
                            .iter()
                            .all(|id| execution.used_ids.contains(id))
                    {
                        return Err(Error::Conflict(
                            "execution receipt does not prove actual used ids".into(),
                        ));
                    }
                }
                Some(receipt)
            }
            None => {
                if !evidence.offered.is_empty()
                    || !evidence.attached.is_empty()
                    || !evidence.used.is_empty()
                    || evidence.execution_receipt_id.is_some()
                {
                    return Err(Error::Invalid(
                        "zero-skill baseline cannot claim application".into(),
                    ));
                }
                None
            }
        };
        let id = format!("host-application-{run_id}");
        let record = HostApplicationRecord {
            id: id.clone(),
            schema_version: HOST_APPLICATION_SCHEMA.into(),
            run_id: run_id.into(),
            release_id: snapshot.release_id.clone(),
            actual_request_digest,
            execution_receipt_id: evidence.execution_receipt_id,
            receipt,
        };
        if let Some(existing) = session
            .get::<HostApplicationRecord>(ctx, "receipt", &id)
            .await?
        {
            if fingerprint(&existing)? != fingerprint(&record)? {
                return Err(Error::Conflict(
                    "host application id reused with different facts".into(),
                ));
            }
            require_body_witness(&mut session, ctx, "receipt", &id, &existing, Role::Host).await?;
            session.commit().await?;
            return Ok(existing);
        }
        session
            .put(ctx, "receipt", &id, ctx.actor(), &record)
            .await?;
        crate::formal_authority::attest_content(&mut session, ctx, "receipt", &id, &record).await?;
        session.audit(ctx, "release.applied_request", &id).await?;
        session.commit().await?;
        Ok(record)
    }

    pub async fn store_host_execution_receipt(
        ctx: &Context,
        store: &Store,
        receipt: TrustedHostExecutionReceipt,
    ) -> Result<()> {
        ctx.require(&[Role::Host])?;
        identifier(&receipt.id)?;
        identifier(&receipt.run_id)?;
        validate_digest(&receipt.request_digest, "execution request digest")?;
        validate_digest(&receipt.environment_digest, "execution environment digest")?;
        validate_digest(
            &receipt.host_surface_digest,
            "execution host surface digest",
        )?;
        validate_digest(&receipt.output_digest, "execution output digest")?;
        if receipt.schema_version != "rsia.host_execution_receipt.v1" {
            return Err(Error::Invalid(
                "unsupported host execution receipt schema".into(),
            ));
        }
        let unique = receipt.used_ids.iter().collect::<BTreeSet<_>>();
        if unique.len() != receipt.used_ids.len() {
            return Err(Error::Conflict("duplicate executed used id".into()));
        }
        for id in &receipt.used_ids {
            identifier(id)?;
        }
        let mut session = store.session().await?;
        if let Some(existing) = session
            .get::<TrustedHostExecutionReceipt>(ctx, "artifact", &receipt.id)
            .await?
        {
            if fingerprint(&existing)? != fingerprint(&receipt)? {
                return Err(Error::Conflict(
                    "host execution receipt id reused with different facts".into(),
                ));
            }
            require_body_witness(
                &mut session,
                ctx,
                "artifact",
                &receipt.id,
                &existing,
                Role::Host,
            )
            .await?;
            session.commit().await?;
            return Ok(());
        }
        session
            .put(ctx, "artifact", &receipt.id, ctx.actor(), &receipt)
            .await?;
        crate::formal_authority::attest_content(
            &mut session,
            ctx,
            "artifact",
            &receipt.id,
            &receipt,
        )
        .await?;
        session
            .audit(ctx, "host.execution_receipt", &receipt.id)
            .await?;
        session.commit().await
    }

    pub async fn rollback(
        ctx: &Context,
        store: &Store,
        profile_id: &str,
        target_release_id: &str,
        expected_pointer_epoch: u64,
        environment_digest: &str,
    ) -> Result<PersistentRelease> {
        ctx.require(&[Role::Admin])?;
        let mut session = store.session().await?;
        let pointer = load_pointer(&mut session, ctx, profile_id).await?;
        let current_id = pointer
            .active_release_id
            .clone()
            .ok_or_else(|| Error::Conflict("profile has no active release".into()))?;
        if current_id == target_release_id {
            return Err(Error::Conflict("rollback target is already active".into()));
        }
        let mut current: PersistentRelease = session.need(ctx, "release", &current_id).await?;
        let mut target: PersistentRelease = session.need(ctx, "release", target_release_id).await?;
        require_body_witness(
            &mut session,
            ctx,
            "release",
            &current.id,
            &current,
            Role::Admin,
        )
        .await?;
        if target.profile_id != profile_id
            || target.environment_digest != environment_digest
            || target.state != PersistentReleaseState::Superseded
        {
            return Err(Error::Conflict(
                "rollback target is not a compatible prior approved release".into(),
            ));
        }
        revalidate_current_release(&mut session, ctx, &target).await?;
        cas_pointer_in_session(
            &mut session,
            ctx,
            profile_id,
            expected_pointer_epoch,
            Some(&current_id),
            target.id.clone(),
            target.bundle_digest.clone(),
        )
        .await?;
        current.state = PersistentReleaseState::RolledBack;
        target.state = PersistentReleaseState::Active;
        write_release_state(&mut session, ctx, &current).await?;
        write_release_state(&mut session, ctx, &target).await?;
        session
            .audit(ctx, "release.rollback", target_release_id)
            .await?;
        session.commit().await?;
        Ok(target)
    }
}

fn validate_stage_request(ctx: &Context, request: &StageBundleRequest) -> Result<()> {
    identifier(&request.candidate_id)?;
    identifier(&request.proposer_actor)?;
    if request.proposer_actor != ctx.actor() {
        return Err(Error::Forbidden);
    }
    validate_digest(&request.environment_digest, "environment_digest")?;
    validate_resolved_bundle_identity(&request.bundle)?;
    crate::formal_authority::require_builtin_parent_strategy(&request.bundle.improver)?;
    if request.sources.is_empty() {
        return Err(Error::Invalid(
            "release candidate requires source closure".into(),
        ));
    }
    Ok(())
}

fn canonical_sources(mut sources: Vec<TypedSourceRef>) -> Result<Vec<TypedSourceRef>> {
    for source in &sources {
        if source.kind != "run" {
            return Err(Error::Invalid("unsupported release source kind".into()));
        }
        identifier(&source.id)?;
        validate_digest(&source.content_digest, "source content digest")?;
    }
    sources.sort();
    if sources
        .windows(2)
        .any(|pair| pair[0].kind == pair[1].kind && pair[0].id == pair[1].id)
    {
        return Err(Error::Conflict(
            "duplicate or conflicting release source identity".into(),
        ));
    }
    Ok(sources)
}

async fn validate_candidate_sources(
    session: &mut Session,
    ctx: &Context,
    candidate: &ReleaseCandidateRecord,
) -> Result<()> {
    let ids = candidate
        .sources
        .iter()
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    validate_stored_sources(session, ctx, &ids, candidate.revoke_watermark).await?;
    for source in &candidate.sources {
        let authority = load_stored_source(session, ctx, &source.id).await?;
        if authority.trace.source_digest != source.content_digest {
            return Err(Error::Conflict("source content digest changed".into()));
        }
    }
    Ok(())
}

/// Reader-side validation of an immutable staged candidate. The formal
/// Evaluator is distinct from the proposer; stage_bundle alone enforces the
/// writer's actor. Read authority and all source checks use the caller's Session.
pub(crate) async fn validate_candidate_read_in_session(
    session: &mut Session,
    ctx: &Context,
    candidate: &ReleaseCandidateRecord,
) -> Result<()> {
    if candidate.schema_version != RELEASE_CANDIDATE_SCHEMA {
        return Err(Error::Invalid(
            "unsupported release candidate schema".into(),
        ));
    }
    identifier(&candidate.id)?;
    identifier(&candidate.profile_id)?;
    identifier(&candidate.proposer_actor)?;
    validate_digest(&candidate.environment_digest, "environment_digest")?;
    validate_digest(&candidate.bundle_digest, "bundle_digest")?;
    validate_digest(&candidate.parent_digest, "parent_digest")?;
    validate_resolved_bundle_identity(&candidate.bundle)?;
    crate::formal_authority::require_builtin_parent_strategy(&candidate.bundle.improver)?;
    if candidate.bundle_digest != candidate.bundle.digest
        || candidate.profile_id != candidate.bundle.profile_id
        || candidate.parent_digest != candidate.bundle.parent_digest
        || candidate.sources.is_empty()
        || canonical_sources(candidate.sources.clone())? != candidate.sources
    {
        return Err(Error::Conflict(
            "release candidate identity is incomplete or noncanonical".into(),
        ));
    }
    validate_candidate_sources(session, ctx, candidate).await
}

fn validate_verified_report_identity(
    candidate: &ReleaseCandidateRecord,
    view: &crate::streaming_evaluator::VerifiedFormalReportView,
) -> Result<()> {
    if view.variant != VerifiedReportVariant::CompleteBatch
        || !matches!(view.verdict, Some(Verdict::Improved | Verdict::Noninferior))
        || view.anchor_status != AnchorEvidenceStatus::CompletePassedStoredClosure
        || view.anchor_evidence_digest.is_none()
        || view.resource_evidence_digest.is_none()
        || view.evidence_scope != EvaluationEvidenceScope::RegisteredExternalHost
        || view.cost_evidence_scope != Some(CostEvidenceScope::CompleteEpisodeRoot)
        || view.dependency_status != DependencyEvidenceStatus::CompleteTypedContentClosure
        || !view.promotion_eligible
    {
        return Err(Error::Forbidden);
    }
    if view.candidate_digest != candidate.id
        || view.candidate_bundle_digest != candidate.bundle_digest
        || view.baseline_parent_digest != candidate.parent_digest
        || view.environment_digest != candidate.environment_digest
        || view.proposer_actor != candidate.proposer_actor
    {
        return Err(Error::Conflict(
            "verified report does not match release candidate".into(),
        ));
    }
    let watermark = view
        .revoke_watermark
        .as_ref()
        .and_then(|(seq, _)| u64::try_from(*seq).ok());
    if watermark != Some(candidate.revoke_watermark) {
        return Err(Error::Conflict("report revoke watermark mismatch".into()));
    }
    Ok(())
}

#[cfg(test)]
fn validate_approval_actor(
    ctx: &Context,
    candidate: &ReleaseCandidateRecord,
    view: &crate::streaming_evaluator::VerifiedFormalReportView,
) -> Result<()> {
    if view.approver_actor != ctx.actor()
        || ctx.actor() == candidate.proposer_actor
        || ctx.actor() == view.evaluator_actor
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

async fn put_release_approval_idempotent(
    session: &mut Session,
    ctx: &Context,
    proposed: PersistentRelease,
) -> Result<PersistentRelease> {
    if let Some(existing) = existing_release_approval_content(session, ctx, &proposed).await? {
        // Content identity alone never authorizes a formal release.
        require_body_witness(
            session,
            ctx,
            "release",
            &existing.id,
            &existing,
            Role::Admin,
        )
        .await?;
        return Ok(existing);
    }
    write_release_state(session, ctx, &proposed).await?;
    Ok(proposed)
}

/// Shared persisted identity/idempotence primitive. It cannot create a release
/// or authorize a formal parent; the formal wrapper adds the registered guard.
async fn existing_release_approval_content(
    session: &mut Session,
    ctx: &Context,
    proposed: &PersistentRelease,
) -> Result<Option<PersistentRelease>> {
    if let Some(existing) = session
        .get::<PersistentRelease>(ctx, "release", &proposed.id)
        .await?
    {
        if same_release_approval_identity(&existing, proposed) {
            require_body_content_witness(
                session,
                ctx,
                "release",
                &existing.id,
                &existing,
                Role::Admin,
            )
            .await?;
            return Ok(Some(existing));
        }
        return Err(Error::Conflict(
            "release id reused with different approval evidence".into(),
        ));
    }
    Ok(None)
}

fn same_release_approval_identity(left: &PersistentRelease, right: &PersistentRelease) -> bool {
    left.id == right.id
        && left.schema_version == right.schema_version
        && left.candidate_id == right.candidate_id
        && left.bundle_digest == right.bundle_digest
        && left.environment_digest == right.environment_digest
        && left.profile_id == right.profile_id
        && left.parent_digest == right.parent_digest
        && left.baseline_bundle_digest == right.baseline_bundle_digest
        && left.report_id == right.report_id
        && left.report_digest == right.report_digest
        && left.proposer_actor == right.proposer_actor
        && left.evaluator_actor == right.evaluator_actor
        && left.approved_by == right.approved_by
}

fn validate_stored_release_identity(
    release: &PersistentRelease,
    candidate: &ReleaseCandidateRecord,
    view: &crate::streaming_evaluator::VerifiedFormalReportView,
) -> Result<()> {
    if view.report_digest != release.report_digest
        || candidate.bundle_digest != release.bundle_digest
        || candidate.environment_digest != release.environment_digest
        || release.proposer_actor != view.proposer_actor
        || release.evaluator_actor != view.evaluator_actor
        || release.approved_by != view.approver_actor
        || release.approved_by == release.proposer_actor
        || release.approved_by == release.evaluator_actor
    {
        return Err(Error::Conflict(
            "release evidence changed after approval".into(),
        ));
    }
    Ok(())
}

fn validate_persisted_approval_binding(
    release: &PersistentRelease,
    candidate: &ReleaseCandidateRecord,
) -> Result<()> {
    identifier(&release.report_id)?;
    validate_digest(&release.report_digest, "release report digest")?;
    if candidate.id != release.candidate_id
        || candidate.bundle_digest != release.bundle_digest
        || candidate.environment_digest != release.environment_digest
        || candidate.parent_digest != release.parent_digest
        || candidate.proposer_actor != release.proposer_actor
        || release.approved_by == release.proposer_actor
        || release.approved_by == release.evaluator_actor
    {
        return Err(Error::Conflict(
            "stored release approval binding is inconsistent".into(),
        ));
    }
    Ok(())
}

// Internal writer only: callers have already checked the formal approval or
// the permitted state transition. This does not register a parent selection.
async fn write_release_state(
    session: &mut Session,
    ctx: &Context,
    release: &PersistentRelease,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    crate::formal_authority::bind_release_principal(session, ctx, release).await?;
    crate::formal_authority::attest_registered_content(
        session,
        ctx,
        "release",
        &release.id,
        release,
    )
    .await?;
    write_release_content(session, ctx, release).await
}

/// Actual production content mutation and latest private-audit witness. This
/// private primitive has no candidate/report/principal or formal-parent power.
async fn write_release_content(
    session: &mut Session,
    ctx: &Context,
    release: &PersistentRelease,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    session
        .put(ctx, "release", &release.id, ctx.actor(), release)
        .await?;
    crate::formal_authority::write_content_witness(session, ctx, "release", &release.id, release)
        .await
}

async fn require_body_content_witness<T: Serialize>(
    session: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    body: &T,
    role: Role,
) -> Result<serde_json::Value> {
    let value = serde_json::to_value(body).map_err(|_| Error::Internal)?;
    crate::formal_authority::require_content_witness(session, ctx, kind, id, &value, role).await?;
    Ok(value)
}

async fn require_body_witness<T: Serialize>(
    session: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    body: &T,
    role: Role,
) -> Result<()> {
    let value = require_body_content_witness(session, ctx, kind, id, body, role).await?;
    if role == Role::Admin {
        crate::formal_authority::require_registered_body_authority(session, ctx, kind, id, &value)
            .await?;
    }
    Ok(())
}

async fn revalidate_release(
    session: &mut Session,
    ctx: &Context,
    release: &PersistentRelease,
) -> Result<()> {
    require_body_witness(session, ctx, "release", &release.id, release, Role::Admin).await?;
    let candidate: ReleaseCandidateRecord =
        session.need(ctx, "artifact", &release.candidate_id).await?;
    validate_candidate_sources(session, ctx, &candidate).await?;
    validate_persisted_approval_binding(release, &candidate)?;
    crate::formal_authority::verify_release_principal(session, ctx, release, &candidate).await?;
    let view = if ctx.role() == Role::Host {
        verified_report_view_for_host_in_session(ctx, session, &release.report_id).await?
    } else {
        verified_report_view_in_session(ctx, session, &release.report_id).await?
    };
    validate_verified_report_identity(&candidate, &view)?;
    validate_stored_release_identity(release, &candidate, &view)
}

async fn load_pointer(
    session: &mut Session,
    ctx: &Context,
    profile_id: &str,
) -> Result<PersistentProfilePointer> {
    let pointer = load_pointer_content(session, ctx, profile_id).await?;
    if session
        .get::<serde_json::Value>(ctx, "pointer", &pointer.id)
        .await?
        .is_some()
    {
        let value = serde_json::to_value(&pointer).map_err(|_| Error::Internal)?;
        crate::formal_authority::require_registered_body_authority(
            session,
            ctx,
            "pointer",
            &pointer.id,
            &value,
        )
        .await?;
    }
    Ok(pointer)
}

// Identity/default handling and private content witness, shared by the formal
// loader and component CAS tests. A returned pointer is not formal authority.
async fn load_pointer_content(
    session: &mut Session,
    ctx: &Context,
    profile_id: &str,
) -> Result<PersistentProfilePointer> {
    identifier(profile_id)?;
    let id = format!("profile-pointer-{profile_id}");
    if let Some(pointer) = session
        .get::<PersistentProfilePointer>(ctx, "pointer", &id)
        .await?
    {
        require_body_content_witness(session, ctx, "pointer", &id, &pointer, Role::Admin).await?;
        if pointer.id != id
            || pointer.profile_id != profile_id
            || pointer.schema_version != PROFILE_POINTER_SCHEMA
        {
            return Err(Error::Conflict("profile pointer identity mismatch".into()));
        }
        Ok(pointer)
    } else {
        Ok(PersistentProfilePointer {
            id,
            schema_version: PROFILE_POINTER_SCHEMA.into(),
            profile_id: profile_id.into(),
            active_release_id: None,
            active_bundle_digest: None,
            epoch: 0,
        })
    }
}

fn cas_pointer(
    pointer: &mut PersistentProfilePointer,
    expected_epoch: u64,
    expected_active_release_id: Option<&str>,
    next_release_id: String,
    next_bundle_digest: String,
) -> Result<()> {
    if pointer.epoch != expected_epoch
        || pointer.active_release_id.as_deref() != expected_active_release_id
    {
        return Err(Error::Conflict("cas_lost".into()));
    }
    identifier(&next_release_id)?;
    validate_digest(&next_bundle_digest, "next bundle digest")?;
    pointer.epoch = pointer
        .epoch
        .checked_add(1)
        .ok_or_else(|| Error::Conflict("pointer_epoch_overflow".into()))?;
    pointer.active_release_id = Some(next_release_id);
    pointer.active_bundle_digest = Some(next_bundle_digest);
    Ok(())
}

async fn cas_pointer_in_session(
    session: &mut Session,
    ctx: &Context,
    profile_id: &str,
    expected_epoch: u64,
    expected_active_release_id: Option<&str>,
    next_release_id: String,
    next_bundle_digest: String,
) -> Result<PersistentProfilePointer> {
    let mut pointer = load_pointer(session, ctx, profile_id).await?;
    cas_pointer(
        &mut pointer,
        expected_epoch,
        expected_active_release_id,
        next_release_id,
        next_bundle_digest,
    )?;
    // Authenticate the complete new pointer before any business-row mutation.
    crate::formal_authority::attest_registered_content(
        session,
        ctx,
        "pointer",
        &pointer.id,
        &pointer,
    )
    .await?;
    write_pointer_content(session, ctx, &pointer).await?;
    Ok(pointer)
}

/// Shared production put/witness step. The caller owns the same transaction
/// that read and compared the epoch; only the formal wrapper authorizes use.
async fn write_pointer_content(
    session: &mut Session,
    ctx: &Context,
    pointer: &PersistentProfilePointer,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    session
        .put(ctx, "pointer", &pointer.id, ctx.actor(), pointer)
        .await?;
    crate::formal_authority::write_content_witness(session, ctx, "pointer", &pointer.id, pointer)
        .await
}

enum StoredHostSurfacePredecessorV2 {
    Legacy(Box<HostSurfaceRecord>),
    Current(Box<HostSurfaceRecordV2>),
}

impl StoredHostSurfacePredecessorV2 {
    fn manifest(&self) -> HostSurfacePredecessorV2<'_> {
        match self {
            Self::Legacy(value) => HostSurfacePredecessorV2::Legacy(&value.manifest),
            Self::Current(value) => HostSurfacePredecessorV2::Current(&value.manifest),
        }
    }
    fn host(&self) -> &str {
        match self {
            Self::Legacy(value) => &value.manifest.host,
            Self::Current(value) => &value.manifest.host,
        }
    }
}

fn validate_host_surface_v2_predecessor(
    record: &HostSurfaceRecordV2,
    predecessor: Option<&StoredHostSurfacePredecessorV2>,
) -> Result<()> {
    if predecessor.is_some_and(|previous| previous.host() != record.manifest.host) {
        return Err(Error::Conflict(
            "host surface predecessor belongs to another host".into(),
        ));
    }
    let diff =
        host_surface_upgrade_diff_v2(&record.manifest, predecessor.map(|value| value.manifest()))?;
    if record.upgrade_diff != diff {
        return Err(Error::Conflict(
            "host surface upgrade diff differs from its predecessor".into(),
        ));
    }
    Ok(())
}

/// Iterative exact-id chain re-read: no fabricated predecessor or depth quota.
/// A v1 terminal remains its original wire; it supplies no typed/default facts.
pub(crate) async fn validate_host_surface_v2_live_in_session(
    session: &mut Session,
    ctx: &Context,
    id: &str,
) -> Result<HostSurfaceRecordV2> {
    identifier(id)?;
    let mut cursor = id.to_owned();
    let mut visited = BTreeSet::new();
    let mut records = Vec::new();
    let mut predecessor = None;
    loop {
        if !visited.insert(cursor.clone()) {
            return Err(Error::Conflict("cyclic host surface predecessor".into()));
        }
        let body: serde_json::Value = session.need(ctx, "artifact", &cursor).await?;
        match body
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
        {
            Some(HOST_SURFACE_RECORD_SCHEMA_V2) => {
                let record: HostSurfaceRecordV2 = serde_json::from_value(body)
                    .map_err(|_| Error::Invalid("invalid host surface v2".into()))?;
                if record.id != cursor {
                    return Err(Error::Conflict("host surface id alias".into()));
                }
                validate_host_record_v2_source(&record)?;
                crate::hosts::validate_registerable_host_v2(
                    &record.manifest.host,
                    &record.manifest.host_version,
                )?;
                require_body_witness(session, ctx, "artifact", &cursor, &record, Role::Admin)
                    .await?;
                let next = record.predecessor_surface_id.clone();
                records.push(record);
                match next {
                    Some(next) => cursor = next,
                    None => break,
                }
            }
            Some(HOST_SURFACE_RECORD_SCHEMA) if !records.is_empty() => {
                let record: HostSurfaceRecord = serde_json::from_value(body)
                    .map_err(|_| Error::Invalid("invalid legacy host surface".into()))?;
                if record.id != cursor || record.manifest_digest != fingerprint(&record.manifest)? {
                    return Err(Error::Conflict(
                        "legacy predecessor identity differs".into(),
                    ));
                }
                record
                    .manifest
                    .validate_against_extraction(&record.extracted)?;
                require_body_witness(session, ctx, "artifact", &cursor, &record, Role::Admin)
                    .await?;
                predecessor = Some(StoredHostSurfacePredecessorV2::Legacy(Box::new(record)));
                break;
            }
            Some(HOST_SURFACE_RECORD_SCHEMA) => {
                // Keep the original malformed-v1 diagnostic before the new-use
                // boundary. A healthy frozen v1 remains audit-only material.
                let record: HostSurfaceRecord = serde_json::from_value(body)
                    .map_err(|_| Error::Invalid("invalid legacy host surface".into()))?;
                record
                    .manifest
                    .validate_against_extraction(&record.extracted)?;
                if record.id != cursor || record.manifest_digest != fingerprint(&record.manifest)? {
                    return Err(Error::Conflict(
                        "legacy predecessor identity differs".into(),
                    ));
                }
                return Err(Error::Invalid(
                    "current prepare requires a source-bound host surface v2".into(),
                ));
            }
            _ => {
                return Err(Error::Invalid(
                    "current prepare requires a source-bound host surface v2".into(),
                ));
            }
        }
    }
    // Consume the chain from the oldest source towards the requested current record.
    for record in records.into_iter().rev() {
        validate_host_surface_v2_predecessor(&record, predecessor.as_ref())?;
        predecessor = Some(StoredHostSurfacePredecessorV2::Current(Box::new(record)));
    }
    match predecessor {
        Some(StoredHostSurfacePredecessorV2::Current(record)) => Ok(*record),
        _ => Err(Error::Invalid("missing current host surface".into())),
    }
}

async fn load_host_surface_predecessor_v2(
    session: &mut Session,
    ctx: &Context,
    id: &str,
) -> Result<StoredHostSurfacePredecessorV2> {
    identifier(id)?;
    let body: serde_json::Value = session.need(ctx, "artifact", id).await?;
    match body
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
    {
        Some(HOST_SURFACE_RECORD_SCHEMA_V2) => Ok(StoredHostSurfacePredecessorV2::Current(
            Box::new(validate_host_surface_v2_live_in_session(session, ctx, id).await?),
        )),
        Some(HOST_SURFACE_RECORD_SCHEMA) => {
            let record: HostSurfaceRecord = serde_json::from_value(body)
                .map_err(|_| Error::Invalid("invalid legacy predecessor".into()))?;
            if record.id != id || record.manifest_digest != fingerprint(&record.manifest)? {
                return Err(Error::Conflict(
                    "legacy predecessor identity differs".into(),
                ));
            }
            record
                .manifest
                .validate_against_extraction(&record.extracted)?;
            require_body_witness(session, ctx, "artifact", id, &record, Role::Admin).await?;
            Ok(StoredHostSurfacePredecessorV2::Legacy(Box::new(record)))
        }
        _ => Err(Error::Invalid("unsupported predecessor schema".into())),
    }
}

fn baseline_projection(snapshot: &SystemSnapshot) -> RunProjection {
    RunProjection {
        bundle_digest: String::new(),
        instructions: Vec::new(),
        tools: snapshot.tools.clone(),
    }
}

fn validate_prepare_request(request: &PrepareRunRequest) -> Result<()> {
    identifier(&request.run_id)?;
    identifier(&request.profile_id)?;
    identifier(&request.host_surface_id)?;
    validate_digest(&request.task_input_digest, "task input digest")?;
    if request.system_snapshot.profile_id != request.profile_id {
        return Err(Error::Conflict("system snapshot profile mismatch".into()));
    }
    Ok(())
}

fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(Error::Invalid(format!(
            "{name}: expected lowercase sha256 digest"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming_evaluator::{
        CostEvidenceScope, DependencyEvidenceStatus, EvaluationEvidenceScope,
        VerifiedFormalReportView,
    };
    use evo_core::evaluation::ProfileKind;

    #[test]
    fn pointer_cas_is_bounded_and_detects_competition() {
        let mut pointer = PersistentProfilePointer {
            id: "profile-pointer-p1".into(),
            schema_version: PROFILE_POINTER_SCHEMA.into(),
            profile_id: "p1".into(),
            active_release_id: None,
            active_bundle_digest: None,
            epoch: 0,
        };
        cas_pointer(&mut pointer, 0, None, "release-a".into(), "a".repeat(64)).unwrap();
        assert!(cas_pointer(&mut pointer, 0, None, "release-b".into(), "b".repeat(64),).is_err());
    }

    #[tokio::test]
    async fn sqlite_pointer_cas_has_exactly_one_winner() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("cas.sqlite3")).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let left_store = store.clone();
        let right_store = store.clone();
        let left_admin = admin.clone();
        let right_admin = admin.clone();
        let left = async move {
            let mut session = left_store.session().await?;
            // Component CAS: the same production read/compare/put+witness
            // primitives and one real Session, without formal registration.
            let mut pointer = load_pointer_content(&mut session, &left_admin, "p1").await?;
            cas_pointer(&mut pointer, 0, None, "release-left".into(), "a".repeat(64))?;
            write_pointer_content(&mut session, &left_admin, &pointer).await?;
            session.commit().await?;
            Ok::<_, Error>(pointer)
        };
        let right = async move {
            let mut session = right_store.session().await?;
            // Component CAS: the same production read/compare/put+witness
            // primitives and one real Session, without formal registration.
            let mut pointer = load_pointer_content(&mut session, &right_admin, "p1").await?;
            cas_pointer(
                &mut pointer,
                0,
                None,
                "release-right".into(),
                "b".repeat(64),
            )?;
            write_pointer_content(&mut session, &right_admin, &pointer).await?;
            session.commit().await?;
            Ok::<_, Error>(pointer)
        };
        let (left, right) = tokio::join!(left, right);
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let mut session = store.session().await.unwrap();
        let pointer = load_pointer_content(&mut session, &admin, "p1")
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert_eq!(pointer.epoch, 1);
    }

    #[test]
    fn early_report_cannot_promote_even_with_an_improved_verdict_field() {
        let bundle = ResolvedBundle {
            schema_version: "rsia.resolved_bundle.v2".into(),
            profile_id: "p1".into(),
            parent_digest: "b".repeat(64),
            baseline_digest: "c".repeat(64),
            skill: evo_core::contract::SkillSnapshot::empty(),
            improver: evo_core::Strategy::default(),
            origins: vec![],
            digest: "d".repeat(64),
        };
        let candidate = ReleaseCandidateRecord {
            id: "candidate-1".into(),
            schema_version: RELEASE_CANDIDATE_SCHEMA.into(),
            bundle,
            bundle_digest: "d".repeat(64),
            environment_digest: "e".repeat(64),
            profile_id: "p1".into(),
            parent_digest: "b".repeat(64),
            proposer_actor: "proposer".into(),
            sources: vec![],
            revoke_watermark: 1,
        };
        let view = VerifiedFormalReportView {
            report_id: "report-1".into(),
            report_digest: "f".repeat(64),
            variant: VerifiedReportVariant::EarlyStopped,
            verdict: Some(Verdict::Improved),
            ticket_id: "ticket-1".into(),
            ticket_digest: "1".repeat(64),
            v1_plan_snapshot_digest: "2".repeat(64),
            formal_plan_digest: "3".repeat(64),
            manifest_digest: "4".repeat(64),
            candidate_digest: candidate.id.clone(),
            baseline_parent_digest: candidate.parent_digest.clone(),
            candidate_bundle_digest: candidate.bundle_digest.clone(),
            baseline_bundle_digest: "5".repeat(64),
            environment_digest: candidate.environment_digest.clone(),
            profile: ProfileKind::QualityGain,
            proposer_actor: candidate.proposer_actor.clone(),
            evaluator_actor: "evaluator".into(),
            approver_actor: "approver".into(),
            anchor_evidence_digest: Some("6".repeat(64)),
            resource_evidence_digest: Some("7".repeat(64)),
            anchor_status: AnchorEvidenceStatus::CompletePassedStoredClosure,
            evidence_scope: EvaluationEvidenceScope::ProgramFixture,
            cost_evidence_scope: Some(CostEvidenceScope::TicketExecutionOnly),
            revoke_watermark: Some((1, "8".repeat(64))),
            dependency_status: DependencyEvidenceStatus::NamespaceWatermarkOnly,
            promotion_eligible: true,
            ineligibility_reasons: vec![],
        };
        let approver = Context::new("n", "approver", Role::Admin).unwrap();
        let host = Context::new("n", "host", Role::Host).unwrap();
        assert!(validate_approval_actor(&approver, &candidate, &view).is_ok());
        assert!(validate_approval_actor(&host, &candidate, &view).is_err());
        let release = PersistentRelease {
            id: "release-1".into(),
            schema_version: PERSISTENT_RELEASE_SCHEMA.into(),
            candidate_id: candidate.id.clone(),
            bundle_digest: candidate.bundle_digest.clone(),
            environment_digest: candidate.environment_digest.clone(),
            profile_id: candidate.profile_id.clone(),
            parent_digest: candidate.parent_digest.clone(),
            baseline_bundle_digest: view.baseline_bundle_digest.clone(),
            report_id: view.report_id.clone(),
            report_digest: view.report_digest.clone(),
            proposer_actor: view.proposer_actor.clone(),
            evaluator_actor: view.evaluator_actor.clone(),
            approved_by: view.approver_actor.clone(),
            expected_parent_release_id: None,
            expected_pointer_epoch: 0,
            state: PersistentReleaseState::Active,
        };
        assert!(validate_persisted_approval_binding(&release, &candidate).is_ok());
        assert!(validate_stored_release_identity(&release, &candidate, &view).is_ok());
        assert!(validate_verified_report_identity(&candidate, &view).is_err());
    }

    #[tokio::test]
    async fn repeated_approval_cannot_regress_active_release_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("approval.sqlite3"))
            .await
            .unwrap();
        let admin = Context::new("n", "approver", Role::Admin).unwrap();
        let existing = PersistentRelease {
            id: "release-1".into(),
            schema_version: PERSISTENT_RELEASE_SCHEMA.into(),
            candidate_id: "candidate-1".into(),
            bundle_digest: "a".repeat(64),
            environment_digest: "b".repeat(64),
            profile_id: "p1".into(),
            parent_digest: "c".repeat(64),
            baseline_bundle_digest: "d".repeat(64),
            report_id: "report-1".into(),
            report_digest: "e".repeat(64),
            proposer_actor: "proposer".into(),
            evaluator_actor: "evaluator".into(),
            approved_by: "approver".into(),
            expected_parent_release_id: None,
            expected_pointer_epoch: 0,
            state: PersistentReleaseState::Active,
        };
        let mut session = store.session().await.unwrap();
        // This fixture exercises the actual private content/idempotence primitives,
        // not the formal writer or a formal P issuer.
        write_release_content(&mut session, &admin, &existing)
            .await
            .unwrap();
        session.commit().await.unwrap();

        let mut proposed = existing.clone();
        proposed.state = PersistentReleaseState::Approved;
        proposed.expected_parent_release_id = Some("new-current-parent".into());
        proposed.expected_pointer_epoch = 99;
        let mut session = store.session().await.unwrap();
        let returned = existing_release_approval_content(&mut session, &admin, &proposed)
            .await
            .unwrap()
            .unwrap();
        session.commit().await.unwrap();
        assert_eq!(returned.state, PersistentReleaseState::Active);
        assert_eq!(returned.expected_pointer_epoch, 0);

        let mut conflicting = existing.clone();
        conflicting.report_digest = "f".repeat(64);
        let mut session = store.session().await.unwrap();
        assert!(
            existing_release_approval_content(&mut session, &admin, &conflicting)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn authorized_state_writes_keep_latest_witness_and_reject_old_body_replay() {
        // A component test of the actual production Admin content writer. No candidate, report or
        // formal_parent is created, and this record cannot confer ordinary P.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state-witness.sqlite3"))
            .await
            .unwrap();
        let admin = Context::new("n", "approver", Role::Admin).unwrap();
        let worker = Context::new("n", "worker", Role::Worker).unwrap();
        let mut current = PersistentRelease {
            id: "component-release".into(),
            schema_version: PERSISTENT_RELEASE_SCHEMA.into(),
            candidate_id: "absent-candidate".into(),
            bundle_digest: "a".repeat(64),
            environment_digest: "b".repeat(64),
            profile_id: "p".into(),
            parent_digest: "c".repeat(64),
            baseline_bundle_digest: "d".repeat(64),
            report_id: "absent-report".into(),
            report_digest: "e".repeat(64),
            proposer_actor: "proposer".into(),
            evaluator_actor: "evaluator".into(),
            approved_by: "approver".into(),
            expected_parent_release_id: None,
            expected_pointer_epoch: 0,
            state: PersistentReleaseState::Approved,
        };
        let mut prior = None;
        for state in [
            PersistentReleaseState::Approved,
            PersistentReleaseState::Canary,
            PersistentReleaseState::Active,
            PersistentReleaseState::Superseded,
            PersistentReleaseState::RolledBack,
            PersistentReleaseState::Active,
        ] {
            current.state = state;
            let mut session = store.session().await.unwrap();
            write_release_content(&mut session, &admin, &current)
                .await
                .unwrap();
            require_body_content_witness(
                &mut session,
                &worker,
                "release",
                &current.id,
                &current,
                Role::Admin,
            )
            .await
            .unwrap();
            assert!(
                session
                    .get::<serde_json::Value>(&worker, "formal_parent", &current.bundle_digest)
                    .await
                    .unwrap()
                    .is_none()
            );
            session.commit().await.unwrap();
            if let Some(old) = &prior {
                let mut session = store.session().await.unwrap();
                session
                    .put(&worker, "release", &current.id, "approver", old)
                    .await
                    .unwrap();
                let operation = format!(
                    "formal.admin.content.v1.{}",
                    fingerprint(&("n", "release", &current.id)).unwrap()
                );
                let entity = fingerprint(&(
                    "n",
                    "release",
                    &current.id,
                    crate::formal_authority::content_digest(old).unwrap(),
                ))
                .unwrap();
                session.audit(&worker, &operation, &entity).await.unwrap();
                assert!(
                    require_body_content_witness(
                        &mut session,
                        &worker,
                        "release",
                        &current.id,
                        old,
                        Role::Admin
                    )
                    .await
                    .is_err()
                );
                session
                    .put(&worker, "release", &current.id, "worker", &current)
                    .await
                    .unwrap();
                require_body_content_witness(
                    &mut session,
                    &worker,
                    "release",
                    &current.id,
                    &current,
                    Role::Admin,
                )
                .await
                .unwrap();
                session.commit().await.unwrap();
            }
            prior = Some(current.clone());
        }
        let mut session = store.session().await.unwrap();
        assert!(
            write_release_content(&mut session, &worker, &current)
                .await
                .is_err()
        );
        drop(session);
        store.close().await;
    }

    #[tokio::test]
    async fn content_primitives_never_authorize_formal_writes_and_missing_inputs_leave_no_side_effects()
     {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("formal-guard.sqlite3"))
            .await
            .unwrap();
        let admin = Context::new("n", "approver", Role::Admin).unwrap();
        let worker = Context::new("n", "worker", Role::Worker).unwrap();
        let component = PersistentRelease {
            id: "component-only".into(),
            schema_version: PERSISTENT_RELEASE_SCHEMA.into(),
            candidate_id: "missing-candidate".into(),
            bundle_digest: "a".repeat(64),
            environment_digest: "b".repeat(64),
            profile_id: "unregistered-profile".into(),
            parent_digest: "c".repeat(64),
            baseline_bundle_digest: "d".repeat(64),
            report_id: "missing-report".into(),
            report_digest: "e".repeat(64),
            proposer_actor: "worker".into(),
            evaluator_actor: "evaluator".into(),
            approved_by: "approver".into(),
            expected_parent_release_id: None,
            expected_pointer_epoch: 0,
            state: PersistentReleaseState::Approved,
        };
        let mut session = store.session().await.unwrap();
        write_release_content(&mut session, &admin, &component)
            .await
            .unwrap();
        session.commit().await.unwrap();

        // A real Admin content witness is insufficient for any formal consumer.
        assert!(matches!(
            ReleaseStore::approve_verified(
                &admin,
                &store,
                &component.candidate_id,
                &component.report_id
            )
            .await,
            Err(Error::NotFound)
        ));
        assert!(matches!(
            ReleaseStore::advance_canary(&admin, &store, &component.id).await,
            Err(Error::NotFound)
        ));
        let mut changed = component.clone();
        changed.state = PersistentReleaseState::Canary;
        let mut session = store.session().await.unwrap();
        assert!(matches!(
            write_release_state(&mut session, &admin, &changed).await,
            Err(Error::NotFound)
        ));
        // Deliberately commit after the rejected call: no changed business body
        // or new content witness was written before the missing-candidate gate.
        session.commit().await.unwrap();
        let mut session = store.session().await.unwrap();
        let kept: PersistentRelease = session
            .need(&admin, "release", &component.id)
            .await
            .unwrap();
        assert_eq!(
            fingerprint(&kept).unwrap(),
            fingerprint(&component).unwrap()
        );
        require_body_content_witness(
            &mut session,
            &worker,
            "release",
            &component.id,
            &component,
            Role::Admin,
        )
        .await
        .unwrap();
        assert!(
            require_body_content_witness(
                &mut session,
                &worker,
                "release",
                &changed.id,
                &changed,
                Role::Admin
            )
            .await
            .is_err()
        );
        assert!(matches!(
            put_release_approval_idempotent(&mut session, &admin, component.clone()).await,
            Err(Error::NotFound)
        ));
        assert!(matches!(
            revalidate_release(&mut session, &admin, &component).await,
            Err(Error::NotFound)
        ));
        session.commit().await.unwrap();

        let mut session = store.session().await.unwrap();
        assert!(matches!(
            cas_pointer_in_session(
                &mut session,
                &admin,
                &component.profile_id,
                0,
                None,
                component.id.clone(),
                component.bundle_digest.clone()
            )
            .await,
            Err(Error::NotFound)
        ));
        // Missing registered profile/opaque authority cannot leak a pointer,
        // even when the caller commits the failed operation's Session.
        session.commit().await.unwrap();
        let mut session = store.session().await.unwrap();
        let pointer_id = format!("profile-pointer-{}", component.profile_id);
        assert!(
            session
                .get::<serde_json::Value>(&admin, "pointer", &pointer_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            session
                .get::<serde_json::Value>(&admin, "formal_parent", &component.bundle_digest)
                .await
                .unwrap()
                .is_none()
        );
        let mut attempted_pointer =
            load_pointer_content(&mut session, &admin, &component.profile_id)
                .await
                .unwrap();
        cas_pointer(
            &mut attempted_pointer,
            0,
            None,
            component.id.clone(),
            component.bundle_digest.clone(),
        )
        .unwrap();
        assert!(
            require_body_content_witness(
                &mut session,
                &admin,
                "pointer",
                &pointer_id,
                &attempted_pointer,
                Role::Admin
            )
            .await
            .is_err()
        );
        session.commit().await.unwrap();
        store.close().await;
    }
}

pub const RELEASE_CANDIDATE_SCHEMA_V2: &str = "rsia.release_candidate.v2";
pub const STORED_BINDING_SCHEMA_V2: &str = "rsia.persisted_compiler_binding.v2";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum StageRequestWire {
    Current(CurrentStageBundleRequest),
    Legacy(StageBundleRequest),
}
impl CurrentStageBundleRequest {
    pub(crate) fn candidate(&self) -> Result<CurrentReleaseCandidateRecord> {
        Ok(CurrentReleaseCandidateRecord {
            id: self.candidate_id.clone(),
            schema_version: RELEASE_CANDIDATE_SCHEMA_V2.into(),
            bundle: self.bundle.clone(),
            bundle_digest: self.bundle.digest.clone(),
            environment_digest: self.environment_digest.clone(),
            profile_id: self.bundle.profile_id.clone(),
            parent_digest: self.bundle.parent_digest.clone(),
            proposer_actor: self.proposer_actor.clone(),
            sources: canonical_sources(self.sources.clone())?,
            revoke_watermark: self.revoke_watermark,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentStageBundleRequest {
    pub candidate_id: String,
    pub bundle: evo_core::contract::ResolvedBundleV3,
    pub environment_digest: String,
    pub proposer_actor: String,
    pub sources: Vec<TypedSourceRef>,
    pub revoke_watermark: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentReleaseCandidateRecord {
    pub id: String,
    pub schema_version: String,
    pub bundle: evo_core::contract::ResolvedBundleV3,
    pub bundle_digest: String,
    pub environment_digest: String,
    pub profile_id: String,
    pub parent_digest: String,
    pub proposer_actor: String,
    pub sources: Vec<TypedSourceRef>,
    pub revoke_watermark: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CurrentPersistedCompilerBinding {
    pub id: String,
    pub schema_version: String,
    pub candidate_id: String,
    pub binding: crate::formal_authority::CurrentFormalCompilerInputBinding,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ReadCandidateWire {
    Current(CurrentReleaseCandidateRecord),
    Legacy(ReleaseCandidateRecord),
}
/// Immutable view only; it has no Serialize or authority constructor.
#[derive(Debug, Clone)]
pub struct ReadCandidateView {
    pub id: String,
    pub schema_version: String,
    pub bundle: crate::compiler::ReadBundle,
    pub bundle_digest: String,
    pub environment_digest: String,
    pub profile_id: String,
    pub parent_digest: String,
    pub proposer_actor: String,
    pub sources: Vec<TypedSourceRef>,
    pub revoke_watermark: u64,
}
#[derive(Debug, Clone)]
pub struct ReadCandidate {
    wire: Box<ReadCandidateWire>,
    view: Box<ReadCandidateView>,
}
impl ReadCandidate {
    fn from_wire(wire: ReadCandidateWire) -> Result<Self> {
        let view = match &wire {
            ReadCandidateWire::Current(record) => {
                if record.schema_version != RELEASE_CANDIDATE_SCHEMA_V2 {
                    return Err(Error::Invalid(
                        "unsupported current ReadCandidate schema".into(),
                    ));
                }
                ReadCandidateView {
                    id: record.id.clone(),
                    schema_version: record.schema_version.clone(),
                    bundle: crate::compiler::ReadBundle::from_current(record.bundle.clone())?,
                    bundle_digest: record.bundle_digest.clone(),
                    environment_digest: record.environment_digest.clone(),
                    profile_id: record.profile_id.clone(),
                    parent_digest: record.parent_digest.clone(),
                    proposer_actor: record.proposer_actor.clone(),
                    sources: record.sources.clone(),
                    revoke_watermark: record.revoke_watermark,
                }
            }
            ReadCandidateWire::Legacy(record) => {
                if record.schema_version != RELEASE_CANDIDATE_SCHEMA {
                    return Err(Error::Invalid(
                        "unsupported legacy ReadCandidate schema".into(),
                    ));
                }
                ReadCandidateView {
                    id: record.id.clone(),
                    schema_version: record.schema_version.clone(),
                    bundle: crate::compiler::ReadBundle::from_legacy(record.bundle.clone())?,
                    bundle_digest: record.bundle_digest.clone(),
                    environment_digest: record.environment_digest.clone(),
                    profile_id: record.profile_id.clone(),
                    parent_digest: record.parent_digest.clone(),
                    proposer_actor: record.proposer_actor.clone(),
                    sources: record.sources.clone(),
                    revoke_watermark: record.revoke_watermark,
                }
            }
        };
        let value = Self {
            wire: Box::new(wire),
            view: Box::new(view),
        };

        Ok(value)
    }
    pub(crate) fn from_current(record: CurrentReleaseCandidateRecord) -> Result<Self> {
        Self::from_wire(ReadCandidateWire::Current(record))
    }
    pub(crate) fn is_current(&self) -> bool {
        matches!(self.wire.as_ref(), ReadCandidateWire::Current(_))
    }

    pub(crate) fn validate_content(&self) -> Result<()> {
        identifier(&self.id)?;
        identifier(&self.profile_id)?;
        identifier(&self.proposer_actor)?;
        validate_digest(&self.environment_digest, "environment_digest")?;
        validate_digest(&self.bundle_digest, "bundle_digest")?;
        validate_digest(&self.parent_digest, "parent_digest")?;
        self.bundle.validate_identity()?;
        if self.bundle_digest != self.bundle.digest
            || self.profile_id != self.bundle.profile_id
            || self.parent_digest != self.bundle.parent_digest
            || self.sources.is_empty()
            || canonical_sources(self.sources.clone())? != self.sources
        {
            return Err(Error::Conflict(
                "release candidate identity is incomplete or noncanonical".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn current(&self) -> Result<&CurrentReleaseCandidateRecord> {
        self.validate_content()?;
        match self.wire.as_ref() {
            ReadCandidateWire::Current(record) => Ok(record),
            ReadCandidateWire::Legacy(_) => Err(Error::Conflict(
                "legacy candidate is not Current input".into(),
            )),
        }
    }
}
impl std::ops::Deref for ReadCandidate {
    type Target = ReadCandidateView;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}
impl Serialize for ReadCandidate {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self.wire.as_ref() {
            ReadCandidateWire::Current(record) => record.serialize(serializer),
            ReadCandidateWire::Legacy(record) => record.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for ReadCandidate {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let wire = ReadCandidateWire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(serde::de::Error::custom)
    }
}

impl ReleaseStore {
    pub async fn stage_current_bundle(
        ctx: &Context,
        store: &Store,
        request: CurrentStageBundleRequest,
    ) -> Result<CurrentReleaseCandidateRecord> {
        let mut session = store.session().await?;
        let candidate = Self::stage_current_bundle_in_session(ctx, &mut session, request).await?;
        session.commit().await?;
        Ok(candidate)
    }
    pub async fn stage_current_bundle_in_session(
        ctx: &Context,
        session: &mut Session,
        request: CurrentStageBundleRequest,
    ) -> Result<CurrentReleaseCandidateRecord> {
        ctx.require(&[Role::Worker, Role::Admin])?;
        validate_current_stage_request(ctx, &request)?;
        let candidate = CurrentReleaseCandidateRecord {
            id: request.candidate_id,
            schema_version: RELEASE_CANDIDATE_SCHEMA_V2.into(),
            bundle_digest: request.bundle.digest.clone(),
            environment_digest: request.environment_digest,
            profile_id: request.bundle.profile_id.clone(),
            parent_digest: request.bundle.parent_digest.clone(),
            proposer_actor: request.proposer_actor,
            sources: canonical_sources(request.sources)?,
            revoke_watermark: request.revoke_watermark,
            bundle: request.bundle,
        };
        let existing = session
            .get::<ReadCandidate>(ctx, "artifact", &candidate.id)
            .await?;
        if let Some(existing) = &existing {
            existing.validate_content()?;
            if existing.id != candidate.id {
                return Err(Error::Conflict(
                    "stored candidate physical identity differs".into(),
                ));
            }
        }
        if let Some(existing) = &existing
            && (!existing.is_current() || fingerprint(&existing)? != fingerprint(&candidate)?)
        {
            return Err(Error::Conflict(
                "candidate id reused with different content".into(),
            ));
        }
        let binding = crate::formal_authority::resolve_current_bundle_in_session(
            session,
            ctx,
            &candidate.bundle,
            &candidate.environment_digest,
            candidate.revoke_watermark,
            evo_core::evidence::Purpose::Development,
        )
        .await?;
        validate_current_candidate_sources(session, ctx, &candidate).await?;
        crate::formal_authority::preflight_current_candidate_principal(
            session,
            ctx,
            binding.binding(),
            &candidate,
        )
        .await?;
        crate::compiler::preflight_current_authenticated_bundle(
            session,
            ctx,
            &binding.request_for_search_parent(candidate.bundle.skill.clone()),
            &candidate.bundle,
        )
        .await?;
        let preflight_binding_id = format!(
            "compiler-binding-{}",
            fingerprint(&(&candidate.id, &candidate.bundle_digest))?
        );
        let preflight_binding = CurrentPersistedCompilerBinding {
            id: preflight_binding_id.clone(),
            schema_version: STORED_BINDING_SCHEMA_V2.into(),
            candidate_id: candidate.id.clone(),
            binding: binding.binding().clone(),
        };
        if let Some(old) = session
            .get::<serde_json::Value>(ctx, "artifact", &preflight_binding_id)
            .await?
            && crate::formal_authority::content_digest(&old)?
                != crate::formal_authority::content_digest(&preflight_binding)?
        {
            return Err(Error::Conflict(
                "compiler binding identity collision".into(),
            ));
        }

        crate::compiler::persist_current_authenticated_bundle(
            session,
            ctx,
            &binding.request_for_search_parent(candidate.bundle.skill.clone()),
            &candidate.bundle,
        )
        .await?;
        if let Some(existing) = existing {
            let existing = existing.current()?.clone();
            crate::formal_authority::bind_current_candidate_principal_from_binding(
                session,
                ctx,
                binding.binding(),
                &existing,
            )
            .await?;
            return Ok(existing);
        }
        session
            .put(ctx, "artifact", &candidate.id, ctx.actor(), &candidate)
            .await?;
        for source in &candidate.sources {
            session
                .put_edge(ctx, "artifact", &candidate.id, &source.kind, &source.id)
                .await?;
        }
        session
            .put_edge(
                ctx,
                "artifact",
                &candidate.id,
                "artifact",
                &format!("formal-bundle-{}", candidate.bundle_digest),
            )
            .await?;

        let binding_id = format!(
            "compiler-binding-{}",
            fingerprint(&(&candidate.id, &candidate.bundle_digest))?
        );
        let binding_record = CurrentPersistedCompilerBinding {
            id: binding_id.clone(),
            schema_version: STORED_BINDING_SCHEMA_V2.into(),
            candidate_id: candidate.id.clone(),
            binding: binding.binding().clone(),
        };
        if let Some(old) = session
            .get::<serde_json::Value>(ctx, "artifact", &binding_id)
            .await?
        {
            if crate::formal_authority::content_digest(&old)?
                != crate::formal_authority::content_digest(&binding_record)?
            {
                return Err(Error::Conflict(
                    "compiler binding identity collision".into(),
                ));
            }
        } else {
            session
                .put(ctx, "artifact", &binding_id, ctx.actor(), &binding_record)
                .await?;
            for node in &binding.binding().closure.nodes {
                session
                    .put_edge(ctx, "artifact", &binding_id, &node.kind, &node.id)
                    .await?;
            }
        }
        session
            .put_edge(ctx, "artifact", &candidate.id, "artifact", &binding_id)
            .await?;
        crate::formal_authority::bind_current_candidate_principal_from_binding(
            session,
            ctx,
            binding.binding(),
            &candidate,
        )
        .await?;
        session
            .audit(ctx, "release.candidate.stage", &candidate.id)
            .await?;
        Ok(candidate)
    }
}
fn validate_current_stage_request(
    ctx: &Context,
    request: &CurrentStageBundleRequest,
) -> Result<()> {
    identifier(&request.candidate_id)?;
    identifier(&request.proposer_actor)?;
    if request.proposer_actor != ctx.actor() {
        return Err(Error::Forbidden);
    }
    validate_digest(&request.environment_digest, "environment_digest")?;
    validate_current_bundle_identity(&request.bundle)?;
    crate::formal_authority::require_builtin_parent_strategy(&request.bundle.improver)?;
    if request.sources.is_empty() {
        return Err(Error::Invalid(
            "release candidate requires source closure".into(),
        ));
    }
    Ok(())
}
async fn validate_current_candidate_sources(
    session: &mut Session,
    ctx: &Context,
    candidate: &CurrentReleaseCandidateRecord,
) -> Result<()> {
    let ids = candidate
        .sources
        .iter()
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    validate_stored_sources(session, ctx, &ids, candidate.revoke_watermark).await?;
    for source in &candidate.sources {
        let authority = load_stored_source(session, ctx, &source.id).await?;
        if authority.trace.source_digest != source.content_digest {
            return Err(Error::Conflict("source content digest changed".into()));
        }
    }
    Ok(())
}
pub(crate) async fn validate_current_candidate_read_in_session(
    session: &mut Session,
    ctx: &Context,
    candidate: &CurrentReleaseCandidateRecord,
) -> Result<()> {
    if candidate.schema_version != RELEASE_CANDIDATE_SCHEMA_V2 {
        return Err(Error::Invalid(
            "unsupported release candidate schema".into(),
        ));
    }
    identifier(&candidate.id)?;
    identifier(&candidate.profile_id)?;
    identifier(&candidate.proposer_actor)?;
    validate_digest(&candidate.environment_digest, "environment_digest")?;
    validate_digest(&candidate.bundle_digest, "bundle_digest")?;
    validate_digest(&candidate.parent_digest, "parent_digest")?;
    validate_current_bundle_identity(&candidate.bundle)?;
    crate::formal_authority::require_builtin_parent_strategy(&candidate.bundle.improver)?;
    if candidate.bundle_digest != candidate.bundle.digest
        || candidate.profile_id != candidate.bundle.profile_id
        || candidate.parent_digest != candidate.bundle.parent_digest
        || candidate.sources.is_empty()
        || canonical_sources(candidate.sources.clone())? != candidate.sources
    {
        return Err(Error::Conflict(
            "release candidate identity is incomplete or noncanonical".into(),
        ));
    }
    validate_current_candidate_sources(session, ctx, candidate).await
}

fn validate_current_bundle_identity(bundle: &evo_core::contract::ResolvedBundleV3) -> Result<()> {
    if bundle.schema_version != evo_core::contract::BUNDLE_SCHEMA_V3
        || bundle.compiler_version != evo_core::contract::BUNDLE_COMPILER_VERSION_V3
        || evo_core::contract::resolved_bundle_digest_v3(bundle)? != bundle.digest
    {
        return Err(Error::Conflict(
            "Current resolved bundle identity differs".into(),
        ));
    }
    identifier(&bundle.profile_id)?;
    bundle.skill.validate()?;
    bundle.improver.validate()?;
    Ok(())
}
pub(crate) async fn validate_read_candidate_in_session(
    s: &mut Session,
    ctx: &Context,
    candidate: &ReadCandidate,
) -> Result<()> {
    match candidate.wire.as_ref() {
        ReadCandidateWire::Current(record) => {
            validate_current_candidate_read_in_session(s, ctx, record).await
        }
        ReadCandidateWire::Legacy(record) => {
            validate_candidate_read_in_session(s, ctx, record).await
        }
    }
}

fn validate_read_verified_report_identity(
    candidate: &ReadCandidate,
    view: &crate::streaming_evaluator::VerifiedFormalReportView,
) -> Result<()> {
    if view.variant != VerifiedReportVariant::CompleteBatch
        || !matches!(view.verdict, Some(Verdict::Improved | Verdict::Noninferior))
        || view.anchor_status != AnchorEvidenceStatus::CompletePassedStoredClosure
        || view.anchor_evidence_digest.is_none()
        || view.resource_evidence_digest.is_none()
        || view.evidence_scope != EvaluationEvidenceScope::RegisteredExternalHost
        || view.cost_evidence_scope != Some(CostEvidenceScope::CompleteEpisodeRoot)
        || view.dependency_status != DependencyEvidenceStatus::CompleteTypedContentClosure
        || !view.promotion_eligible
    {
        return Err(Error::Forbidden);
    }
    if view.candidate_digest != candidate.id
        || view.candidate_bundle_digest != candidate.bundle_digest
        || view.baseline_parent_digest != candidate.parent_digest
        || view.environment_digest != candidate.environment_digest
        || view.proposer_actor != candidate.proposer_actor
    {
        return Err(Error::Conflict(
            "verified report does not match release candidate".into(),
        ));
    }
    let watermark = view
        .revoke_watermark
        .as_ref()
        .and_then(|(seq, _)| u64::try_from(*seq).ok());
    if watermark != Some(candidate.revoke_watermark) {
        return Err(Error::Conflict("report revoke watermark mismatch".into()));
    }
    Ok(())
}
fn validate_read_approval_actor(
    ctx: &Context,
    candidate: &ReadCandidate,
    view: &crate::streaming_evaluator::VerifiedFormalReportView,
) -> Result<()> {
    if view.approver_actor != ctx.actor()
        || ctx.actor() == candidate.proposer_actor
        || ctx.actor() == view.evaluator_actor
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
async fn validate_read_candidate_sources(
    session: &mut Session,
    ctx: &Context,
    candidate: &ReadCandidate,
) -> Result<()> {
    let ids = candidate
        .sources
        .iter()
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    validate_stored_sources(session, ctx, &ids, candidate.revoke_watermark).await?;
    for source in &candidate.sources {
        let authority = load_stored_source(session, ctx, &source.id).await?;
        if authority.trace.source_digest != source.content_digest {
            return Err(Error::Conflict("source content digest changed".into()));
        }
    }
    Ok(())
}
fn validate_read_persisted_approval_binding(
    release: &PersistentRelease,
    candidate: &ReadCandidate,
) -> Result<()> {
    identifier(&release.report_id)?;
    validate_digest(&release.report_digest, "release report digest")?;
    if candidate.id != release.candidate_id
        || candidate.bundle_digest != release.bundle_digest
        || candidate.environment_digest != release.environment_digest
        || candidate.parent_digest != release.parent_digest
        || candidate.proposer_actor != release.proposer_actor
        || release.approved_by == release.proposer_actor
        || release.approved_by == release.evaluator_actor
    {
        return Err(Error::Conflict(
            "stored release approval binding is inconsistent".into(),
        ));
    }
    Ok(())
}
fn validate_read_stored_release_identity(
    release: &PersistentRelease,
    candidate: &ReadCandidate,
    view: &crate::streaming_evaluator::VerifiedFormalReportView,
) -> Result<()> {
    if view.report_digest != release.report_digest
        || candidate.bundle_digest != release.bundle_digest
        || candidate.environment_digest != release.environment_digest
        || release.proposer_actor != view.proposer_actor
        || release.evaluator_actor != view.evaluator_actor
        || release.approved_by != view.approver_actor
        || release.approved_by == release.proposer_actor
        || release.approved_by == release.evaluator_actor
    {
        return Err(Error::Conflict(
            "release evidence changed after approval".into(),
        ));
    }
    Ok(())
}
async fn revalidate_current_release(
    session: &mut Session,
    ctx: &Context,
    release: &PersistentRelease,
) -> Result<()> {
    require_body_witness(session, ctx, "release", &release.id, release, Role::Admin).await?;
    let candidate: ReadCandidate = session.need(ctx, "artifact", &release.candidate_id).await?;
    candidate.current()?;
    validate_read_candidate_sources(session, ctx, &candidate).await?;
    validate_read_persisted_approval_binding(release, &candidate)?;
    crate::formal_authority::verify_read_release_principal(session, ctx, release, &candidate)
        .await?;
    let view = if ctx.role() == Role::Host {
        verified_report_view_for_host_in_session(ctx, session, &release.report_id).await?
    } else {
        verified_report_view_in_session(ctx, session, &release.report_id).await?
    };
    validate_read_verified_report_identity(&candidate, &view)?;
    validate_read_stored_release_identity(release, &candidate, &view)
}
fn project_read_run_surface_v2(
    surface: &HostSurfaceRecordV2,
    capabilities: &HostCapabilities,
    bundle: &crate::compiler::ReadBundle,
    snapshot: &SystemSnapshot,
) -> Result<RunProjection> {
    if !capabilities.allows(&bundle.skill.required_capabilities) {
        return Err(Error::Forbidden);
    }
    let projection = crate::compiler::projection_for_read_bundle(bundle, snapshot, true)?;
    // SystemSnapshot.tools is the unchanged host baseline. The only candidate
    // content emitted by the existing project_run is skill.content.
    if !projection.instructions.is_empty()
        && !host_surface_v2_allows_field(&surface.manifest, capabilities, "skill.content")
    {
        return Err(Error::Forbidden);
    }
    Ok(projection)
}

async fn require_current_run_source(
    s: &mut Session,
    ctx: &Context,
    snapshot: &LoadedRunApplicationSnapshot,
) -> Result<()> {
    if let Some(id) = &snapshot.release_id {
        let release: PersistentRelease = s.need(ctx, "release", id).await?;
        revalidate_current_release(s, ctx, &release).await?;
    }
    Ok(())
}
