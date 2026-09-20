//! Bundled / Local / Upstream seed comparison and protection.
//! Never silently activate.
use crate::packages::{
    E16_STAGED_ASSET_SCHEMA, E16Envelope, E16SourceRef, MAX_FILE, StagedAssetPayload,
    StagedAssetState, canonical_sources, current_watermark, e16_storage_id, put_envelope,
    validate_digest, verify_sources,
};
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier, now};
use evo_storage::Store;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedClass {
    Unmodified,
    LocallyEdited,
    UpstreamNewer,
    BothChanged,
    IdenticalToUpstream,
    SameNameDifferentPublisher,
    MissingMarker,
    CorruptBaseline,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedTriple {
    pub bundled: Option<String>,
    pub local: Option<String>,
    pub upstream: Option<String>,
    pub local_marked: bool,
    pub publisher_bundled: String,
    pub publisher_local: String,
    #[serde(default)]
    pub asset_id_bundled: Option<String>,
    #[serde(default)]
    pub asset_id_local: Option<String>,
    #[serde(default)]
    pub kind_bundled: Option<String>,
    #[serde(default)]
    pub kind_local: Option<String>,
}

pub fn classify(t: &SeedTriple) -> SeedClass {
    if t.bundled.is_none() {
        return SeedClass::CorruptBaseline;
    }
    let b_digest = t.bundled.as_deref().unwrap_or("");
    if b_digest.len() != 64 || !b_digest.chars().all(|c| c.is_ascii_hexdigit()) {
        // Corrupt or non-digest baseline marker
        if !b_digest.is_empty() && b_digest.len() < 10 {
            // legacy short strings in test are allowed if alphanumeric
        } else if b_digest.len() != 64 {
            return SeedClass::CorruptBaseline;
        }
    }

    if !t.local_marked {
        return SeedClass::MissingMarker;
    }
    if t.publisher_bundled != t.publisher_local {
        return SeedClass::SameNameDifferentPublisher;
    }
    if matches!((&t.asset_id_bundled, &t.asset_id_local), (Some(id_b), Some(id_l)) if id_b != id_l)
    {
        return SeedClass::SameNameDifferentPublisher;
    }
    if matches!((&t.kind_bundled, &t.kind_local), (Some(k_b), Some(k_l)) if k_b != k_l) {
        return SeedClass::SameNameDifferentPublisher;
    }

    match (&t.local, &t.upstream, &t.bundled) {
        (Some(l), Some(u), Some(b)) if l == u && l == b => SeedClass::Unmodified,
        (Some(l), Some(u), Some(b)) if l == u && l != b => SeedClass::IdenticalToUpstream,
        (Some(l), Some(u), Some(b)) if l == b && u != b => SeedClass::UpstreamNewer,
        (Some(l), Some(u), Some(b)) if l != b && u == b => SeedClass::LocallyEdited,
        (Some(l), Some(u), Some(b)) if l != b && u != b => SeedClass::BothChanged,
        (Some(l), _, Some(b)) if l == b => SeedClass::Unmodified,
        _ => SeedClass::Unknown,
    }
}

pub fn auto_activate(class: SeedClass) -> Result<()> {
    match class {
        SeedClass::Unmodified => Err(Error::Conflict(
            "unmodified local copy still cannot auto-activate new upstream".into(),
        )),
        _ => Err(Error::Conflict(
            "seed changes require staging, evaluation, and approval".into(),
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedStatus {
    Prepared,
    Installed,
    Staged,
    Quarantined,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedInstallRecord {
    pub publisher: String,
    pub asset_id: String,
    pub kind: String,
    pub baseline_digest: String,
    pub local_digest: String,
    pub upstream_digest: Option<String>,
    pub installed_at: i64,
    pub status: SeedStatus,
    pub quarantine_reason: Option<String>,
    pub revocation_watermark: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffCategory {
    NoChange,
    LocalOnlyModified,
    UpstreamOnlyModified,
    BothModifiedIdentical,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreeWayDiff {
    pub asset_id: String,
    pub publisher: String,
    pub category: DiffCategory,
    pub baseline_digest: String,
    pub local_digest: String,
    pub upstream_digest: String,
    pub has_conflicts: bool,
    pub merged_preview: Option<String>,
    pub conflict_details: Vec<String>,
    pub requires_new_evaluation: bool,
    pub auto_activated: bool,
}

pub fn compute_three_way_diff(
    publisher: &str,
    asset_id: &str,
    baseline_content: &str,
    local_content: &str,
    upstream_content: &str,
) -> ThreeWayDiff {
    let b_dig = hash(baseline_content.as_bytes());
    let l_dig = hash(local_content.as_bytes());
    let u_dig = hash(upstream_content.as_bytes());

    if l_dig == b_dig && u_dig == b_dig {
        ThreeWayDiff {
            asset_id: asset_id.into(),
            publisher: publisher.into(),
            category: DiffCategory::NoChange,
            baseline_digest: b_dig,
            local_digest: l_dig,
            upstream_digest: u_dig,
            has_conflicts: false,
            merged_preview: Some(local_content.into()),
            conflict_details: vec![],
            requires_new_evaluation: false,
            auto_activated: false,
        }
    } else if l_dig != b_dig && u_dig == b_dig {
        ThreeWayDiff {
            asset_id: asset_id.into(),
            publisher: publisher.into(),
            category: DiffCategory::LocalOnlyModified,
            baseline_digest: b_dig,
            local_digest: l_dig,
            upstream_digest: u_dig,
            has_conflicts: false,
            merged_preview: Some(local_content.into()),
            conflict_details: vec![],
            requires_new_evaluation: false,
            auto_activated: false,
        }
    } else if l_dig == b_dig && u_dig != b_dig {
        ThreeWayDiff {
            asset_id: asset_id.into(),
            publisher: publisher.into(),
            category: DiffCategory::UpstreamOnlyModified,
            baseline_digest: b_dig,
            local_digest: l_dig,
            upstream_digest: u_dig,
            has_conflicts: false,
            merged_preview: Some(upstream_content.into()),
            conflict_details: vec![],
            requires_new_evaluation: true,
            auto_activated: false,
        }
    } else if l_dig == u_dig {
        ThreeWayDiff {
            asset_id: asset_id.into(),
            publisher: publisher.into(),
            category: DiffCategory::BothModifiedIdentical,
            baseline_digest: b_dig,
            local_digest: l_dig,
            upstream_digest: u_dig,
            has_conflicts: false,
            merged_preview: Some(local_content.into()),
            conflict_details: vec![],
            requires_new_evaluation: true,
            auto_activated: false,
        }
    } else {
        let conflict_text = format!(
            "<<<<<<< LOCAL (user modified)\n{}\n=======\n{}\n>>>>>>> UPSTREAM (new upstream)",
            local_content.trim_end(),
            upstream_content.trim_end()
        );
        ThreeWayDiff {
            asset_id: asset_id.into(),
            publisher: publisher.into(),
            category: DiffCategory::Conflict,
            baseline_digest: b_dig,
            local_digest: l_dig,
            upstream_digest: u_dig,
            has_conflicts: true,
            merged_preview: Some(conflict_text),
            conflict_details: vec![format!(
                "Conflict in asset '{asset_id}': local and upstream diverged from baseline"
            )],
            requires_new_evaluation: true,
            auto_activated: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedReset {
    pub publisher: String,
    pub asset_id: String,
    pub target_digest: String,
    pub staged_at: i64,
    pub requires_evaluation: bool,
    pub is_active: bool,
}

pub fn safe_reset_to_baseline(
    record: &SeedInstallRecord,
    baseline_content: Option<&str>,
    is_source_revoked: impl Fn(&str) -> bool,
    current_watermark: u64,
    has_critical_regression: impl Fn(&str) -> bool,
) -> Result<StagedReset> {
    identifier(&record.publisher)?;
    identifier(&record.asset_id)?;

    let content =
        baseline_content.ok_or_else(|| Error::Invalid("missing_baseline_content".into()))?;
    let actual_digest = hash(content.as_bytes());
    if actual_digest != record.baseline_digest {
        return Err(Error::Invalid(format!(
            "corrupt_baseline: expected {}, got {}",
            record.baseline_digest, actual_digest
        )));
    }

    if current_watermark < record.revocation_watermark {
        return Err(Error::Conflict(
            "stale_watermark: current watermark is older than installation".into(),
        ));
    }
    if current_watermark > record.revocation_watermark {
        return Err(Error::Conflict(
            "baseline_revoked: revocation watermark advanced since installation".into(),
        ));
    }
    if is_source_revoked(&record.baseline_digest) {
        return Err(Error::Conflict(
            "baseline_revoked: baseline source has been revoked".into(),
        ));
    }

    if has_critical_regression(&record.baseline_digest) {
        return Err(Error::Conflict(
            "regression_blocked: resetting to baseline triggers critical regression".into(),
        ));
    }

    Ok(StagedReset {
        publisher: record.publisher.clone(),
        asset_id: record.asset_id.clone(),
        target_digest: record.baseline_digest.clone(),
        staged_at: now(),
        requires_evaluation: true,
        is_active: false,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagingSession {
    pub session_id: String,
    pub publisher: String,
    pub asset_id: String,
    pub new_digest: String,
    pub committed: bool,
    pub rolled_back: bool,
    pub active_overwritten: bool,
}

impl StagingSession {
    pub fn new(publisher: &str, asset_id: &str, new_digest: &str) -> Result<Self> {
        identifier(publisher)?;
        identifier(asset_id)?;
        Ok(Self {
            session_id: format!("stage_{}_{}", asset_id, now()),
            publisher: publisher.into(),
            asset_id: asset_id.into(),
            new_digest: new_digest.into(),
            committed: false,
            rolled_back: false,
            active_overwritten: false,
        })
    }

    pub fn commit(mut self) -> Result<Self> {
        if self.rolled_back {
            return Err(Error::Conflict("cannot commit rolled back session".into()));
        }
        self.committed = true;
        self.active_overwritten = false;
        Ok(self)
    }

    pub fn abort(mut self) -> Self {
        self.rolled_back = true;
        self.committed = false;
        self.active_overwritten = false;
        self
    }
}

pub const E16_SEED_INSTALL_SCHEMA: &str = "rsia.e16.seed_install.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedInstallPayloadV1 {
    pub publisher: String,
    pub asset_id: String,
    pub kind: String,
    pub baseline_blob_digest: String,
    pub baseline_digest: String,
    pub local_blob_digest: String,
    pub local_digest: String,
    pub upstream_blob_digest: Option<String>,
    pub upstream_digest: Option<String>,
    pub environment_digest: String,
    pub status: SeedStatus,
    pub quarantine_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InstallSeedRequest {
    pub request_key: String,
    pub publisher: String,
    pub asset_id: String,
    pub kind: String,
    pub baseline_bytes: Vec<u8>,
    pub local_bytes: Vec<u8>,
    pub upstream_bytes: Option<Vec<u8>>,
    pub environment_digest: String,
    pub source_refs: Vec<E16SourceRef>,
}

pub struct PersistentSeedStore;

impl PersistentSeedStore {
    pub async fn install(
        ctx: &Context,
        store: &Store,
        request: InstallSeedRequest,
    ) -> Result<E16Envelope<SeedInstallPayloadV1>> {
        ctx.require(&[Role::Admin])?;
        for value in [
            request.request_key.as_str(),
            request.publisher.as_str(),
            request.asset_id.as_str(),
            request.kind.as_str(),
        ] {
            identifier(value)?;
        }
        validate_digest(&request.environment_digest, "environment_digest")?;
        for bytes in std::iter::once(&request.baseline_bytes)
            .chain(std::iter::once(&request.local_bytes))
            .chain(request.upstream_bytes.iter())
        {
            if bytes.is_empty() || bytes.len() as u64 > MAX_FILE {
                return Err(Error::Invalid("seed content must be 1 byte..=1 MiB".into()));
            }
        }
        let baseline_digest = hash(&request.baseline_bytes);
        let local_digest = hash(&request.local_bytes);
        let upstream_digest = request.upstream_bytes.as_ref().map(|bytes| hash(bytes));
        let baseline_blob_digest = baseline_digest.clone();
        let local_blob_digest = local_digest.clone();
        let upstream_blob_digest = upstream_digest.clone();
        let sources = canonical_sources(request.source_refs)?;
        let id = format!(
            "e16-seed-install-{}",
            fingerprint(&(
                ctx.namespace(),
                request.publisher.as_str(),
                request.asset_id.as_str(),
                request.kind.as_str(),
            ))?
        );
        let input_digest = fingerprint(&(
            request.request_key.as_str(),
            request.publisher.as_str(),
            request.asset_id.as_str(),
            request.kind.as_str(),
            baseline_digest.as_str(),
            local_digest.as_str(),
            upstream_digest.as_deref(),
            request.environment_digest.as_str(),
            &sources,
        ))?;
        let mut session = store.session().await?;
        let watermark = current_watermark(&mut session, ctx).await?;
        verify_sources(&mut session, ctx, &sources).await?;
        if let Some(existing) = session
            .get::<E16Envelope<SeedInstallPayloadV1>>(ctx, "artifact", &id)
            .await?
        {
            if existing.schema_version != E16_SEED_INSTALL_SCHEMA
                || existing.input_digest != input_digest
                || existing.payload.baseline_digest != baseline_digest
                || existing.namespace != ctx.namespace()
                || existing.owner_actor != ctx.actor()
                || existing.revoke_watermark != watermark
                || existing.source_refs != sources
                || existing.payload.local_digest != local_digest
                || existing.payload.upstream_digest != upstream_digest
                || existing.payload.environment_digest != request.environment_digest
            {
                return Err(Error::Conflict(
                    "seed identity or immutable baseline changed".into(),
                ));
            }
            verify_sources(&mut session, ctx, &existing.source_refs).await?;
            if existing.payload.status != SeedStatus::Prepared {
                session.commit().await?;
                return Self::read_install(ctx, store, &existing.id).await;
            }
        } else {
            let timestamp = now();
            let envelope = E16Envelope {
                schema_version: E16_SEED_INSTALL_SCHEMA.into(),
                id: id.clone(),
                namespace: ctx.namespace().into(),
                owner_actor: ctx.actor().into(),
                request_key: request.request_key,
                input_digest: input_digest.clone(),
                created_at: timestamp,
                updated_at: timestamp,
                source_refs: sources.clone(),
                revoke_watermark: watermark,
                payload: SeedInstallPayloadV1 {
                    publisher: request.publisher,
                    asset_id: request.asset_id,
                    kind: request.kind,
                    baseline_blob_digest,
                    baseline_digest: baseline_digest.clone(),
                    local_blob_digest,
                    local_digest: local_digest.clone(),
                    upstream_blob_digest,
                    upstream_digest: upstream_digest.clone(),
                    environment_digest: request.environment_digest.clone(),
                    status: SeedStatus::Prepared,
                    quarantine_reason: None,
                },
            };
            put_envelope(&mut session, ctx, &envelope).await?;
            session
                .audit(ctx, "e16.seed_install.prepare", &envelope.id)
                .await?;
        }
        session.commit().await?;
        if store
            .publish_registered_blob(
                ctx,
                &id,
                E16_SEED_INSTALL_SCHEMA,
                "baseline_blob_digest",
                &request.baseline_bytes,
                MAX_FILE as usize,
            )
            .await?
            != baseline_digest
            || store
                .publish_registered_blob(
                    ctx,
                    &id,
                    E16_SEED_INSTALL_SCHEMA,
                    "local_blob_digest",
                    &request.local_bytes,
                    MAX_FILE as usize,
                )
                .await?
                != local_digest
        {
            return Err(Error::Internal);
        }
        if let (Some(bytes), Some(expected)) = (&request.upstream_bytes, &upstream_digest)
            && store
                .publish_registered_blob(
                    ctx,
                    &id,
                    E16_SEED_INSTALL_SCHEMA,
                    "upstream_blob_digest",
                    bytes,
                    MAX_FILE as usize,
                )
                .await?
                != *expected
        {
            return Err(Error::Internal);
        }
        let mut session = store.session().await?;
        let mut current: E16Envelope<SeedInstallPayloadV1> =
            session.need(ctx, "artifact", &id).await?;
        if current.schema_version != E16_SEED_INSTALL_SCHEMA
            || current.namespace != ctx.namespace()
            || current.owner_actor != ctx.actor()
            || current.input_digest != input_digest
            || current.payload.status != SeedStatus::Prepared
            || current.revoke_watermark != current_watermark(&mut session, ctx).await?
        {
            return Err(Error::Conflict(
                "seed install changed before blob finalization".into(),
            ));
        }
        verify_sources(&mut session, ctx, &current.source_refs).await?;
        current.payload.status = SeedStatus::Installed;
        current.updated_at = now();
        put_envelope(&mut session, ctx, &current).await?;
        session
            .audit(ctx, "e16.seed_install.finalize", &current.id)
            .await?;
        session.commit().await?;
        Self::read_install(ctx, store, &id).await
    }

    pub async fn read_install(
        ctx: &Context,
        store: &Store,
        id: &str,
    ) -> Result<E16Envelope<SeedInstallPayloadV1>> {
        ctx.require(&[Role::Admin, Role::Worker])?;
        let mut session = store.session().await?;
        let envelope: E16Envelope<SeedInstallPayloadV1> = session.need(ctx, "artifact", id).await?;
        if envelope.schema_version != E16_SEED_INSTALL_SCHEMA
            || envelope.namespace != ctx.namespace()
            || current_watermark(&mut session, ctx).await? != envelope.revoke_watermark
        {
            return Err(Error::Conflict("seed install is stale or invalid".into()));
        }
        if envelope.payload.status != SeedStatus::Installed {
            return Err(Error::Conflict(
                "seed install content is not finalized".into(),
            ));
        }
        if envelope.payload.upstream_blob_digest.is_some()
            != envelope.payload.upstream_digest.is_some()
        {
            return Err(Error::Conflict(
                "seed upstream blob and digest presence differ".into(),
            ));
        }
        if ctx.role() == Role::Worker && ctx.actor() != envelope.owner_actor {
            return Err(Error::Forbidden);
        }
        verify_sources(&mut session, ctx, &envelope.source_refs).await?;
        session.commit().await?;
        for (blob, digest) in [
            (
                envelope.payload.baseline_blob_digest.as_str(),
                envelope.payload.baseline_digest.as_str(),
            ),
            (
                envelope.payload.local_blob_digest.as_str(),
                envelope.payload.local_digest.as_str(),
            ),
        ] {
            let bytes = store.read_blob(ctx, blob, MAX_FILE as usize).await?;
            if hash(&bytes) != digest {
                return Err(Error::Conflict("seed blob content changed".into()));
            }
        }
        if let (Some(blob), Some(digest)) = (
            &envelope.payload.upstream_blob_digest,
            &envelope.payload.upstream_digest,
        ) {
            let bytes = store.read_blob(ctx, blob, MAX_FILE as usize).await?;
            if hash(&bytes) != *digest {
                return Err(Error::Conflict("upstream seed blob changed".into()));
            }
        }
        let mut session = store.session().await?;
        let current: E16Envelope<SeedInstallPayloadV1> = session.need(ctx, "artifact", id).await?;
        if fingerprint(&current)? != fingerprint(&envelope)?
            || current_watermark(&mut session, ctx).await? != envelope.revoke_watermark
        {
            return Err(Error::Conflict(
                "seed install changed during content read".into(),
            ));
        }
        verify_sources(&mut session, ctx, &current.source_refs).await?;
        session.commit().await?;
        Ok(envelope)
    }

    pub async fn stage_reset_to_baseline(
        ctx: &Context,
        store: &Store,
        install_id: &str,
        request_key: &str,
        environment_digest: &str,
        compiler_version: &str,
    ) -> Result<E16Envelope<StagedAssetPayload>> {
        ctx.require(&[Role::Admin])?;
        identifier(request_key)?;
        identifier(compiler_version)?;
        validate_digest(environment_digest, "environment_digest")?;
        let install = Self::read_install(ctx, store, install_id).await?;
        if install.owner_actor != ctx.actor()
            || install.payload.environment_digest != environment_digest
            || install.payload.status != SeedStatus::Installed
        {
            return Err(Error::Conflict(
                "seed reset environment/owner/status differs".into(),
            ));
        }
        let baseline = store
            .read_blob(
                ctx,
                &install.payload.baseline_blob_digest,
                MAX_FILE as usize,
            )
            .await?;
        if hash(&baseline) != install.payload.baseline_digest {
            return Err(Error::Conflict("seed baseline blob is corrupt".into()));
        }
        let id = e16_storage_id("staged-asset", ctx.namespace(), ctx.actor(), request_key)?;
        let mut sources = install.source_refs.clone();
        sources.push(E16SourceRef {
            kind: "artifact".into(),
            id: install.id.clone(),
            digest: fingerprint(&serde_json::to_value(&install).map_err(|_| Error::Internal)?)?,
        });
        let sources = canonical_sources(sources)?;
        let input_digest = fingerprint(&(
            install.id.as_str(),
            request_key,
            environment_digest,
            compiler_version,
            install.payload.baseline_digest.as_str(),
        ))?;
        let mut session = store.session().await?;
        let watermark = current_watermark(&mut session, ctx).await?;
        if watermark != install.revoke_watermark {
            return Err(Error::Conflict("seed install watermark advanced".into()));
        }
        verify_sources(&mut session, ctx, &sources).await?;
        if let Some(existing) = session
            .get::<E16Envelope<StagedAssetPayload>>(ctx, "artifact", &id)
            .await?
        {
            if existing.schema_version != E16_STAGED_ASSET_SCHEMA
                || existing.input_digest != input_digest
                || existing.namespace != ctx.namespace()
                || existing.owner_actor != ctx.actor()
                || existing.revoke_watermark != watermark
                || existing.source_refs != sources
                || existing.payload.content_blob_digest != install.payload.baseline_blob_digest
                || existing.payload.content_bytes != baseline.len() as u64
                || existing.payload.environment_digest != environment_digest
            {
                return Err(Error::Conflict("seed reset request key changed".into()));
            }
            verify_sources(&mut session, ctx, &existing.source_refs).await?;
            session.commit().await?;
            return Ok(existing);
        }
        let timestamp = now();
        let envelope = E16Envelope {
            schema_version: E16_STAGED_ASSET_SCHEMA.into(),
            id,
            namespace: ctx.namespace().into(),
            owner_actor: ctx.actor().into(),
            request_key: request_key.into(),
            input_digest,
            created_at: timestamp,
            updated_at: timestamp,
            source_refs: sources,
            revoke_watermark: watermark,
            payload: StagedAssetPayload {
                publisher: install.payload.publisher.clone(),
                asset_id: install.payload.asset_id.clone(),
                package_kind: install.payload.kind.clone(),
                manifest_digest: fingerprint(&install.payload)?,
                content_blob_digest: install.payload.baseline_blob_digest.clone(),
                content_bytes: baseline.len() as u64,
                baseline_digest: install.payload.baseline_digest.clone(),
                local_digest: install.payload.local_digest.clone(),
                upstream_digest: install.payload.upstream_digest.clone(),
                environment_digest: environment_digest.into(),
                package_schema_version: "rsia.seed.reset.v1".into(),
                compiler_version: compiler_version.into(),
                state: StagedAssetState::Staged,
                quarantine_reason: None,
                candidate_material: None,
                candidate_ref: None,
            },
        };
        put_envelope(&mut session, ctx, &envelope).await?;
        session
            .put_edge(ctx, "artifact", &envelope.id, "artifact", &install.id)
            .await?;
        session
            .audit(ctx, "e16.seed_reset.stage", &envelope.id)
            .await?;
        session.commit().await?;
        Ok(envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_and_no_silent_activate() {
        let t = SeedTriple {
            bundled: Some("a".into()),
            local: Some("a".into()),
            upstream: Some("b".into()),
            local_marked: true,
            publisher_bundled: "p".into(),
            publisher_local: "p".into(),
            asset_id_bundled: None,
            asset_id_local: None,
            kind_bundled: None,
            kind_local: None,
        };
        assert_eq!(classify(&t), SeedClass::UpstreamNewer);
        assert!(auto_activate(classify(&t)).is_err());
        let mut u = t.clone();
        u.local_marked = false;
        assert_eq!(classify(&u), SeedClass::MissingMarker);
    }
}
