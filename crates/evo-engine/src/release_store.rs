//! Persistent release control. Production transitions consume stored evidence by id.

use crate::evidence::{load_stored_source, validate_stored_sources};
use crate::releases::validate_resolved_bundle_identity;
use crate::streaming_evaluator::{
    AnchorEvidenceStatus, CostEvidenceScope, DependencyEvidenceStatus, EvaluationEvidenceScope,
    VerifiedReportVariant, verified_report_view_for_host_in_session,
    verified_report_view_in_session,
};
use evo_core::contract::{
    AppliedReceipt, CapabilityLevel, HostCapabilities, HostSurfaceManifest, ResolvedBundle,
    RunProjection, SystemSnapshot, project_run,
};
use evo_core::evaluation::Verdict;
use evo_core::{Context, Error, Result, Role, fingerprint, identifier};
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

#[derive(Debug, Clone)]
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
    pub schema_version: String,
    pub manifest: HostSurfaceManifest,
    pub manifest_digest: String,
    pub extracted: Vec<String>,
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

pub struct ReleaseStore;

impl ReleaseStore {
    pub async fn stage_bundle(
        ctx: &Context,
        store: &Store,
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
        let mut session = store.session().await?;
        validate_candidate_sources(&mut session, ctx, &candidate).await?;
        if let Some(existing) = session
            .get::<ReleaseCandidateRecord>(ctx, "artifact", &candidate.id)
            .await?
        {
            if fingerprint(&existing)? != fingerprint(&candidate)? {
                return Err(Error::Conflict(
                    "candidate id reused with different content".into(),
                ));
            }
            session.commit().await?;
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
            .audit(ctx, "release.candidate.stage", &candidate.id)
            .await?;
        session.commit().await?;
        Ok(candidate)
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
        let candidate: ReleaseCandidateRecord = session.need(ctx, "artifact", candidate_id).await?;
        if candidate.proposer_actor == ctx.actor() {
            return Err(Error::Forbidden);
        }
        validate_candidate_sources(&mut session, ctx, &candidate).await?;
        let view = verified_report_view_in_session(ctx, &mut session, report_id).await?;
        validate_verified_report_identity(&candidate, &view)?;
        validate_approval_actor(ctx, &candidate, &view)?;
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
        if release.state != PersistentReleaseState::Approved {
            session.commit().await?;
            return Ok(release);
        }
        session
            .put_edge(ctx, "release", &release.id, "artifact", &candidate.id)
            .await?;
        session
            .put_edge(ctx, "release", &release.id, "artifact", &release.report_id)
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
        revalidate_release(&mut session, ctx, &release).await?;
        release.state = PersistentReleaseState::Canary;
        session
            .put(ctx, "release", release_id, ctx.actor(), &release)
            .await?;
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
        revalidate_release(&mut session, ctx, &release).await?;
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
            previous.state = PersistentReleaseState::Superseded;
            session
                .put(ctx, "release", previous_id, ctx.actor(), &previous)
                .await?;
        }
        release.state = PersistentReleaseState::Active;
        session
            .put(ctx, "release", release_id, ctx.actor(), &release)
            .await?;
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
            session.commit().await?;
            return Ok(existing);
        }
        session
            .put(ctx, "artifact", id, ctx.actor(), &record)
            .await?;
        session.audit(ctx, "host_surface.register", id).await?;
        session.commit().await?;
        Ok(record)
    }

    pub async fn prepare_run(
        ctx: &Context,
        store: &Store,
        request: PrepareRunRequest,
    ) -> Result<RunApplicationSnapshot> {
        ctx.require(&[Role::Host])?;
        validate_prepare_request(&request)?;
        let snapshot_id = format!("run-snapshot-{}", request.run_id);
        let requested_environment_digest = request.system_snapshot.digest()?;
        let requested_caps_digest = fingerprint(&request.host_capabilities)?;
        let mut session = store.session().await?;
        if let Some(existing) = session
            .get::<RunApplicationSnapshot>(ctx, "artifact", &snapshot_id)
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
            let existing =
                Self::validate_run_snapshot_live_in_session(ctx, &mut session, &request.run_id)
                    .await?;
            session.commit().await?;
            return Ok(existing);
        }
        let surface: HostSurfaceRecord = session
            .need(ctx, "artifact", &request.host_surface_id)
            .await?;
        surface
            .manifest
            .validate_against_extraction(&surface.extracted)?;
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
                revalidate_release(&mut session, ctx, &release).await?;
                let candidate: ReleaseCandidateRecord =
                    session.need(ctx, "artifact", &release.candidate_id).await?;
                if !request
                    .host_capabilities
                    .allows(&candidate.bundle.skill.required_capabilities)
                {
                    return Err(Error::Forbidden);
                }
                (
                    Some(release.id),
                    Some(release.bundle_digest),
                    project_run(&candidate.bundle, &request.system_snapshot, true)?,
                )
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
        let record = RunApplicationSnapshot {
            id: snapshot_id.clone(),
            schema_version: RUN_APPLICATION_SCHEMA.into(),
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
            projection,
            request_digest: fingerprint(&material)?,
            request_material: material,
            capability_level: request.capability_level,
            evolution_enabled: request.evolution_enabled,
        };
        session
            .put(ctx, "artifact", &snapshot_id, ctx.actor(), &record)
            .await?;
        if let Some(release_id) = &record.release_id {
            session
                .put_edge(ctx, "artifact", &snapshot_id, "release", release_id)
                .await?;
        }
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
    ) -> Result<RunApplicationSnapshot> {
        Self::validate_run_snapshot_live(ctx, store, run_id).await
    }

    pub async fn validate_run_snapshot_live(
        ctx: &Context,
        store: &Store,
        run_id: &str,
    ) -> Result<RunApplicationSnapshot> {
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
    ) -> Result<RunApplicationSnapshot> {
        ctx.require(&[Role::Host, Role::Admin])?;
        identifier(run_id)?;
        let snapshot_id = format!("run-snapshot-{run_id}");
        let snapshot: RunApplicationSnapshot = session.need(ctx, "artifact", &snapshot_id).await?;
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
            let candidate: ReleaseCandidateRecord =
                session.need(ctx, "artifact", &release.candidate_id).await?;
            if candidate.bundle_digest != release.bundle_digest
                || candidate.environment_digest != release.environment_digest
            {
                return Err(Error::Conflict(
                    "run release candidate identity changed".into(),
                ));
            }
            revalidate_release(session, ctx, &release).await?;
        } else if snapshot.bundle_digest.is_some() {
            return Err(Error::Conflict(
                "run snapshot has a bundle without an active release".into(),
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
            session.commit().await?;
            return Ok(existing);
        }
        session
            .put(ctx, "receipt", &id, ctx.actor(), &record)
            .await?;
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
            session.commit().await?;
            return Ok(());
        }
        session
            .put(ctx, "artifact", &receipt.id, ctx.actor(), &receipt)
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
        if target.profile_id != profile_id
            || target.environment_digest != environment_digest
            || target.state != PersistentReleaseState::Superseded
        {
            return Err(Error::Conflict(
                "rollback target is not a compatible prior approved release".into(),
            ));
        }
        revalidate_release(&mut session, ctx, &target).await?;
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
        session
            .put(ctx, "release", &current.id, ctx.actor(), &current)
            .await?;
        session
            .put(ctx, "release", &target.id, ctx.actor(), &target)
            .await?;
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

fn validate_verified_report_identity(
    candidate: &ReleaseCandidateRecord,
    view: &crate::streaming_evaluator::VerifiedFormalReportView,
) -> Result<()> {
    if view.variant != VerifiedReportVariant::CompleteBatch
        || !matches!(view.verdict, Some(Verdict::Improved | Verdict::Noninferior))
        || view.anchor_status != AnchorEvidenceStatus::CompletePassedStoredClosure
        || view.anchor_evidence_digest.is_none()
        || view.resource_evidence_digest.is_none()
        || view.evidence_scope == EvaluationEvidenceScope::ProgramFixture
        || view.cost_evidence_scope == Some(CostEvidenceScope::TicketExecutionOnly)
        || view.dependency_status == DependencyEvidenceStatus::NamespaceWatermarkOnly
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
    if let Some(existing) = session
        .get::<PersistentRelease>(ctx, "release", &proposed.id)
        .await?
    {
        if same_release_approval_identity(&existing, &proposed) {
            return Ok(existing);
        }
        return Err(Error::Conflict(
            "release id reused with different approval evidence".into(),
        ));
    }
    session
        .put(ctx, "release", &proposed.id, ctx.actor(), &proposed)
        .await?;
    Ok(proposed)
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

async fn revalidate_release(
    session: &mut Session,
    ctx: &Context,
    release: &PersistentRelease,
) -> Result<()> {
    let candidate: ReleaseCandidateRecord =
        session.need(ctx, "artifact", &release.candidate_id).await?;
    validate_candidate_sources(session, ctx, &candidate).await?;
    validate_persisted_approval_binding(release, &candidate)?;
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
    identifier(profile_id)?;
    let id = format!("profile-pointer-{profile_id}");
    Ok(session
        .get(ctx, "pointer", &id)
        .await?
        .unwrap_or(PersistentProfilePointer {
            id,
            schema_version: PROFILE_POINTER_SCHEMA.into(),
            profile_id: profile_id.into(),
            active_release_id: None,
            active_bundle_digest: None,
            epoch: 0,
        }))
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
    session
        .put(ctx, "pointer", &pointer.id, ctx.actor(), &pointer)
        .await?;
    Ok(pointer)
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
            let pointer = cas_pointer_in_session(
                &mut session,
                &left_admin,
                "p1",
                0,
                None,
                "release-left".into(),
                "a".repeat(64),
            )
            .await?;
            session.commit().await?;
            Ok::<_, Error>(pointer)
        };
        let right = async move {
            let mut session = right_store.session().await?;
            let pointer = cas_pointer_in_session(
                &mut session,
                &right_admin,
                "p1",
                0,
                None,
                "release-right".into(),
                "b".repeat(64),
            )
            .await?;
            session.commit().await?;
            Ok::<_, Error>(pointer)
        };
        let (left, right) = tokio::join!(left, right);
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let mut session = store.session().await.unwrap();
        let pointer = load_pointer(&mut session, &admin, "p1").await.unwrap();
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
        session
            .put(&admin, "release", &existing.id, admin.actor(), &existing)
            .await
            .unwrap();
        session.commit().await.unwrap();

        let mut proposed = existing.clone();
        proposed.state = PersistentReleaseState::Approved;
        proposed.expected_parent_release_id = Some("new-current-parent".into());
        proposed.expected_pointer_epoch = 99;
        let mut session = store.session().await.unwrap();
        let returned = put_release_approval_idempotent(&mut session, &admin, proposed)
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert_eq!(returned.state, PersistentReleaseState::Active);
        assert_eq!(returned.expected_pointer_epoch, 0);

        let mut conflicting = existing.clone();
        conflicting.report_digest = "f".repeat(64);
        let mut session = store.session().await.unwrap();
        assert!(
            put_release_approval_idempotent(&mut session, &admin, conflicting)
                .await
                .is_err()
        );
    }
}
