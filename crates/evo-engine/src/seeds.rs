//! Bundled / Local / Upstream seed comparison and protection.
//! Never silently activate.
use crate::capacity::{
    CapacityField, CapacityLimits, V41CapacityUsage, admit_field, unix_now_secs,
};
use crate::packages::{
    E16_STAGED_ASSET_SCHEMA, E16Envelope, E16SourceRef, MAX_FILE, PersistentPackageStore,
    StagedAssetPayload, StagedAssetState, canonical_sources, current_watermark, e16_storage_id,
    put_envelope, validate_digest, verify_sources,
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
    let (Some(b), Some(l), Some(u), Some(id_b), Some(id_l), Some(kind_b), Some(kind_l)) = (
        t.bundled.as_deref(),
        t.local.as_deref(),
        t.upstream.as_deref(),
        t.asset_id_bundled.as_deref(),
        t.asset_id_local.as_deref(),
        t.kind_bundled.as_deref(),
        t.kind_local.as_deref(),
    ) else {
        return SeedClass::Unknown;
    };
    // Shape checks establish comparability, never publisher trust or proof of
    // source bytes. Those remain the installation/catalog consumer's guards.
    if [(b, "baseline"), (l, "local"), (u, "upstream")]
        .into_iter()
        .any(|(value, name)| validate_digest(value, name).is_err())
        || [
            t.publisher_bundled.as_str(),
            t.publisher_local.as_str(),
            id_b,
            id_l,
        ]
        .into_iter()
        .any(|value| identifier(value).is_err())
        || !matches!(kind_b, "improver" | "skill" | "harness")
        || !matches!(kind_l, "improver" | "skill" | "harness")
    {
        return SeedClass::Unknown;
    }
    if !t.local_marked {
        return SeedClass::MissingMarker;
    }
    if t.publisher_bundled != t.publisher_local || id_b != id_l || kind_b != kind_l {
        return SeedClass::SameNameDifferentPublisher;
    }
    match (l, u, b) {
        (l, u, b) if l == u && l == b => SeedClass::Unmodified,
        (l, u, b) if l == u && l != b => SeedClass::IdenticalToUpstream,
        (l, u, b) if l == b && u != b => SeedClass::UpstreamNewer,
        (l, u, b) if l != b && u == b => SeedClass::LocallyEdited,
        (l, u, b) if l != b && u != b => SeedClass::BothChanged,
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
    // Mirror the persistent path (`stage_reset_to_baseline`): only an
    // installed record may be reset; prepared, staged, quarantined or unknown
    // records are refused before any baseline or watermark check.
    if record.status != SeedStatus::Installed {
        return Err(Error::Conflict(
            "reset_rejected: install record is not in the installed state".into(),
        ));
    }

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
        staged_at: now()?,
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
            session_id: format!("stage_{}_{}", asset_id, now()?),
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
pub const E16_CURRENT_SEED_INSTALL_SCHEMA: &str = "rsia.e16.seed_install.v2";
pub const BUILTIN_SEED_CATALOG_BYTES: &[u8] = include_bytes!("../builtin/seed-catalog.v1.json");
pub const BUILTIN_I0_BYTES: &[u8] = include_bytes!("../builtin/improver-i0.v2.json");
const BUILTIN_LICENSE_BYTES: &[u8] = include_bytes!("../../../LICENSE");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedIdentity {
    pub publisher: String,
    pub asset_id: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinSeedLicense {
    pub name: String,
    pub path: String,
    pub sha256: String,
    pub git_blob: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinSeedEntry {
    pub publisher: String,
    pub asset_id: String,
    pub kind: String,
    pub version: u32,
    pub content_sha256: String,
    pub path: String,
    pub git_blob: String,
    pub license: BuiltinSeedLicense,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinSeedCatalog {
    pub schema_version: String,
    pub entries: Vec<BuiltinSeedEntry>,
}

/// Audits bytes; callers cannot install a catalog as runtime authority. Production
/// always resolves the compiled-in catalog and its compiled-in original bytes.
pub fn validate_builtin_seed_catalog(
    catalog_bytes: &[u8],
    content: &[u8],
    license: &[u8],
) -> Result<BuiltinSeedCatalog> {
    let mismatch = || Error::Invalid("seed_catalog_mismatch".into());
    let catalog: BuiltinSeedCatalog =
        serde_json::from_slice(catalog_bytes).map_err(|_| mismatch())?;
    if catalog.schema_version != "rsia.builtin_seed_catalog.v1" || catalog.entries.len() != 1 {
        return Err(mismatch());
    }
    let entry = &catalog.entries[0];
    // The first release has exactly one first-party entry. These reviewed Git
    // identities bind the raw hashes below; no Git process runs in the product.
    if entry.publisher != "rsia.builtin"
        || entry.asset_id != "improver-i0"
        || entry.kind != "improver"
        || entry.version != 2
        || entry.path != "crates/evo-engine/builtin/improver-i0.v2.json"
        || entry.git_blob != "189d2461f1f8983386f774d1e6c8b5d50781ef47"
        || entry.content_sha256
            != "7f79becfa89f0e951c0fafea209956938f7e9b28804ad6255cf30b65fc9d9c2a"
        || hash(content) != entry.content_sha256
        || entry.license.name != "Source-Available"
        || entry.license.path != "LICENSE"
        || entry.license.git_blob != "64db287a4cbdecf401babab8559a1c5a7f22568a"
        || entry.license.sha256
            != "164d42969d38c372e7cd85fe0daf749b6e8409fbd28afd36d1d2b098c5e9debb"
        || hash(license) != entry.license.sha256
    {
        return Err(mismatch());
    }
    Ok(catalog)
}

pub fn builtin_seed_catalog() -> Result<BuiltinSeedCatalog> {
    validate_builtin_seed_catalog(
        BUILTIN_SEED_CATALOG_BYTES,
        BUILTIN_I0_BYTES,
        BUILTIN_LICENSE_BYTES,
    )
}

fn builtin_entry(identity: &SeedIdentity) -> Result<BuiltinSeedEntry> {
    builtin_seed_catalog()?
        .entries
        .into_iter()
        .find(|entry| {
            entry.publisher == identity.publisher
                && entry.asset_id == identity.asset_id
                && entry.kind == identity.kind
        })
        .ok_or_else(|| Error::Invalid("seed_catalog_unregistered".into()))
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedInstallPayloadV2 {
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
    pub local_identity: SeedIdentity,
    pub local_marked: bool,
    pub upstream_path: String,
    pub compiler_version: String,
    pub catalog_version: Option<u32>,
    pub confirmed_content_digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SeedUpstream {
    pub path: String,
    pub content_digest: String,
    pub bytes: Vec<u8>,
}

/// Boundary input: neither B/L identity nor an existing L marker is inferred
/// from the other side. Sources remain checked against the real stored closure.
#[derive(Debug, Clone)]
pub struct InstallCurrentSeedRequest {
    pub request_key: String,
    pub baseline_identity: Option<SeedIdentity>,
    pub local_identity: Option<SeedIdentity>,
    pub local_marked: bool,
    pub baseline_bytes: Option<Vec<u8>>,
    pub local_bytes: Option<Vec<u8>>,
    pub upstream: Option<SeedUpstream>,
    pub environment_digest: String,
    pub source_refs: Vec<E16SourceRef>,
    pub compiler_version: String,
    /// Explicitly confirmed material remains staging, never an approval.
    pub confirmed_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct SeedInstallComparison {
    pub install: E16Envelope<SeedInstallPayloadV2>,
    pub class: SeedClass,
    pub diff: ThreeWayDiff,
    pub staged: Option<E16Envelope<StagedAssetPayload>>,
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

// One storage boundary decode, shared by Current reads, legacy reads and the
// explicit safe-B material path. The common material is never persisted anew.
struct ReadSeedInstall {
    material: E16Envelope<SeedInstallPayloadV1>,
    current: Option<E16Envelope<SeedInstallPayloadV2>>,
    raw: serde_json::Value,
}

pub struct PersistentSeedStore;

impl PersistentSeedStore {
    /// Compatibility bridge for a first managed copy of the registered I0.
    /// It establishes the marker at installation; it cannot attest an existing L.
    pub async fn install(
        ctx: &Context,
        store: &Store,
        request: InstallSeedRequest,
    ) -> Result<E16Envelope<SeedInstallPayloadV2>> {
        let identity = SeedIdentity {
            publisher: request.publisher,
            asset_id: request.asset_id,
            kind: request.kind,
        };
        let upstream = request.upstream_bytes.map(|bytes| SeedUpstream {
            path: "crates/evo-engine/builtin/improver-i0.v2.json".into(),
            content_digest: hash(&bytes),
            bytes,
        });
        Ok(Self::install_inner(
            ctx,
            store,
            InstallCurrentSeedRequest {
                request_key: request.request_key,
                baseline_identity: Some(identity.clone()),
                local_identity: Some(identity),
                local_marked: true,
                baseline_bytes: Some(request.baseline_bytes),
                local_bytes: Some(request.local_bytes),
                upstream,
                environment_digest: request.environment_digest,
                source_refs: request.source_refs,
                compiler_version: "builtin-i0-v2".into(),
                confirmed_bytes: None,
            },
            true,
        )
        .await?
        .install)
    }

    pub async fn install_current(
        ctx: &Context,
        store: &Store,
        request: InstallCurrentSeedRequest,
    ) -> Result<SeedInstallComparison> {
        Self::install_inner(ctx, store, request, false).await
    }

    async fn install_inner(
        ctx: &Context,
        store: &Store,
        request: InstallCurrentSeedRequest,
        initial_builtin: bool,
    ) -> Result<SeedInstallComparison> {
        ctx.require(&[Role::Admin])?;
        identifier(&request.request_key)?;
        identifier(&request.compiler_version)?;
        validate_digest(&request.environment_digest, "environment_digest")?;
        let sources = canonical_sources(request.source_refs.clone())?;
        let mut session = store.session().await?;
        let watermark = current_watermark(&mut session, ctx).await?;
        verify_sources(&mut session, ctx, &sources).await?;
        let unknown = || Error::Invalid("seed_comparison_unknown".into());
        let (Some(b), Some(l), Some(u), Some(identity), Some(local_identity)) = (
            request.baseline_bytes.as_ref(),
            request.local_bytes.as_ref(),
            request.upstream.as_ref(),
            request.baseline_identity.as_ref(),
            request.local_identity.as_ref(),
        ) else {
            return Err(unknown());
        };
        let mut texts = Vec::with_capacity(3);
        for bytes in [b, l, &u.bytes] {
            if bytes.is_empty() || bytes.len() as u64 > MAX_FILE {
                return Err(unknown());
            }
            texts.push(std::str::from_utf8(bytes).map_err(|_| unknown())?);
        }
        if u.path.is_empty()
            || u.path.contains('\\')
            || u.path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || validate_digest(&u.content_digest, "upstream_digest").is_err()
            || hash(&u.bytes) != u.content_digest
        {
            return Err(unknown());
        }
        if initial_builtin {
            let entry = builtin_entry(identity)?;
            if hash(b) != entry.content_sha256 || b != l {
                return Err(Error::Invalid("seed_catalog_mismatch".into()));
            }
        }
        let baseline_digest = hash(b);
        let local_digest = hash(l);
        let upstream_digest = hash(&u.bytes);
        let triple = SeedTriple {
            bundled: Some(baseline_digest.clone()),
            local: Some(local_digest.clone()),
            upstream: Some(upstream_digest.clone()),
            local_marked: request.local_marked,
            publisher_bundled: identity.publisher.clone(),
            publisher_local: local_identity.publisher.clone(),
            asset_id_bundled: Some(identity.asset_id.clone()),
            asset_id_local: Some(local_identity.asset_id.clone()),
            kind_bundled: Some(identity.kind.clone()),
            kind_local: Some(local_identity.kind.clone()),
        };
        let class = classify(&triple);
        match class {
            SeedClass::Unknown | SeedClass::CorruptBaseline => return Err(unknown()),
            SeedClass::MissingMarker => {
                return Err(Error::Conflict("seed_comparison_missing_marker".into()));
            }
            SeedClass::SameNameDifferentPublisher => {
                return Err(Error::Conflict("seed_comparison_identity_differs".into()));
            }
            _ => {}
        }
        let catalog = if identity.publisher == "rsia.builtin" {
            let entry = builtin_entry(identity)?;
            if u.path != entry.path || upstream_digest != entry.content_sha256 {
                return Err(Error::Invalid("seed_catalog_mismatch".into()));
            }
            Some(entry)
        } else {
            None
        };
        let confirmed_content_digest = if let Some(bytes) = &request.confirmed_bytes {
            if class != SeedClass::BothChanged || bytes.is_empty() || bytes.len() as u64 > MAX_FILE
            {
                return Err(Error::Invalid(
                    "seed_confirmation_requires_conflict_material".into(),
                ));
            }
            std::str::from_utf8(bytes)
                .map_err(|_| Error::Invalid("seed_confirmation_not_utf8".into()))?;
            Some(hash(bytes))
        } else {
            None
        };
        let diff = compute_three_way_diff(
            &identity.publisher,
            &identity.asset_id,
            texts[0],
            texts[1],
            texts[2],
        );
        let stage_bytes = if class == SeedClass::UpstreamNewer {
            Some(&u.bytes)
        } else {
            request.confirmed_bytes.as_ref()
        };
        let id = format!(
            "e16-seed-install-{}",
            fingerprint(&(E16_CURRENT_SEED_INSTALL_SCHEMA, ctx.namespace(), identity))?
        );
        let catalog_version = catalog.as_ref().map(|entry| entry.version);
        let input_digest = fingerprint(&(
            E16_CURRENT_SEED_INSTALL_SCHEMA,
            &request.request_key,
            identity,
            local_identity,
            request.local_marked,
            &baseline_digest,
            &local_digest,
            &upstream_digest,
            &u.path,
            &request.environment_digest,
            &sources,
            &request.compiler_version,
            &confirmed_content_digest,
            &catalog_version,
        ))?;
        let existing = session
            .get::<E16Envelope<SeedInstallPayloadV2>>(ctx, "artifact", &id)
            .await?;
        if let Some(existing) = &existing {
            if initial_builtin && existing.request_key != request.request_key {
                return Err(Error::Conflict(
                    "seed_initial_install_already_exists".into(),
                ));
            }
            if existing.schema_version != E16_CURRENT_SEED_INSTALL_SCHEMA
                || existing.namespace != ctx.namespace()
                || existing.owner_actor != ctx.actor()
                || existing.revoke_watermark != watermark
                || existing.payload.baseline_digest != baseline_digest
                || existing.payload.environment_digest != request.environment_digest
                || (existing.request_key == request.request_key
                    && existing.input_digest != input_digest)
                || (existing.payload.status == SeedStatus::Prepared
                    && existing.input_digest != input_digest)
            {
                return Err(Error::Conflict(
                    "seed identity or immutable baseline changed".into(),
                ));
            }
            verify_sources(&mut session, ctx, &existing.source_refs).await?;
        } else if let Some(entry) = &catalog
            && baseline_digest != entry.content_sha256
        {
            return Err(Error::Invalid("seed_catalog_mismatch".into()));
        }
        if let Some(existing) = &existing {
            if existing.payload.status == SeedStatus::Installed {
                session.commit().await?;
                let verified = Self::read_install(ctx, store, &id).await?;
                if stage_bytes.is_none() {
                    return Ok(SeedInstallComparison {
                        install: verified,
                        class,
                        diff,
                        staged: None,
                    });
                }
                session = store.session().await?;
                if current_watermark(&mut session, ctx).await? != watermark {
                    return Err(Error::Conflict("seed install watermark advanced".into()));
                }
            } else if existing.payload.status != SeedStatus::Prepared {
                return Err(Error::Conflict(
                    "seed install content is not finalized".into(),
                ));
            }
        }
        let stage_id = e16_storage_id(
            "staged-asset",
            ctx.namespace(),
            ctx.actor(),
            &request.request_key,
        )?;
        let mut stage_sources = sources.clone();
        if let Some(existing) = &existing {
            for source in &existing.source_refs {
                if !stage_sources.contains(source) {
                    stage_sources.push(source.clone());
                }
            }
        }
        let stage_sources = canonical_sources(stage_sources)?;
        let stage_input = if let Some(bytes) = stage_bytes {
            verify_sources(&mut session, ctx, &stage_sources).await?;
            Some(fingerprint(&(
                "rsia.seed.update.v1",
                &input_digest,
                &id,
                hash(bytes),
                &stage_sources,
            ))?)
        } else {
            None
        };
        let previous_stage = if let Some(expected) = &stage_input {
            let previous = session
                .get::<E16Envelope<StagedAssetPayload>>(ctx, "artifact", &stage_id)
                .await?;
            if let Some(previous) = &previous {
                if previous.schema_version != E16_STAGED_ASSET_SCHEMA
                    || previous.input_digest != *expected
                    || previous.namespace != ctx.namespace()
                    || previous.owner_actor != ctx.actor()
                    || previous.revoke_watermark != watermark
                    || previous.source_refs != stage_sources
                {
                    return Err(Error::Conflict("seed update request key changed".into()));
                }
            } else {
                // The actual instance count and both Prepared registrations are
                // in this one Session, before any installation/blob/audit write.
                let usage: V41CapacityUsage =
                    session.capacity_usage_v41(unix_now_secs()?).await?.into();
                admit_field(
                    CapacityField::StagedPackages,
                    usage.staged_packages,
                    &CapacityLimits::default(),
                )?;
            }
            previous
        } else {
            None
        };
        let needs_publication = existing
            .as_ref()
            .is_none_or(|e| e.payload.status == SeedStatus::Prepared);
        if existing.is_none() {
            let timestamp = now()?;
            let envelope = E16Envelope {
                schema_version: E16_CURRENT_SEED_INSTALL_SCHEMA.into(),
                id: id.clone(),
                namespace: ctx.namespace().into(),
                owner_actor: ctx.actor().into(),
                request_key: request.request_key.clone(),
                input_digest: input_digest.clone(),
                created_at: timestamp,
                updated_at: timestamp,
                source_refs: sources.clone(),
                revoke_watermark: watermark,
                payload: SeedInstallPayloadV2 {
                    publisher: identity.publisher.clone(),
                    asset_id: identity.asset_id.clone(),
                    kind: identity.kind.clone(),
                    baseline_blob_digest: baseline_digest.clone(),
                    baseline_digest: baseline_digest.clone(),
                    local_blob_digest: local_digest.clone(),
                    local_digest: local_digest.clone(),
                    upstream_blob_digest: Some(upstream_digest.clone()),
                    upstream_digest: Some(upstream_digest.clone()),
                    environment_digest: request.environment_digest.clone(),
                    status: SeedStatus::Prepared,
                    quarantine_reason: None,
                    local_identity: local_identity.clone(),
                    local_marked: request.local_marked,
                    upstream_path: u.path.clone(),
                    compiler_version: request.compiler_version.clone(),
                    catalog_version,
                    confirmed_content_digest: confirmed_content_digest.clone(),
                },
            };
            put_envelope(&mut session, ctx, &envelope).await?;
            session.audit(ctx, "e16.seed_install.prepare", &id).await?;
        }
        if let (Some(bytes), Some(stage_input)) = (stage_bytes, &stage_input)
            && previous_stage.is_none()
        {
            let timestamp = now()?;
            let digest = hash(bytes);
            let envelope = E16Envelope {
                schema_version: E16_STAGED_ASSET_SCHEMA.into(),
                id: stage_id.clone(),
                namespace: ctx.namespace().into(),
                owner_actor: ctx.actor().into(),
                request_key: request.request_key.clone(),
                input_digest: stage_input.clone(),
                created_at: timestamp,
                updated_at: timestamp,
                source_refs: stage_sources,
                revoke_watermark: watermark,
                payload: StagedAssetPayload {
                    publisher: identity.publisher.clone(),
                    asset_id: identity.asset_id.clone(),
                    package_kind: identity.kind.clone(),
                    manifest_digest: fingerprint(&(
                        "rsia.seed.update.v1",
                        &u.path,
                        &upstream_digest,
                        &digest,
                    ))?,
                    content_blob_digest: digest,
                    content_bytes: bytes.len() as u64,
                    baseline_digest: baseline_digest.clone(),
                    local_digest: local_digest.clone(),
                    upstream_digest: Some(upstream_digest.clone()),
                    environment_digest: request.environment_digest.clone(),
                    package_schema_version: "rsia.seed.update.v1".into(),
                    compiler_version: request.compiler_version.clone(),
                    state: StagedAssetState::Prepared,
                    quarantine_reason: None,
                    candidate_material: None,
                    candidate_ref: None,
                },
            };
            put_envelope(&mut session, ctx, &envelope).await?;
            session
                .put_edge(ctx, "artifact", &stage_id, "artifact", &id)
                .await?;
            session
                .audit(ctx, "e16.seed_update.prepare", &stage_id)
                .await?;
        }
        session.commit().await?;
        if needs_publication {
            for (field, bytes) in [
                ("baseline_blob_digest", b),
                ("local_blob_digest", l),
                ("upstream_blob_digest", &u.bytes),
            ] {
                store
                    .publish_registered_blob(
                        ctx,
                        &id,
                        E16_CURRENT_SEED_INSTALL_SCHEMA,
                        field,
                        bytes,
                        MAX_FILE as usize,
                    )
                    .await?;
            }
            let mut session = store.session().await?;
            let mut current: E16Envelope<SeedInstallPayloadV2> =
                session.need(ctx, "artifact", &id).await?;
            if current.input_digest != input_digest
                || current.payload.status != SeedStatus::Prepared
                || current.revoke_watermark != current_watermark(&mut session, ctx).await?
            {
                return Err(Error::Conflict(
                    "seed install changed before blob finalization".into(),
                ));
            }
            verify_sources(&mut session, ctx, &current.source_refs).await?;
            current.payload.status = SeedStatus::Installed;
            current.updated_at = now()?;
            put_envelope(&mut session, ctx, &current).await?;
            session.audit(ctx, "e16.seed_install.finalize", &id).await?;
            session.commit().await?;
        }
        let install = Self::read_install(ctx, store, &id).await?;
        let staged = if let (Some(bytes), Some(stage_input)) = (stage_bytes, stage_input) {
            if previous_stage
                .as_ref()
                .is_none_or(|envelope| envelope.payload.state == StagedAssetState::Prepared)
            {
                store
                    .publish_registered_blob(
                        ctx,
                        &stage_id,
                        E16_STAGED_ASSET_SCHEMA,
                        "content_blob_digest",
                        bytes,
                        MAX_FILE as usize,
                    )
                    .await?;
                let mut session = store.session().await?;
                let mut current: E16Envelope<StagedAssetPayload> =
                    session.need(ctx, "artifact", &stage_id).await?;
                if current.input_digest != stage_input
                    || current.payload.state != StagedAssetState::Prepared
                    || current.revoke_watermark != current_watermark(&mut session, ctx).await?
                {
                    return Err(Error::Conflict(
                        "seed update changed before blob finalization".into(),
                    ));
                }
                verify_sources(&mut session, ctx, &current.source_refs).await?;
                current.payload.state = StagedAssetState::Staged;
                current.updated_at = now()?;
                put_envelope(&mut session, ctx, &current).await?;
                session
                    .audit(ctx, "e16.seed_update.finalize", &stage_id)
                    .await?;
                session.commit().await?;
            }
            Some(PersistentPackageStore::read_staged(ctx, store, &stage_id).await?)
        } else {
            None
        };
        Ok(SeedInstallComparison {
            install,
            class,
            diff,
            staged,
        })
    }

    async fn read_material(ctx: &Context, store: &Store, id: &str) -> Result<ReadSeedInstall> {
        ctx.require(&[Role::Admin, Role::Worker])?;
        let mut session = store.session().await?;
        let raw: serde_json::Value = session.need(ctx, "artifact", id).await?;
        let current = match raw["schema_version"].as_str() {
            Some(E16_CURRENT_SEED_INSTALL_SCHEMA) => {
                let current: E16Envelope<SeedInstallPayloadV2> =
                    serde_json::from_value(raw.clone())
                        .map_err(|_| Error::Conflict("seed install is invalid".into()))?;
                let p = &current.payload;
                let identity = SeedIdentity {
                    publisher: p.publisher.clone(),
                    asset_id: p.asset_id.clone(),
                    kind: p.kind.clone(),
                };
                let expected = fingerprint(&(
                    E16_CURRENT_SEED_INSTALL_SCHEMA,
                    &current.request_key,
                    &identity,
                    &p.local_identity,
                    p.local_marked,
                    &p.baseline_digest,
                    &p.local_digest,
                    p.upstream_digest
                        .as_ref()
                        .ok_or_else(|| Error::Conflict("seed install is invalid".into()))?,
                    &p.upstream_path,
                    &p.environment_digest,
                    &current.source_refs,
                    &p.compiler_version,
                    &p.confirmed_content_digest,
                    &p.catalog_version,
                ))?;
                if expected != current.input_digest
                    || current.id
                        != format!(
                            "e16-seed-install-{}",
                            fingerprint(&(
                                E16_CURRENT_SEED_INSTALL_SCHEMA,
                                ctx.namespace(),
                                &identity
                            ))?
                        )
                {
                    return Err(Error::Conflict("seed install is invalid".into()));
                }
                Some(current)
            }
            Some(E16_SEED_INSTALL_SCHEMA) => None,
            _ => return Err(Error::Conflict("seed install is invalid".into())),
        };
        let envelope: E16Envelope<SeedInstallPayloadV1> = if let Some(current) = &current {
            let p = &current.payload;
            E16Envelope {
                schema_version: current.schema_version.clone(),
                id: current.id.clone(),
                namespace: current.namespace.clone(),
                owner_actor: current.owner_actor.clone(),
                request_key: current.request_key.clone(),
                input_digest: current.input_digest.clone(),
                created_at: current.created_at,
                updated_at: current.updated_at,
                source_refs: current.source_refs.clone(),
                revoke_watermark: current.revoke_watermark,
                payload: SeedInstallPayloadV1 {
                    publisher: p.publisher.clone(),
                    asset_id: p.asset_id.clone(),
                    kind: p.kind.clone(),
                    baseline_blob_digest: p.baseline_blob_digest.clone(),
                    baseline_digest: p.baseline_digest.clone(),
                    local_blob_digest: p.local_blob_digest.clone(),
                    local_digest: p.local_digest.clone(),
                    upstream_blob_digest: p.upstream_blob_digest.clone(),
                    upstream_digest: p.upstream_digest.clone(),
                    environment_digest: p.environment_digest.clone(),
                    status: p.status,
                    quarantine_reason: p.quarantine_reason.clone(),
                },
            }
        } else {
            serde_json::from_value(raw.clone())
                .map_err(|_| Error::Conflict("seed install is invalid".into()))?
        };
        if !matches!(
            envelope.schema_version.as_str(),
            E16_SEED_INSTALL_SCHEMA | E16_CURRENT_SEED_INSTALL_SCHEMA
        ) || envelope.namespace != ctx.namespace()
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
        let current_raw: serde_json::Value = session.need(ctx, "artifact", id).await?;
        if fingerprint(&current_raw)? != fingerprint(&raw)?
            || current_watermark(&mut session, ctx).await? != envelope.revoke_watermark
        {
            return Err(Error::Conflict(
                "seed install changed during content read".into(),
            ));
        }
        verify_sources(&mut session, ctx, &envelope.source_refs).await?;
        session.commit().await?;
        Ok(ReadSeedInstall {
            material: envelope,
            current,
            raw,
        })
    }

    pub async fn read_install(
        ctx: &Context,
        store: &Store,
        id: &str,
    ) -> Result<E16Envelope<SeedInstallPayloadV2>> {
        Self::read_material(ctx, store, id)
            .await?
            .current
            .ok_or_else(|| Error::Conflict("seed_install_legacy".into()))
    }

    /// Historical records keep their original version and bytes. This entry
    /// confers no Current comparison, I0, approval, or active-asset authority.
    pub async fn read_legacy_install(
        ctx: &Context,
        store: &Store,
        id: &str,
    ) -> Result<E16Envelope<SeedInstallPayloadV1>> {
        let read = Self::read_material(ctx, store, id).await?;
        if read.current.is_some() {
            return Err(Error::Conflict("seed_install_is_current".into()));
        }
        Ok(read.material)
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
        let ReadSeedInstall {
            material: install,
            raw: install_raw,
            ..
        } = Self::read_material(ctx, store, install_id).await?;
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
        if install.payload.publisher == "rsia.builtin" {
            let entry = builtin_entry(&SeedIdentity {
                publisher: install.payload.publisher.clone(),
                asset_id: install.payload.asset_id.clone(),
                kind: install.payload.kind.clone(),
            })?;
            if hash(&baseline) != entry.content_sha256 {
                return Err(Error::Invalid("seed_catalog_mismatch".into()));
            }
        }
        let id = e16_storage_id("staged-asset", ctx.namespace(), ctx.actor(), request_key)?;
        let mut sources = install.source_refs.clone();
        sources.push(E16SourceRef {
            kind: "artifact".into(),
            id: install.id.clone(),
            digest: fingerprint(&install_raw)?,
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
        let timestamp = now()?;
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
            bundled: Some(hash(b"baseline")),
            local: Some(hash(b"baseline")),
            upstream: Some(hash(b"upstream")),
            local_marked: true,
            publisher_bundled: "p".into(),
            publisher_local: "p".into(),
            asset_id_bundled: Some("seed-identity".into()),
            asset_id_local: Some("seed-identity".into()),
            kind_bundled: Some("skill".into()),
            kind_local: Some("skill".into()),
        };
        assert_eq!(classify(&t), SeedClass::UpstreamNewer);
        assert!(auto_activate(classify(&t)).is_err());
        let mut u = t.clone();
        u.local_marked = false;
        assert_eq!(classify(&u), SeedClass::MissingMarker);
    }
}
