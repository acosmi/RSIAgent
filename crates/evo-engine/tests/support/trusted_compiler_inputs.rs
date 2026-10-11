//! Explicit Admin empty-root setup for software fixtures. These configurations
//! never issue ordinary P, provider captures, isolated grades or promotion.
#![allow(dead_code)]
use async_trait::async_trait;
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
