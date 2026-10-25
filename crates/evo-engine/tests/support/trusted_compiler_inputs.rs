//! Explicit Admin empty-root setup for software fixtures. These configurations
//! never issue ordinary P, provider captures, isolated grades or promotion.
#![allow(dead_code)]
use async_trait::async_trait;
use evo_core::contract::ResolvedBundleV3;
use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, PatchOp, Profile, ResolvedBundle, SkillSnapshot,
    SystemSnapshot, compile_bundle, rebase_skill_patch,
};
use evo_core::evidence::{EvidenceSet, Purpose, SourceSelection};
use evo_core::optimization::{OptimizationTrace, TrustedSourceBinding};
use evo_core::skill_edit::TrustedEditContext;
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::formal_authority::{
    CompilerInputRequest, EmptyRootAuthorization, FormalRegistry, FrozenCompilerProfile,
    HOST_SCHEMA, PROFILE_SCHEMA, PROVIDER_SCHEMA, ROOT_SCHEMA, RegisteredExternalProvider,
    RegisteredFormalHost, VerifiedCompilerInputs, empty_root_strategy, resolve_compiler_inputs,
};
use evo_engine::formal_authority::{
    CurrentVerifiedCompilerInputs, resolve_current_bundle_in_session,
    resolve_current_compiler_inputs,
};
use evo_engine::model::CurrentCompilerInputUse;
use evo_engine::optimization::{
    OptimizationJournal, OptimizationStepRequest, StageFact,
    StoreOptimizationJournal as RealJournal,
};
use evo_storage::Store;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone)]
pub struct FixtureInputs {
    pub host: RegisteredFormalHost,
    pub provider: RegisteredExternalProvider,
    pub frozen: FrozenCompilerProfile,
    pub root: EmptyRootAuthorization,
    pub profile: Profile,
}
static FIXTURES: OnceLock<Mutex<BTreeMap<String, FixtureInputs>>> = OnceLock::new();
fn fixtures() -> &'static Mutex<BTreeMap<String, FixtureInputs>> {
    FIXTURES.get_or_init(Mutex::default)
}

pub fn inputs(
    namespace: &str,
    profile_id: &str,
    baseline: SkillSnapshot,
    baseline_strategy: Strategy,
    capabilities: HostCapabilities,
    environment: &str,
) -> FixtureInputs {
    let scope = fingerprint(&(
        namespace,
        profile_id,
        &baseline,
        &baseline_strategy,
        &capabilities,
        environment,
    ))
    .unwrap();
    let snapshot = SystemSnapshot {
        schema_version: "rsia.system_snapshot.v1".into(),
        profile_id: profile_id.into(),
        host_id: format!("fixture-host-{}", &scope[..16]),
        host_version: "fixture-v1".into(),
        model_id: "fixture-model".into(),
        tools: vec![],
        mandatory_context_digest: hash(b"fixture-mandatory-context"),
    };
    let host = RegisteredFormalHost {
        id: format!("fixture-host-registration-{scope}"),
        schema_version: HOST_SCHEMA.into(),
        namespace: namespace.into(),
        host_id: snapshot.host_id.clone(),
        host_version: snapshot.host_version.clone(),
        code_digest: hash(b"fixture-program-code"),
        grader_code_digest: hash(b"fixture-grader-code"),
        host_uid: 10001,
        grader_uid: 10002,
        administrator_uid: 10003,
        host_workspace: "/unavailable/fixture-host".into(),
        grader_workspace: "/unavailable/fixture-grader".into(),
        grader_credential_environment_key: "RSIA_FIXTURE_UNAVAILABLE_GRADER_KEY".into(),
        host_principal: hash(b"fixture-host-principal"),
        grader_principal: hash(b"fixture-grader-principal"),
        administrator_principal: hash(b"fixture-admin-principal"),
        worker_principal: hash(b"fixture-worker-principal"),
        credential_binding_digest: hash(b"fixture-config-only"),
        isolated_workspace_digest: hash(b"fixture-host-workspace"),
        grader_workspace_digest: hash(b"fixture-grader-workspace"),
        snapshot,
        capabilities: capabilities.clone(),
        host_surface_digest: hash(b"fixture-surface"),
        adapter_digest: hash(b"fixture-adapter"),
        authorized_by: "fixture-admin".into(),
    };
    let provider = RegisteredExternalProvider {
        id: format!("fixture-provider-{scope}"),
        schema_version: PROVIDER_SCHEMA.into(),
        namespace: namespace.into(),
        host_registration_id: host.id.clone(),
        endpoint: "https://fixture.invalid/v1/chat/completions".into(),
        provider_model: "fixture-model".into(),
        model_digest: hash(b"fixture-model"),
        transport_code_digest: host.code_digest.clone(),
        credential_environment_key: "RSIA_FIXTURE_UNAVAILABLE_PROVIDER_KEY".into(),
        credential_principal: host.host_principal.clone(),
        currency: "USD".into(),
        pricing_version: "fixture-pricing".into(),
        input_micros_per_million_tokens: 1,
        output_micros_per_million_tokens: 1,
        max_output_tokens: 256,
        authorized_by: "fixture-admin".into(),
    };
    let frozen = FrozenCompilerProfile {
        id: format!("fixture-profile-{scope}"),
        schema_version: PROFILE_SCHEMA.into(),
        namespace: namespace.into(),
        profile_id: profile_id.into(),
        version: 1,
        host_registration_id: host.id.clone(),
        provider_registration_id: provider.id.clone(),
        resource_instrumentation_id: None,
        system_snapshot: host.snapshot.clone(),
        capabilities,
        environment_digest: environment.into(),
        baseline_skill: baseline,
        baseline_strategy,
        dependencies: vec![],
        purpose: Purpose::Development,
        application_order: vec!["mandatory_context".into(), "skill".into(), "task".into()],
        authorized_by: "fixture-admin".into(),
    };
    let root = EmptyRootAuthorization {
        id: format!("fixture-root-{scope}"),
        schema_version: ROOT_SCHEMA.into(),
        namespace: namespace.into(),
        profile_registration_id: frozen.id.clone(),
        profile_digest: fingerprint(&frozen).unwrap(),
        host_registration_id: host.id.clone(),
        skill: SkillSnapshot::empty(),
        strategy: empty_root_strategy(),
        authorized_by: "fixture-admin".into(),
    };
    let profile = Profile {
        id: profile_id.into(),
        evolution_enabled: true,
        parent_digest: root.digest().unwrap(),
        baseline_digest: frozen.baseline_digest().unwrap(),
    };
    let fixture = FixtureInputs {
        host,
        provider,
        frozen,
        root,
        profile,
    };
    fixtures()
        .lock()
        .unwrap()
        .insert(fixture.profile.parent_digest.clone(), fixture.clone());
    fixture
}

pub async fn install(store: &Store, fixture: &FixtureInputs) -> Result<()> {
    let admin = Context::new(&fixture.frozen.namespace, "fixture-admin", Role::Admin)?;
    let mut s = store.session().await?;
    let exists = s
        .get::<serde_json::Value>(
            &admin,
            "artifact",
            &format!("formal-profile-{}", fixture.profile.id),
        )
        .await?
        .is_some();
    s.commit().await?;
    if exists {
        return Ok(());
    }
    FormalRegistry::register_host(&admin, store, fixture.host.clone()).await?;
    FormalRegistry::register_provider(&admin, store, fixture.provider.clone()).await?;
    FormalRegistry::freeze_profile(&admin, store, fixture.frozen.clone()).await?;
    FormalRegistry::authorize_empty_root(&admin, store, fixture.root.clone()).await?;
    Ok(())
}
pub async fn install_request(store: &Store, request: &CompilerInputRequest) -> Result<()> {
    let fixture = fixtures()
        .lock()
        .map_err(|_| Error::Internal)?
        .get(&request.profile.parent_digest)
        .cloned()
        .ok_or(Error::Forbidden)?;
    install(store, &fixture).await
}
pub async fn resolve_software_fixture(
    request: &CompilerInputRequest,
) -> Result<VerifiedCompilerInputs> {
    let fixture = fixtures()
        .lock()
        .map_err(|_| Error::Internal)?
        .get(&request.profile.parent_digest)
        .cloned()
        .ok_or(Error::Forbidden)?;
    let dir = tempfile::tempdir().map_err(|_| Error::Internal)?;
    let store = Store::open(&dir.path().join("compiler-fixture.sqlite3")).await?;
    install(&store, &fixture).await?;
    let worker = Context::new(&fixture.frozen.namespace, "fixture-worker", Role::Worker)?;
    let mut s = store.session().await?;
    s.bump_watermark(&worker, &hash(b"fixture-initial-watermark"))
        .await?;
    let result = resolve_compiler_inputs(&mut s, &worker, request).await?;
    s.commit().await?;
    store.close().await;
    Ok(result)
}

/// Adapts old search-parent fixture data while preserving its entire Skill and
/// generation Strategy. Only the control P/B identity becomes an explicit root.
pub fn request(mut r: OptimizationStepRequest<'_>) -> OptimizationStepRequest<'_> {
    let fixture = inputs(
        &r.model_context.namespace,
        &r.bundle_context.profile.id,
        r.bundle_context.baseline.clone(),
        r.bundle_context.baseline_strategy.clone(),
        r.bundle_context.caps.clone(),
        &r.development_request.environment_digest,
    );
    let profile = Box::leak(Box::new(fixture.profile));
    let edit = TrustedEditContext::new(
        &r.edit_batch_template.namespace,
        &r.edit_batch_template.profile_id,
        &r.edit_batch_template.skill_id,
        &r.edit_batch_template.skill_version,
        &profile.parent_digest,
        &profile.baseline_digest,
        r.parent_skill,
        r.model_context.source_closure.clone(),
    )
    .unwrap();
    r.edit_context = Box::leak(Box::new(edit));
    r.bundle_context.profile = profile;
    r.edit_batch_template.approved_parent_digest = profile.parent_digest.clone();
    r.edit_batch_template.safe_baseline_digest = profile.baseline_digest.clone();
    r
}

/// A complete pure value compiled relative to the explicit root, with every
/// original search-parent leaf retained. The fixture is registered separately.
pub fn bundle(
    parts: CompileParts<'_>,
    namespace: &str,
    environment: &str,
) -> Result<ResolvedBundle> {
    let baseline = parts.baseline.clone();
    let baseline_strategy = parts.baseline_strategy.clone();
    let caps = parts.caps.clone();
    let old = compile_bundle(parts)?;
    let fixture = inputs(
        namespace,
        &old.profile_id,
        baseline,
        baseline_strategy,
        caps,
        environment,
    );
    compile_bundle(CompileParts {
        profile: &fixture.profile,
        parent: &fixture.root.skill,
        baseline: &fixture.frozen.baseline_skill,
        parent_strategy: &fixture.root.strategy,
        baseline_strategy: &fixture.frozen.baseline_strategy,
        skill_patch: &rebase_skill_patch(&fixture.root.skill, &old.skill)?,
        improver_patch: &ImproverPatch {
            instruction: PatchOp::Set {
                value: old.improver.instruction,
            },
            max_candidates: PatchOp::Set {
                value: old.improver.max_candidates,
            },
            max_rounds: PatchOp::Set {
                value: old.improver.max_rounds,
            },
        },
        caps: &fixture.frozen.capabilities,
        revoked: &Default::default(),
    })
}

#[derive(Clone)]
pub struct StoreOptimizationJournal {
    inner: RealJournal,
    store: Store,
    context: Context,
}
impl StoreOptimizationJournal {
    pub fn new(store: Store, context: Context, owner: impl Into<String>) -> Result<Self> {
        Ok(Self {
            inner: RealJournal::new(store.clone(), context.clone(), owner)?,
            store,
            context,
        })
    }
    pub async fn reload(&self, id: &str) -> Result<StageFact> {
        self.inner.reload(id).await
    }
}
#[async_trait]
impl OptimizationJournal for StoreOptimizationJournal {
    async fn current_compiler_inputs(
        &self,
        request: &CompilerInputRequest,
    ) -> Result<CurrentVerifiedCompilerInputs> {
        resolve_software_fixture_current(&self.store, &self.context, request).await
    }
    async fn claim_source_bound(
        &self,
        fact: StageFact,
        inputs: &CurrentCompilerInputUse,
    ) -> Result<bool> {
        self.inner.claim_source_bound(fact, inputs).await
    }
    async fn lookup_source_bound(
        &self,
        id: &str,
        inputs: &CurrentCompilerInputUse,
    ) -> Result<Option<StageFact>> {
        self.inner.lookup_source_bound(id, inputs).await
    }
    async fn commit_source_bound(
        &self,
        fact: StageFact,
        inputs: &CurrentCompilerInputUse,
    ) -> Result<()> {
        self.inner.commit_source_bound(fact, inputs).await
    }
    async fn compiler_inputs(
        &self,
        request: &CompilerInputRequest,
    ) -> Result<VerifiedCompilerInputs> {
        install_request(&self.store, request).await?;
        let mut s = self.store.session().await?;
        let verified = resolve_compiler_inputs(&mut s, &self.context, request).await?;
        s.commit().await?;
        Ok(verified)
    }
    async fn claim_bound(
        &self,
        fact: StageFact,
        inputs: &evo_engine::model::CompilerInputUse,
    ) -> Result<bool> {
        self.inner.claim_bound(fact, inputs).await
    }
    async fn lookup_bound(
        &self,
        id: &str,
        inputs: &evo_engine::model::CompilerInputUse,
    ) -> Result<Option<StageFact>> {
        self.inner.lookup_bound(id, inputs).await
    }
    async fn commit_bound(
        &self,
        fact: StageFact,
        inputs: &evo_engine::model::CompilerInputUse,
    ) -> Result<()> {
        self.inner.commit_bound(fact, inputs).await
    }
    async fn commit(&self, fact: StageFact) -> Result<()> {
        self.inner.commit(fact).await
    }
    async fn lookup(&self, id: &str) -> Result<Option<StageFact>> {
        self.inner.lookup(id).await
    }
    async fn claim(&self, fact: StageFact) -> Result<bool> {
        self.inner.claim(fact).await
    }
    async fn verify_sources(
        &self,
        selection: &SourceSelection,
        evidence: &EvidenceSet,
        bindings: &[TrustedSourceBinding],
        traces: &[OptimizationTrace],
        watermark: u64,
    ) -> Result<()> {
        self.inner
            .verify_sources(selection, evidence, bindings, traces, watermark)
            .await
    }
    async fn check_live(&self, ids: &[String], watermark: u64) -> Result<()> {
        self.inner.check_live(ids, watermark).await
    }
}

pub fn software_candidate(namespace: &str, environment: &str, content: &str) -> ResolvedBundle {
    let f = inputs(
        namespace,
        "development-fixture",
        SkillSnapshot::empty(),
        Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        environment,
    );
    compile_bundle(CompileParts {
        profile: &f.profile,
        parent: &f.root.skill,
        baseline: &f.frozen.baseline_skill,
        parent_strategy: &f.root.strategy,
        baseline_strategy: &f.frozen.baseline_strategy,
        skill_patch: &evo_core::contract::SkillPatch {
            content: PatchOp::Set {
                value: content.into(),
            },
            ..Default::default()
        },
        improver_patch: &ImproverPatch::default(),
        caps: &f.frozen.capabilities,
        revoked: &Default::default(),
    })
    .unwrap()
}
pub async fn install_bundle(store: &Store, bundle: &ResolvedBundle) -> Result<()> {
    let f = fixtures()
        .lock()
        .map_err(|_| Error::Internal)?
        .get(&bundle.parent_digest)
        .cloned()
        .ok_or(Error::Forbidden)?;
    install(store, &f).await
}
pub async fn persist_bundle(
    store: &Store,
    namespace: &str,
    bundle: &ResolvedBundle,
    environment: &str,
    watermark: u64,
) -> Result<()> {
    install_bundle(store, bundle).await?;
    let ctx = Context::new(namespace, "fixture-worker", Role::Worker)?;
    let mut s = store.session().await?;
    let v = evo_engine::formal_authority::resolve_bundle_in_session(
        &mut s,
        &ctx,
        bundle,
        environment,
        watermark,
        Purpose::Development,
    )
    .await?;
    evo_engine::compiler::persist_authenticated_bundle(
        &mut s,
        &ctx,
        &v.request_for_search_parent(bundle.skill.clone()),
        bundle,
    )
    .await?;
    s.commit().await
}

/// Reads the already authenticated candidate's exact output binding for a
/// manual healthy development request/observation pair.
pub async fn bundle_use(
    store: &Store,
    namespace: &str,
    bundle_digest: &str,
) -> Result<evo_engine::model::CompilerInputUse> {
    let worker = Context::new(namespace, "fixture-worker", Role::Worker)?;
    let mut s = store.session().await?;
    let record: evo_engine::compiler::AuthenticatedBundleRecord = s
        .need(
            &worker,
            "artifact",
            &format!("formal-bundle-{bundle_digest}"),
        )
        .await?;
    let inputs = evo_engine::model::CompilerInputUse::new(record.input, record.binding);
    inputs.verify_current(&mut s, &worker).await?;
    s.commit().await?;
    Ok(inputs)
}

// Current siblings operate on the supplied Store and its actual namespace and
// watermark. The old helpers above remain byte-for-byte historical fixtures.
// These EmptyRoot/ProgramFixture registrations cannot execute a provider, mint
// independent Formal evidence or establish an OS/paid-source capability.
pub async fn resolve_software_fixture_current(
    store: &Store,
    ctx: &Context,
    request: &CompilerInputRequest,
) -> Result<CurrentVerifiedCompilerInputs> {
    install_request(store, request).await?;
    let mut session = store.session().await?;
    let verified = Box::pin(resolve_current_compiler_inputs(&mut session, ctx, request)).await?;
    session.commit().await?;
    Ok(verified)
}

async fn compile_current_fixture(
    store: &Store,
    ctx: &Context,
    fixture: &FixtureInputs,
    desired: &SkillSnapshot,
    desired_strategy: &Strategy,
    environment: &str,
    watermark: u64,
) -> Result<ResolvedBundleV3> {
    if ctx.namespace() != fixture.frozen.namespace
        || environment != fixture.frozen.environment_digest
    {
        return Err(Error::Forbidden);
    }
    install(store, fixture).await?;
    let request = CompilerInputRequest {
        profile: fixture.profile.clone(),
        search_parent: fixture.root.skill.clone(),
        baseline: fixture.frozen.baseline_skill.clone(),
        search_parent_strategy: fixture.root.strategy.clone(),
        expected_parent_strategy_digest: Some(fingerprint(&fixture.root.strategy)?),
        baseline_strategy: fixture.frozen.baseline_strategy.clone(),
        capabilities: fixture.frozen.capabilities.clone(),
        environment_digest: environment.into(),
        purpose: Purpose::Development,
        revoke_watermark: watermark,
    };
    let skill_patch = rebase_skill_patch(&fixture.root.skill, desired)?;
    let improver_patch = ImproverPatch {
        instruction: PatchOp::Set {
            value: desired_strategy.instruction.clone(),
        },
        max_candidates: PatchOp::Set {
            value: desired_strategy.max_candidates,
        },
        max_rounds: PatchOp::Set {
            value: desired_strategy.max_rounds,
        },
    };
    let mut session = store.session().await?;
    let bundle = Box::pin(evo_engine::compiler::compile_current_in_session(
        &mut session,
        ctx,
        &request,
        &skill_patch,
        &improver_patch,
    ))
    .await?;
    // Refuse an unrepresentable strategy instead of silently normalizing it.
    // The actual P/B, origin kernel and permission checks are production code.
    if fingerprint(&bundle.skill)? != fingerprint(desired)?
        || fingerprint(&bundle.improver)? != fingerprint(desired_strategy)?
    {
        return Err(Error::Conflict(
            "Current fixture changed requested Skill or strategy".into(),
        ));
    }
    let stored = Box::pin(evo_engine::compiler::verify_current_bundle_in_session(
        &mut session,
        ctx,
        &bundle.digest,
        environment,
        watermark,
    ))
    .await?;
    if serde_json::to_vec(&stored.bundle).map_err(|_| Error::Internal)?
        != serde_json::to_vec(&bundle).map_err(|_| Error::Internal)?
        || fingerprint(&stored.original_baseline)? != fingerprint(&fixture.frozen)?
    {
        return Err(Error::Conflict(
            "Current fixture original material differs".into(),
        ));
    }
    session.commit().await?;
    Ok(bundle)
}

pub async fn software_candidate_current(
    store: &Store,
    ctx: &Context,
    environment: &str,
    content: &str,
    watermark: u64,
) -> Result<ResolvedBundleV3> {
    let fixture = inputs(
        ctx.namespace(),
        "development-fixture",
        SkillSnapshot::empty(),
        Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        environment,
    );
    let desired = SkillSnapshot {
        content: content.into(),
        ..SkillSnapshot::empty()
    };
    Box::pin(compile_current_fixture(
        store,
        ctx,
        &fixture,
        &desired,
        &fixture.root.strategy,
        environment,
        watermark,
    ))
    .await
}

pub async fn bundle_current<'a>(
    store: &'a Store,
    ctx: &'a Context,
    parts: CompileParts<'a>,
    environment: &'a str,
    watermark: u64,
) -> Result<ResolvedBundleV3> {
    let baseline = parts.baseline.clone();
    let baseline_strategy = parts.baseline_strategy.clone();
    let capabilities = parts.caps.clone();
    // Preserve the old pure patch mathematics and all original input guards.
    // Only the resulting Skill/strategy are requested data. No old digest,
    // origins, stored record or compiler binding becomes Current authority.
    let desired = compile_bundle(parts)?;
    let fixture = inputs(
        ctx.namespace(),
        &desired.profile_id,
        baseline,
        baseline_strategy,
        capabilities,
        environment,
    );
    Box::pin(compile_current_fixture(
        store,
        ctx,
        &fixture,
        &desired.skill,
        &desired.improver,
        environment,
        watermark,
    ))
    .await
}

pub async fn persist_bundle_current(
    store: &Store,
    ctx: &Context,
    bundle: &ResolvedBundleV3,
    environment: &str,
    watermark: u64,
) -> Result<()> {
    // This diagnostic helper supports only one of its complete explicit roots.
    // A caller cannot turn an arbitrary P digest into an approved parent.
    let fixture = fixtures()
        .lock()
        .map_err(|_| Error::Internal)?
        .get(&bundle.parent_digest)
        .cloned()
        .ok_or(Error::Forbidden)?;
    if ctx.namespace() != fixture.frozen.namespace
        || environment != fixture.frozen.environment_digest
    {
        return Err(Error::Forbidden);
    }
    install(store, &fixture).await?;
    let mut session = store.session().await?;
    let verified = Box::pin(resolve_current_bundle_in_session(
        &mut session,
        ctx,
        bundle,
        environment,
        watermark,
        Purpose::Development,
    ))
    .await?;
    let request = verified.request_for_search_parent(bundle.skill.clone());
    Box::pin(evo_engine::compiler::persist_current_authenticated_bundle(
        &mut session,
        ctx,
        &request,
        bundle,
    ))
    .await?;
    let stored = Box::pin(evo_engine::compiler::verify_current_bundle_in_session(
        &mut session,
        ctx,
        &bundle.digest,
        environment,
        watermark,
    ))
    .await?;
    if serde_json::to_vec(&stored.bundle).map_err(|_| Error::Internal)?
        != serde_json::to_vec(bundle).map_err(|_| Error::Internal)?
    {
        return Err(Error::Conflict(
            "Current fixture persisted wire differs".into(),
        ));
    }
    session.commit().await
}

pub async fn bundle_use_current(
    store: &Store,
    ctx: &Context,
    bundle_digest: &str,
    environment: &str,
    watermark: u64,
) -> Result<CurrentCompilerInputUse> {
    let mut session = store.session().await?;
    let record = Box::pin(evo_engine::compiler::verify_current_bundle_in_session(
        &mut session,
        ctx,
        bundle_digest,
        environment,
        watermark,
    ))
    .await?;
    let inputs = CurrentCompilerInputUse::new(record.input, record.binding);
    Box::pin(inputs.verify_current(&mut session, ctx)).await?;
    session.commit().await?;
    Ok(inputs)
}

/// Explicit software reviewed source for existing fixture declarations. The raw
/// UTF-8 contains every name and typed shape, and the real source-bound writer
/// extracts them; this is neither a historical v1 record nor native Host proof.
pub async fn register_fixture_host_surface(
    ctx: &Context,
    store: &Store,
    id: &str,
    declaration: evo_core::contract::HostSurfaceManifest,
    extracted: Vec<String>,
) -> Result<evo_engine::release_store::HostSurfaceRecordV2> {
    use evo_core::contract::{
        HostSurfaceClassificationV2, HostSurfaceRegistrationV2, ReviewedHostSourceEnvelopeV1,
        SurfaceCoverage,
    };
    declaration.validate_against_extraction(&extracted)?;
    let declarations: Vec<_> = declaration
        .items
        .iter()
        .map(|item| {
            serde_json::json!({
                "name":item.name,"value_type":"string","enum_values":[],"default":{"kind":"none"}
            })
        })
        .collect();
    let original_utf8 = serde_json::to_string(&serde_json::json!({
        "schema_version":"rsia.host_reviewed_declarations.v1", "host":declaration.host,
        "host_version":declaration.host_version,"declarations":declarations,
    }))
    .map_err(|_| Error::Internal)?;
    let source = ReviewedHostSourceEnvelopeV1 {
        schema_version: "rsia.host_source_envelope.v1".into(),
        source_kind: "reviewed_fixture".into(),
        source_schema: "rsia.host_reviewed_declarations.v1".into(),
        extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8,
    };
    let classifications = declaration
        .items
        .into_iter()
        .map(|item| {
            let supported = item.coverage == SurfaceCoverage::Supported;
            let consumer = if supported {
                Some(
                    evo_core::contract::field_contract(
                        item.mapped_field.as_deref().ok_or(Error::Forbidden)?,
                    )?
                    .consumer
                    .into(),
                )
            } else {
                None
            };
            Ok(HostSurfaceClassificationV2 {
                name: item.name,
                coverage: item.coverage,
                mapped_field: item.mapped_field,
                consumer,
                reason: item.reason,
                renamed_from: None,
                test_ref: supported.then(|| "existing software fixture assertions".into()),
                capability: supported.then(|| "fixture-capability".into()),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let raw = serde_json::to_string(&source).map_err(|_| Error::Internal)?;
    evo_engine::hosts::register_reviewed_host_surface_json(
        ctx,
        store,
        id,
        &raw,
        HostSurfaceRegistrationV2 {
            adapter_version: declaration.adapter_version,
            classifications,
            retired: vec![],
        },
        None,
    )
    .await
}

/// Pure complete Current content for planning fixture pins. The result gains
/// authority only after the separate real Admin registrations and in-Session
/// resolver/persistence path have succeeded in the supplied Store.
pub fn bundle_content_current(
    parts: CompileParts<'_>,
    namespace: &str,
    environment: &str,
) -> Result<ResolvedBundleV3> {
    use evo_core::contract::{
        CompilePartsV3, ParentContentV3, ProfileContentProofV3, compile_bundle_v3,
    };
    let baseline = parts.baseline.clone();
    let strategy = parts.baseline_strategy.clone();
    let capabilities = parts.caps.clone();
    let desired = compile_bundle(parts)?;
    let fixture = inputs(
        namespace,
        &desired.profile_id,
        baseline,
        strategy,
        capabilities,
        environment,
    );
    let parent = evo_core::contract::EmptyRootContentV1 {
        id: fixture.root.id.clone(),
        schema_version: fixture.root.schema_version.clone(),
        namespace: fixture.root.namespace.clone(),
        profile_registration_id: fixture.root.profile_registration_id.clone(),
        profile_digest: fixture.root.profile_digest.clone(),
        host_registration_id: fixture.root.host_registration_id.clone(),
        skill: fixture.root.skill.clone(),
        strategy: fixture.root.strategy.clone(),
        authorized_by: fixture.root.authorized_by.clone(),
    };
    let baseline = evo_core::contract::FrozenCompilerProfileContentV1 {
        id: fixture.frozen.id.clone(),
        schema_version: fixture.frozen.schema_version.clone(),
        namespace: fixture.frozen.namespace.clone(),
        profile_id: fixture.frozen.profile_id.clone(),
        version: fixture.frozen.version,
        host_registration_id: fixture.frozen.host_registration_id.clone(),
        provider_registration_id: fixture.frozen.provider_registration_id.clone(),
        resource_instrumentation_id: fixture.frozen.resource_instrumentation_id.clone(),
        environment_digest: fixture.frozen.environment_digest.clone(),
        system_snapshot: fixture.frozen.system_snapshot.clone(),
        capabilities: fixture.frozen.capabilities.clone(),
        baseline_skill: fixture.frozen.baseline_skill.clone(),
        baseline_strategy: fixture.frozen.baseline_strategy.clone(),
        dependencies: fixture
            .frozen
            .dependencies
            .iter()
            .map(|d| evo_core::contract::CompilerDependencyContentV1 {
                kind: d.kind.clone(),
                id: d.id.clone(),
                schema_version: d.schema_version.clone(),
                content_digest: d.content_digest.clone(),
            })
            .collect(),
        purpose: fixture.frozen.purpose,
        application_order: fixture.frozen.application_order.clone(),
        authorized_by: fixture.frozen.authorized_by.clone(),
    };
    let proof = ProfileContentProofV3::from_content(
        fixture.profile.clone(),
        ParentContentV3::EmptyRoot(parent),
        baseline,
    )?;
    let patch = rebase_skill_patch(&fixture.root.skill, &desired.skill)?;
    let improver = ImproverPatch {
        instruction: PatchOp::Set {
            value: desired.improver.instruction.clone(),
        },
        max_candidates: PatchOp::Set {
            value: desired.improver.max_candidates,
        },
        max_rounds: PatchOp::Set {
            value: desired.improver.max_rounds,
        },
    };
    let bundle = compile_bundle_v3(CompilePartsV3 {
        content: &proof,
        skill_patch: &patch,
        improver_patch: &improver,
        revoked: &Default::default(),
    })?;
    if fingerprint(&bundle.skill)? != fingerprint(&desired.skill)?
        || fingerprint(&bundle.improver)? != fingerprint(&desired.improver)?
    {
        return Err(Error::Conflict(
            "Current content fixture changed requested effective content".into(),
        ));
    }
    Ok(bundle)
}

pub async fn install_bundle_current_sources(
    store: &Store,
    bundle: &ResolvedBundleV3,
) -> Result<()> {
    let fixture = fixtures()
        .lock()
        .map_err(|_| Error::Internal)?
        .get(&bundle.parent_digest)
        .cloned()
        .ok_or(Error::Forbidden)?;
    install(store, &fixture).await
}

pub fn software_candidate_content_current(
    namespace: &str,
    environment: &str,
    content: &str,
) -> ResolvedBundleV3 {
    let old = software_candidate(namespace, environment, content);
    let profile = Profile {
        id: old.profile_id.clone(),
        evolution_enabled: true,
        parent_digest: old.parent_digest.clone(),
        baseline_digest: old.baseline_digest.clone(),
    };
    bundle_content_current(
        CompileParts {
            profile: &profile,
            parent: &old.skill,
            baseline: &SkillSnapshot::empty(),
            parent_strategy: &old.improver,
            baseline_strategy: &Strategy::default(),
            skill_patch: &Default::default(),
            improver_patch: &Default::default(),
            caps: &HostCapabilities {
                available: Default::default(),
                granted: Default::default(),
            },
            revoked: &Default::default(),
        },
        namespace,
        environment,
    )
    .expect("complete Current content fixture")
}

// Lossless parent issuer application facts, with exact original body UTF-8.
// Migration ledger and disposable FTS internals come from Store initialization.
// These are synthetic old V2 fixtures, never authority for new dispatch.
const LEGACY_ISSUER_CONSTANTS: &str = include_str!("../fixtures/ag135b_legacy_issuers.json");
pub async fn restore_frozen_issuer_rows(
    path: &std::path::Path,
    key: &str,
) -> Result<serde_json::Value> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let all: serde_json::Value =
        serde_json::from_str(LEGACY_ISSUER_CONSTANTS).map_err(|_| Error::Internal)?;
    let frozen = all["fixtures"].get(key).ok_or(Error::NotFound)?.clone();
    let script = r#"
import json,sqlite3,sys
snapshot=json.load(sys.stdin)
con=sqlite3.connect(sys.argv[1])
con.execute("PRAGMA foreign_keys=OFF")
con.execute("BEGIN IMMEDIATE")
quote=lambda name:'"'+name.replace('"','""')+'"'
def cell(value):
    if value is None:return None
    if 'text' in value:return value['text']
    if 'integer' in value:return int(value['integer'])
    if 'blob_hex' in value:return bytes.fromhex(value['blob_hex'])
    raise AssertionError('unsupported frozen fixture cell')
for table in snapshot['tables']:con.execute('DELETE FROM '+quote(table))
for table,data in snapshot['tables'].items():
    statement='INSERT INTO '+quote(table)+'('+','.join(map(quote,data['columns']))+') VALUES('+','.join('?' for _ in data['columns'])+')'
    con.executemany(statement,[[cell(value) for value in row] for row in data['rows']])
con.commit()
assert con.execute('PRAGMA integrity_check').fetchall()==[('ok',)]
assert con.execute('PRAGMA foreign_key_check').fetchall()==[]
con.close()
"#;
    let mut child = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| Error::Internal)?;
    child
        .stdin
        .take()
        .ok_or(Error::Internal)?
        .write_all(&serde_json::to_vec(&frozen).map_err(|_| Error::Internal)?)
        .map_err(|_| Error::Internal)?;
    let result = child.wait_with_output().map_err(|_| Error::Internal)?;
    if !result.status.success() {
        return Err(Error::Conflict(format!(
            "frozen issuer import failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    Ok(frozen)
}

pub async fn assert_frozen_issuer_static_read(key: &str) -> Result<()> {
    let directory = tempfile::tempdir().map_err(|_| Error::Internal)?;
    let path = directory.path().join("frozen-issuer.sqlite3");
    let store = Store::open(&path).await?;
    let snapshot = restore_frozen_issuer_rows(&path, key).await?;
    let rows = snapshot["tables"]["objects"]["rows"]
        .as_array()
        .ok_or(Error::Internal)?;
    let (namespace, ticket, body) = rows
        .iter()
        .find_map(|row| {
            let namespace = row[0]["text"].as_str()?;
            let body = row[4]["text"].as_str()?;
            let envelope: serde_json::Value = serde_json::from_str(body).ok()?;
            (envelope["record_kind"] == "evaluation_ticket_v2").then(|| {
                (
                    namespace.to_owned(),
                    envelope["payload"].clone(),
                    body.to_owned(),
                )
            })
        })
        .ok_or(Error::NotFound)?;
    let ctx = Context::new(
        &namespace,
        ticket["evaluator_actor"].as_str().ok_or(Error::Internal)?,
        Role::Evaluator,
    )?;
    let id = ticket["id"].as_str().ok_or(Error::Internal)?;
    let mut session = store.session().await?;
    let before = session.list::<serde_json::Value>(&ctx, "artifact").await?;
    session.commit().await?;
    let before_audit = store.verify_audit(&ctx).await?;
    let status =
        evo_engine::streaming_evaluator::IndependentEvaluationControl::status(&ctx, &store, id)
            .await?;
    assert_eq!(status.ticket.schema_version, "rsia.evaluation_ticket.v2");
    assert!(status.ticket.source_plan_binding.is_none());
    assert!(status.ticket.candidate_dependency_binding.is_none());
    assert_eq!(
        serde_json::to_value(&status.ticket).map_err(|_| Error::Internal)?,
        ticket
    );
    let mut session = store.session().await?;
    assert_eq!(
        session.list::<serde_json::Value>(&ctx, "artifact").await?,
        before
    );
    let physical =
        serde_json::from_str::<serde_json::Value>(&body).map_err(|_| Error::Internal)?["id"]
            .as_str()
            .ok_or(Error::Internal)?
            .to_owned();
    assert_eq!(
        session
            .object_row(&ctx, "artifact", &physical)
            .await?
            .ok_or(Error::NotFound)?
            .body,
        body
    );
    session.commit().await?;
    assert_eq!(store.verify_audit(&ctx).await?, before_audit);
    store.close().await;
    Ok(())
}
