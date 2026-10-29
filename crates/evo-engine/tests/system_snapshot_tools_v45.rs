//! Software producer and complete original controls. Content fixtures never
//! authenticate a new OS principal, formal deployment, provider or paid send.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::contract::*;
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Prepare, Role, Strategy, canonical_fingerprint, fingerprint, hash};
use evo_engine::compiler::{
    AuthenticatedBundleRecordV3, CurrentAuthenticatedBundleRecord, LoadedAuthenticatedBundleRecord,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::formal_authority::{
    CurrentParentMaterial, FormalOperatorCommand, FormalRegistry, FrozenCompilerProfile,
    FrozenCompilerProfileV2, LoadedFrozenCompilerProfile, LoadedRegisteredFormalHost,
    PrincipalAuthorityService, RegisteredFormalHost, RegisteredFormalHostV2, RegisteredPrincipal,
    RegisteredPrincipalSet, project_complete_frozen_profile,
};
use evo_engine::monitoring::{
    ClaimOutcome, CompleteDevelopmentCycleRequest, EnvironmentEvidenceScope, LoadedEnvironment,
    MonitoringCoordinator, RecordEnvironmentRequest, RecordedEnvironment,
};
use evo_engine::optimization::{
    DevelopmentExecutionProvenance, DevelopmentManifest, DevelopmentRunReport, DevelopmentTask,
    OptimizationJournal, OptimizationJournalStage, PairedTaskResult, StageDependency, StageFact,
    StageFactKind,
};
use evo_engine::release_store::{
    LoadedRunApplicationSnapshot, PrepareRunRequest, ReleaseStore, TrustedHostExecutionEvidence,
};
use evo_engine::service::{CompleteHostPrepareConfig, HostService};
use evo_storage::budget::RootBudgetAuthorization;
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use evo_storage::{PersistentContractState, Store, persistent_contract_state};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use trusted_compiler_inputs::StoreOptimizationJournal;

const OLD_HOST: &str = include_str!("fixtures/ag168-legacy-formal-host-v1.raw.json");
const OLD_PROFILE: &str = include_str!("fixtures/ag168-legacy-frozen-profile-v1.raw.json");
const OLD_AUTH: &str =
    include_str!("fixtures/ag168-legacy-authenticated-bundle-v2-candidate.raw.json");
const OLD_BASELINE_AUTH: &str =
    include_str!("fixtures/ag168-legacy-authenticated-bundle-v2-baseline.raw.json");
const OLD_RUN: &str = include_str!("fixtures/ag168-legacy-run-v2.raw.json");
const OLD_ENV: &str = include_str!("fixtures/ag168-legacy-environment-v1.raw.json");
fn d(v: &str) -> String {
    hash(v.as_bytes())
}
fn ctx(actor: &str, role: Role) -> Context {
    Context::new("n", actor, role).unwrap()
}
fn source() -> ToolSourceV1 {
    ToolSourceV1 {
        schema_version: TOOL_SOURCE_SCHEMA.into(),
        domain: "mcp_protocol".into(),
        producer_id: "rmcp.ToolRouter.list_all".into(),
        producer_version: "rmcp-3.3.0".into(),
        definitions: serde_json::from_str(include_str!(
            "fixtures/ag168-actual-mcp-descriptors.json"
        ))
        .unwrap(),
    }
}
fn snapshot(source: ToolSourceV1) -> SystemSnapshotV3 {
    SystemSnapshotV3 {
        schema_version: SYSTEM_SNAPSHOT_SCHEMA_V3.into(),
        profile_id: "p1".into(),
        host_id: "protocol-fixture".into(),
        host_version: "1.0.0".into(),
        model_id: "disabled".into(),
        tools: source.names().unwrap(),
        mandatory_context_digest: d("mandatory"),
        tool_source: source,
    }
}
fn material_evidence(run: &LoadedRunApplicationSnapshot) -> TrustedHostExecutionEvidence {
    TrustedHostExecutionEvidence {
        actual_request_material: run.request_material.clone(),
        environment_digest: run.environment_digest.clone(),
        host_surface_digest: run.host_surface_digest.clone(),
        host_capabilities_digest: run.host_capabilities_digest.clone(),
        offered: vec![],
        attached: vec![],
        used: vec![],
        execution_receipt_id: None,
        truncated: false,
    }
}
const SOFTWARE_TASK_INPUT_UTF8: &str = "software task";
fn software_development_manifest() -> DevelopmentManifest {
    DevelopmentManifest::build(
        "ag168-software-development-manifest",
        vec![DevelopmentTask {
            id: "task".into(),
            parent_family: "software-family".into(),
            input_digest: hash(SOFTWARE_TASK_INPUT_UTF8.as_bytes()),
        }],
    )
    .unwrap()
}
fn software_grader_declaration_digest() -> String {
    // The actual Rust source of this fixture is the declared software grader
    // preimage. It is not an executed grader or OS/provider authority.
    hash(include_bytes!("system_snapshot_tools_v45.rs"))
}
fn environment_request(run_id: &str) -> RecordEnvironmentRequest {
    RecordEnvironmentRequest {
        run_id: run_id.into(),
        development_manifest_digest: software_development_manifest().digest,
        grader_digest: software_grader_declaration_digest(),
        generation_strategy_digest: fingerprint(&Strategy::default()).unwrap(),
        revoke_watermark: 1,
    }
}
fn software_source() -> StoredTraceAuthority {
    let body = b"source content for software fixture";
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "software-source".into(),
            body: body.to_vec(),
            parent_family: "software-family".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: "software-source".into(),
            parent_family: "software-family".into(),
            source_digest: hash(body),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: String::from_utf8(body.to_vec()).unwrap(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    service: HostService,
    config: CompleteHostPrepareConfig,
    admin: Context,
    host: Context,
    agent: Context,
    worker: Context,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("complete.sqlite3"))
            .await
            .unwrap();
        let admin = ctx("admin", Role::Admin);
        let host = ctx("host", Role::Host);
        let agent = ctx("agent", Role::Agent);
        let worker = ctx("worker", Role::Worker);
        let original = json!({"schema_version":"rsia.host_reviewed_declarations.v1","host":"protocol-fixture","host_version":"1.0.0","declarations":[{"name":"model","value_type":"string","enum_values":[],"default":{"kind":"none"}}]});
        ReleaseStore::register_host_surface_from_source(
            &admin,
            &store,
            "surface",
            ReviewedHostSourceEnvelopeV1 {
                schema_version: "rsia.host_source_envelope.v1".into(),
                source_kind: "reviewed_fixture".into(),
                source_schema: "rsia.host_reviewed_declarations.v1".into(),
                extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
                original_utf8: serde_json::to_string(&original).unwrap(),
            },
            HostSurfaceRegistrationV2 {
                adapter_version: "1.0.0".into(),
                classifications: vec![HostSurfaceClassificationV2 {
                    name: "model".into(),
                    coverage: SurfaceCoverage::RuntimeOwned,
                    mapped_field: None,
                    consumer: None,
                    reason: "ToolOnly software fixture has no native instruction slot".into(),
                    renamed_from: None,
                    test_ref: None,
                    capability: None,
                }],
                retired: vec![],
            },
            None,
        )
        .await
        .unwrap();
        store_trace_authority(&store, &host, &software_source())
            .await
            .unwrap();
        let mut s = store.session().await.unwrap();
        assert_eq!(
            s.bump_watermark(&host, &d("initial-watermark"))
                .await
                .unwrap(),
            1
        );
        s.commit().await.unwrap();
        let config = CompleteHostPrepareConfig {
            profile_id: "p1".into(),
            system_snapshot: snapshot(source()),
            host_surface_id: "surface".into(),
            host_capabilities: HostCapabilities {
                available: ["fs_read".into()].into(),
                granted: ["fs_read".into()].into(),
            },
            evolution_enabled: false,
            capability_level: CapabilityLevel::ToolOnly,
        };
        let service = HostService::new(store.clone(), host.clone()).unwrap();
        Self {
            _dir: dir,
            store,
            service,
            config,
            admin,
            host,
            agent,
            worker,
        }
    }
    async fn prepare(&self, key: &str) -> evo_core::Prepared {
        self.service
            .prepare_complete(
                &self.agent,
                Prepare {
                    request_key: key.into(),
                    goal: SOFTWARE_TASK_INPUT_UTF8.into(),
                    capabilities: vec!["fs_read".into()],
                },
                &self.config,
            )
            .await
            .unwrap()
    }
    async fn run(&self, key: &str) -> LoadedRunApplicationSnapshot {
        let p = self.prepare(key).await;
        self.service
            .read_run_snapshot(&self.host, &p.run.id)
            .await
            .unwrap()
    }
    async fn environment(&self, key: &str) -> (LoadedRunApplicationSnapshot, RecordedEnvironment) {
        let run = self.run(key).await;
        self.service
            .record_application(&self.host, &run.run_id, material_evidence(&run))
            .await
            .unwrap();
        let environment = MonitoringCoordinator::record_environment_from_run(
            &self.host,
            &self.store,
            environment_request(&run.run_id),
        )
        .await
        .unwrap();
        (run, environment)
    }
    async fn counts(&self) -> (u64, usize) {
        let mut s = self.store.session().await.unwrap();
        let count = s
            .namespace_object_count(&self.admin, "artifact")
            .await
            .unwrap();
        s.commit().await.unwrap();
        (count, self.store.verify_audit(&self.admin).await.unwrap())
    }
    async fn budget(&self) {
        self.store
            .authorize_root_budget(
                &self.admin,
                &RootBudgetAuthorization {
                    root_budget_id: "root-budget".into(),
                    billing_scope: "billing-scope".into(),
                    allowed_namespaces: vec!["n".into()],
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    payment_subject: "payer".into(),
                    authorization_receipt_digest: d("authorization"),
                    per_call_cap_micros: 100,
                    total_limit_micros: 1000,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
    }
    async fn cycle(
        &self,
        environment: &RecordedEnvironment,
        ordinal: u32,
    ) -> evo_engine::monitoring::DevelopmentCycleRecord {
        let request_id = format!("development-{ordinal}");
        let parent = format!("parent-execution-{ordinal}");
        let candidate = format!("candidate-execution-{ordinal}");
        let execution = format!("runner-execution-{ordinal}");
        let grader = d(&format!("grader-receipt-{ordinal}"));
        let mut s = self.store.session().await.unwrap();
        for id in [&parent, &candidate, &execution, &grader] {
            s.put(&self.worker,"artifact",id,self.worker.actor(),&json!({"id":id,"schema_version":"rsia.development_receipt.fixture.v1","fixture":true})).await.unwrap();
        }
        s.commit().await.unwrap();
        let report = DevelopmentRunReport {
            request_id: request_id.clone(),
            manifest_digest: software_development_manifest().digest,
            parent_bundle_digest: d("parent-bundle"),
            candidate_bundle_digest: d(&format!("candidate-bundle-{ordinal}")),
            environment_digest: environment.environment.environment_digest.clone(),
            grader_digest: software_grader_declaration_digest(),
            results: vec![PairedTaskResult {
                task_id: "task".into(),
                parent_score_micros: 500000,
                candidate_score_micros: 500000,
                parent_passed: true,
                candidate_passed: true,
                parent_execution_id: parent.clone(),
                candidate_execution_id: candidate.clone(),
                grader_receipt_digest: grader.clone(),
            }],
            execution_receipt_id: execution,
            usage_record_ids: vec![],
            provenance: DevelopmentExecutionProvenance::Fixture,
        };
        let fact = StageFact {
            schema_version: "rsia.optimization.stage_fact.v1".into(),
            artifact_id: "placeholder".into(),
            namespace: "n".into(),
            episode_id: format!("development-episode-{ordinal}"),
            step: ordinal,
            attempt: 1,
            stage: OptimizationJournalStage::Development,
            kind: StageFactKind::DevelopmentObserved,
            request_id,
            input_digest: d(&format!("input-{ordinal}")),
            output_digest: None,
            dependencies: vec![
                StageDependency {
                    kind: "execution".into(),
                    id: parent,
                },
                StageDependency {
                    kind: "execution".into(),
                    id: candidate,
                },
                StageDependency {
                    kind: "grader".into(),
                    id: grader,
                },
                StageDependency {
                    kind: "run".into(),
                    id: "software-source".into(),
                },
                StageDependency {
                    kind: "revoke_watermark".into(),
                    id: "1".into(),
                },
            ],
            payload: serde_json::to_value(report).unwrap(),
        }
        .seal()
        .unwrap();
        let fact_id = fact.artifact_id.clone();
        StoreOptimizationJournal::new(self.store.clone(), self.worker.clone(), self.worker.actor())
            .unwrap()
            .commit(fact)
            .await
            .unwrap();
        MonitoringCoordinator::close_development_cycle(
            &self.worker,
            &self.store,
            cycle_request(environment, &fact_id),
        )
        .await
        .unwrap()
    }
}
fn cycle_request(
    environment: &RecordedEnvironment,
    fact_id: &str,
) -> CompleteDevelopmentCycleRequest {
    CompleteDevelopmentCycleRequest {
        skill_id: "skill".into(),
        profile_id: "p1".into(),
        environment_id: environment.environment.id.clone(),
        development_manifest_digest: software_development_manifest().digest,
        grader_digest: software_grader_declaration_digest(),
        generation_strategy_digest: fingerprint(&Strategy::default()).unwrap(),
        parent_skill_digest: d("parent-skill"),
        parent_bundle_digest: d("parent-bundle"),
        billing_scope: "billing-scope".into(),
        root_budget_id: "root-budget".into(),
        report_fact_id: fact_id.into(),
        source_ids: vec!["software-source".into()],
        revoke_watermark: 1,
    }
}
fn complete_profile(source: ToolSourceV1) -> FrozenCompilerProfileV2 {
    let old: FrozenCompilerProfile = serde_json::from_str(OLD_PROFILE).unwrap();
    let mut system = snapshot(source);
    system.host_id = old.system_snapshot.host_id;
    system.host_version = old.system_snapshot.host_version;
    system.model_id = old.system_snapshot.model_id;
    let mut profile = FrozenCompilerProfileV2 {
        id: String::new(),
        schema_version: "rsia.frozen_compiler_profile.v2".into(),
        namespace: old.namespace,
        profile_id: old.profile_id,
        version: old.version,
        host_registration_id: old.host_registration_id,
        provider_registration_id: old.provider_registration_id,
        resource_instrumentation_id: old.resource_instrumentation_id,
        environment_digest: system.environment_digest(&old.capabilities).unwrap(),
        system_snapshot: system,
        capabilities: old.capabilities,
        baseline_skill: old.baseline_skill,
        baseline_strategy: old.baseline_strategy,
        dependencies: old.dependencies,
        purpose: old.purpose,
        application_order: old.application_order,
        authorized_by: old.authorized_by,
    };
    profile.id = profile.recompute_id().unwrap();
    profile
}
/// Data-only original and pure compiler replay. The historical binding copied
/// here has no matching new live registration/principal witnesses and cannot
/// authorize persist/reserve/dispatch through the real verifier.
fn complete_authenticated(
    source: ToolSourceV1,
    parent: Option<ResolvedBundleV3>,
) -> LoadedAuthenticatedBundleRecord {
    let old: CurrentAuthenticatedBundleRecord = serde_json::from_str(OLD_AUTH).unwrap();
    let profile = complete_profile(source);
    let baseline = project_complete_frozen_profile(&profile).unwrap();
    let mut root = match old.original_parent {
        CurrentParentMaterial::EmptyRoot { root } => root,
        _ => unreachable!(),
    };
    root.profile_registration_id = profile.id.clone();
    root.profile_digest = fingerprint(&profile).unwrap();
    root.host_registration_id = profile.host_registration_id.clone();
    let (parent_content, original_parent, parent_digest) = if let Some(bundle) = parent {
        let digest = bundle.digest.clone();
        (
            ParentContentV3::CurrentBundle(bundle.clone()),
            CurrentParentMaterial::ApprovedRelease {
                release_id: "data-only-unregistered-release".into(),
                bundle,
            },
            digest,
        )
    } else {
        let core = EmptyRootContentV1 {
            id: root.id.clone(),
            schema_version: root.schema_version.clone(),
            namespace: root.namespace.clone(),
            profile_registration_id: root.profile_registration_id.clone(),
            profile_digest: root.profile_digest.clone(),
            host_registration_id: root.host_registration_id.clone(),
            skill: root.skill.clone(),
            strategy: root.strategy.clone(),
            authorized_by: root.authorized_by.clone(),
        };
        (
            ParentContentV3::EmptyRoot(core),
            CurrentParentMaterial::EmptyRoot { root: root.clone() },
            root.digest().unwrap(),
        )
    };
    let p = Profile {
        id: profile.profile_id.clone(),
        evolution_enabled: true,
        parent_digest,
        baseline_digest: fingerprint(&profile).unwrap(),
    };
    let proof =
        CompleteProfileContentProofV3::from_content(p.clone(), parent_content, baseline).unwrap();
    let bundle = compile_complete_bundle_v3(CompleteCompilePartsV3 {
        content: &proof,
        skill_patch: &SkillPatch::default(),
        improver_patch: &ImproverPatch::default(),
        revoked: &BTreeSet::new(),
    })
    .unwrap();
    let mut input = old.input;
    input.profile = p;
    input.search_parent = bundle.skill.clone();
    input.search_parent_strategy = bundle.improver.clone();
    input.baseline = profile.baseline_skill.clone();
    input.baseline_strategy = profile.baseline_strategy.clone();
    input.capabilities = profile.capabilities.clone();
    input.environment_digest = profile.environment_digest.clone();
    AuthenticatedBundleRecordV3 {
        id: format!("formal-bundle-{}", bundle.digest),
        schema_version: "rsia.authenticated_bundle.v3".into(),
        bundle,
        input,
        binding: old.binding,
        original_baseline: profile,
        original_parent,
    }
    .into()
}
fn bind_complete_pair(
    mut host: RegisteredFormalHostV2,
    mut set: RegisteredPrincipalSet,
) -> (RegisteredFormalHostV2, RegisteredPrincipalSet) {
    host.namespace = set.namespace.clone();
    host.host_uid = set.host.uid;
    host.grader_uid = set.grader.uid;
    host.administrator_uid = set.administrator.uid;
    host.host_principal = set.host.principal.clone();
    host.grader_principal = set.grader.principal.clone();
    host.administrator_principal = set.administrator.principal.clone();
    host.worker_principal = set.worker.principal.clone();
    host.authorized_by = set.administrator.actor.clone();
    host.credential_binding_digest = set.credential_binding_digest_v2().unwrap();
    host.id = host.recompute_id().unwrap();
    set.id = format!("formal-principals-{}", host.id);
    set.host_registration_id = host.id.clone();
    (host, set)
}
async fn revoke(f: &Fixture, source_id: &str) -> evo_storage::lifecycle::CleanupStatus {
    let mut status = LifecycleStore::begin_revoke(
        &f.admin,
        &f.store,
        TypedObjectRef {
            kind: "run".into(),
            id: source_id.into(),
        },
        "software source revoked",
        100,
    )
    .await
    .unwrap();
    for _ in 0..128 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        status = LifecycleStore::cleanup_step(&f.worker, &f.store, &status.job_id, 32, 101)
            .await
            .unwrap();
    }
    status
}

#[tokio::test]
async fn complete_prepare_checks_initial_two_caches_and_live_sources() {
    let f = Fixture::new().await;
    let request = Prepare {
        request_key: "same-key".into(),
        goal: SOFTWARE_TASK_INPUT_UTF8.into(),
        capabilities: vec!["fs_read".into()],
    };
    let (first, concurrent) = tokio::join!(
        f.service
            .prepare_complete(&f.agent, request.clone(), &f.config),
        f.service
            .prepare_complete(&f.agent, request.clone(), &f.config)
    );
    let first = first.unwrap();
    let concurrent = concurrent.unwrap();
    assert_eq!(first.run.id, concurrent.run.id);
    let repeated = f
        .service
        .prepare_complete(&f.agent, request.clone(), &f.config)
        .await
        .unwrap();
    assert_eq!(
        fingerprint(&first).unwrap(),
        fingerprint(&repeated).unwrap()
    );
    let before = f.counts().await;
    let mut changed = f.config.clone();
    changed.system_snapshot.tool_source.definitions[0]["description"] =
        json!("same name, changed complete schema");
    assert!(matches!(
        f.service
            .prepare_complete(&f.agent, request.clone(), &changed)
            .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(f.counts().await, before);
    changed.system_snapshot.tool_source.domain = "unknown-source".into();
    assert!(matches!(
        f.service
            .prepare_complete(&f.worker, request.clone(), &changed)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        f.service
            .prepare_complete(&f.agent, request, &changed)
            .await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(f.counts().await, before);
    let run = f
        .service
        .read_run_snapshot(&f.host, &first.run.id)
        .await
        .unwrap();
    let mut corrupt = serde_json::to_value(&run).unwrap();
    corrupt["system_snapshot"]["tool_source"]["definitions"][0]["description"] =
        json!("changed after freeze");
    let mut s = f.store.session().await.unwrap();
    s.put(&f.host, "artifact", &run.id, f.host.actor(), &corrupt)
        .await
        .unwrap();
    s.commit().await.unwrap();
    let before = f.counts().await;
    assert!(
        f.service
            .read_run_snapshot(&f.host, &first.run.id)
            .await
            .is_err()
    );
    assert!(
        f.service
            .prepare_complete(
                &f.agent,
                Prepare {
                    request_key: "same-key".into(),
                    goal: SOFTWARE_TASK_INPUT_UTF8.into(),
                    capabilities: vec!["fs_read".into()]
                },
                &f.config
            )
            .await
            .is_err()
    );
    assert_eq!(f.counts().await, before);
    f.store.close().await;
}

#[tokio::test]
async fn run_v3_reload_application_and_idempotency_bind_full_request() {
    let f = Fixture::new().await;
    let run = f.run("request-binding").await;
    let original = run.current_v3().unwrap();
    assert_eq!(
        serde_json::to_value(original)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        18
    );
    assert_eq!(run.id, format!("run-snapshot-v3-{}", run.run_id));
    assert_eq!(
        run.environment_digest,
        canonical_fingerprint(&(
            "rsia.tool_capability_environment.v3",
            &original.system_snapshot,
            &original.host_capabilities
        ))
        .unwrap()
    );
    assert_eq!(
        run.request_digest,
        canonical_fingerprint(&(
            "rsia.applied_request.v3",
            &original.request_material,
            &original.environment_digest,
            &original.host_surface_digest,
            fingerprint(&original.host_capabilities).unwrap()
        ))
        .unwrap()
    );
    let bytes = serde_json::to_vec(&run).unwrap();
    let reload: LoadedRunApplicationSnapshot = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&reload).unwrap(), bytes);
    let a = f
        .service
        .record_application(&f.host, &run.run_id, material_evidence(&run))
        .await
        .unwrap();
    let repeated = f
        .service
        .record_application(&f.host, &run.run_id, material_evidence(&run))
        .await
        .unwrap();
    assert_eq!(fingerprint(&a).unwrap(), fingerprint(&repeated).unwrap());
    let mut changed = material_evidence(&run);
    changed.actual_request_material.task_input_digest = d("other-task");
    let before = f.counts().await;
    assert!(matches!(
        f.service
            .record_application(&f.host, &run.run_id, changed)
            .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(f.counts().await, before);
    let old = SystemSnapshot {
        schema_version: "rsia.system_snapshot.v2".into(),
        profile_id: run.profile_id.clone(),
        host_id: run.system_snapshot.host_id.clone(),
        host_version: run.system_snapshot.host_version.clone(),
        model_id: run.system_snapshot.model_id.clone(),
        tools: run.system_snapshot.tools.clone(),
        mandatory_context_digest: run.system_snapshot.mandatory_context_digest.clone(),
    };
    assert!(matches!(
        ReleaseStore::prepare_run(
            &f.host,
            &f.store,
            PrepareRunRequest {
                run_id: run.run_id.clone(),
                profile_id: run.profile_id.clone(),
                system_snapshot: old,
                host_surface_id: run.host_surface_id.clone(),
                host_capabilities: original.host_capabilities.clone(),
                task_input_digest: run.request_material.task_input_digest.clone(),
                evolution_enabled: run.evolution_enabled,
                capability_level: run.capability_level
            }
        )
        .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(f.counts().await, before);
    // Deliberately invalid alias, never a new authority or a historical upgrade.
    let mut s = f.store.session().await.unwrap();
    s.put(
        &f.host,
        "artifact",
        &format!("run-snapshot-{}", run.run_id),
        f.host.actor(),
        &run,
    )
    .await
    .unwrap();
    s.commit().await.unwrap();
    assert!(
        matches!(f.service.read_run_snapshot(&f.host,&run.run_id).await,Err(Error::Conflict(message)) if message=="run id has both physical generations")
    );
    f.store.close().await;
}

#[tokio::test]
async fn environment_v3_derives_full_original_from_the_same_run() {
    let f = Fixture::new().await;
    let (run, recorded) = f.environment("environment").await;
    let original = recorded.environment.complete().unwrap();
    assert_eq!(
        serde_json::to_value(original)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        23
    );
    assert_eq!(
        original.evidence_scope,
        EnvironmentEvidenceScope::ProgramFixture
    );
    assert_eq!(original.run_scope.as_deref(), Some(run.run_id.as_str()));
    assert_eq!(
        original.system_snapshot,
        run.current_v3().unwrap().system_snapshot
    );
    assert_eq!(
        original.host_capabilities,
        run.current_v3().unwrap().host_capabilities
    );
    assert_eq!(
        original.ordered_tools,
        original.system_snapshot.tool_source.names().unwrap()
    );
    let mut preimage = serde_json::to_value(original).unwrap();
    preimage.as_object_mut().unwrap().remove("id");
    preimage.as_object_mut().unwrap().remove("identity_digest");
    assert_eq!(
        original.identity_digest,
        canonical_fingerprint(&("rsia.monitoring.environment.identity.v3", preimage)).unwrap()
    );
    assert_eq!(
        original.id,
        format!("monitor-env-v3-{}", original.identity_digest)
    );
    let mut s = f.store.session().await.unwrap();
    let stored: LoadedEnvironment = s.need(&f.host, "artifact", &original.id).await.unwrap();
    let edges = s
        .dependency_edges(&f.host, "artifact", &original.id)
        .await
        .unwrap();
    s.commit().await.unwrap();
    assert_eq!(
        serde_json::to_vec(&stored).unwrap(),
        serde_json::to_vec(original).unwrap()
    );
    assert!(edges.contains(&("artifact".into(), run.id.clone())));
    assert!(edges.contains(&(
        "receipt".into(),
        recorded.observation.application_id.clone()
    )));
    let mut other_meaning = serde_json::to_value(original).unwrap();
    other_meaning["schema_version"] = json!("rsia.monitoring.environment.v2");
    assert!(serde_json::from_value::<LoadedEnvironment>(other_meaning).is_err());
    assert_eq!(recorded.observation.request_digest, run.request_digest);
    assert!(recorded.observation.execution_receipt_id.is_none());
    f.store.close().await;
}

#[tokio::test]
async fn environment_v3_cycle_drift_and_claim_require_complete_live_sources() {
    let f = Fixture::new().await;
    f.budget().await;
    let (_, recorded) = f.environment("cycle-environment").await;
    let first = f.cycle(&recorded, 1).await;
    let second = f.cycle(&recorded, 2).await;
    assert_eq!(first.scope_id, second.scope_id);
    let mut s = f.store.session().await.unwrap();
    let old: LoadedEnvironment = serde_json::from_str(OLD_ENV).unwrap();
    s.put(&f.host, "artifact", &old.id, f.host.actor(), &old)
        .await
        .unwrap();
    s.commit().await.unwrap();
    let mut request = cycle_request(&recorded, "unused-fact-after-environment-gate");
    request.environment_id = old.id.clone();
    let before = f.counts().await;
    assert!(
        matches!(MonitoringCoordinator::close_development_cycle(&f.worker,&f.store,request).await,Err(Error::Conflict(message)) if message=="legacy environment cannot authorize new use")
    );
    assert_eq!(f.counts().await, before);
    assert!(
        matches!(MonitoringCoordinator::record_environment_drift(&f.admin,&f.store,&old.id,&recorded.environment.id,&first.scope_id,"legacy source").await,Err(Error::Conflict(message)) if message=="legacy environment cannot authorize new use")
    );
    assert_eq!(f.counts().await, before);
    let run_id = recorded
        .environment
        .complete()
        .unwrap()
        .run_scope
        .as_deref()
        .unwrap();
    let run = f.service.read_run_snapshot(&f.host, run_id).await.unwrap();
    let mut changed = serde_json::to_value(&run).unwrap();
    changed["system_snapshot"]["tool_source"]["definitions"][0]["description"] =
        json!("changed live source");
    let mut s = f.store.session().await.unwrap();
    s.put(&f.host, "artifact", &run.id, f.host.actor(), &changed)
        .await
        .unwrap();
    s.commit().await.unwrap();
    let before = f.counts().await;
    assert!(
        MonitoringCoordinator::claim_consolidation(&f.worker, &f.store, &first.scope_id)
            .await
            .is_err()
    );
    assert!(
        MonitoringCoordinator::close_development_cycle(
            &f.worker,
            &f.store,
            cycle_request(&recorded, "unused-fact-after-source-gate")
        )
        .await
        .is_err()
    );
    assert!(
        MonitoringCoordinator::record_environment_drift(
            &f.admin,
            &f.store,
            &recorded.environment.id,
            &recorded.environment.id,
            &first.scope_id,
            "changed source"
        )
        .await
        .is_err()
    );
    assert_eq!(f.counts().await, before);
    let mut s = f.store.session().await.unwrap();
    let raw: LoadedEnvironment = s.need(&f.host, "artifact", &old.id).await.unwrap();
    s.commit().await.unwrap();
    assert_eq!(serde_json::to_string(&raw).unwrap(), OLD_ENV);
    f.store.close().await;
}

#[tokio::test]
async fn environment_v3_context_change_cannot_reuse_identity() {
    let f = Fixture::new().await;
    let (run, first) = f.environment("contexts").await;
    let same = MonitoringCoordinator::record_environment_from_run(
        &f.host,
        &f.store,
        environment_request(&run.run_id),
    )
    .await
    .unwrap();
    assert_eq!(same.environment, first.environment);
    let mut ids = BTreeSet::new();
    ids.insert(first.environment.id.clone());
    for field in 0..3 {
        let mut r = environment_request(&run.run_id);
        match field {
            0 => r.development_manifest_digest = d("other-manifest"),
            1 => r.grader_digest = d("other-grader"),
            _ => r.generation_strategy_digest = d("other-strategy"),
        };
        let changed = MonitoringCoordinator::record_environment_from_run(&f.host, &f.store, r)
            .await
            .unwrap();
        assert!(ids.insert(changed.environment.id.clone()));
    }
    let (_, second_run) = f.environment("another-run-same-material").await;
    assert_ne!(
        first.environment.identity_digest,
        second_run.environment.identity_digest
    );
    assert_ne!(
        first.environment.complete().unwrap().run_scope,
        second_run.environment.complete().unwrap().run_scope
    );
    let mut wrong = environment_request(&run.run_id);
    wrong.revoke_watermark = 2;
    let before = f.counts().await;
    assert!(matches!(
        MonitoringCoordinator::record_environment_from_run(&f.host, &f.store, wrong.clone()).await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(f.counts().await, before);
    let mut s = f.store.session().await.unwrap();
    assert_eq!(
        s.bump_watermark(&f.host, &d("real-watermark-2"))
            .await
            .unwrap(),
        2
    );
    s.commit().await.unwrap();
    let changed = MonitoringCoordinator::record_environment_from_run(&f.host, &f.store, wrong)
        .await
        .unwrap();
    assert_ne!(changed.environment.id, first.environment.id);
    f.store.close().await;
}

#[tokio::test]
async fn complete_formal_host_profile_and_final_command_are_bound() {
    let f = Fixture::new().await;
    let profile = complete_profile(evo_core::model_protocol::registered_chat_wire_v1_tool_source());
    let mirror = project_complete_frozen_profile(&profile).unwrap();
    assert_eq!(
        serde_json::to_vec(&profile).unwrap(),
        serde_json::to_vec(&mirror).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&mirror)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        17
    );
    let command = FormalOperatorCommand::FreezeProfileComplete {
        profile: Box::new(profile.clone()),
    };
    let raw = serde_json::to_string(&command).unwrap();
    let loaded: FormalOperatorCommand = serde_json::from_str(&raw).unwrap();
    assert_eq!(serde_json::to_string(&loaded).unwrap(), raw);
    let mut changed = profile.clone();
    changed.system_snapshot.tool_source.producer_version = "unregistered-adapter-version".into();
    let changed_command = FormalOperatorCommand::FreezeProfileComplete {
        profile: Box::new(changed.clone()),
    };
    assert_ne!(
        fingerprint(&command).unwrap(),
        fingerprint(&changed_command).unwrap()
    );
    assert!(
        matches!(evo_core::model_protocol::verify_registered_chat_wire_v1_snapshot(&changed.system_snapshot),Err(Error::Conflict(message)) if message=="registered adapter tool source differs")
    );
    let before = f.counts().await;
    assert!(matches!(
        FormalRegistry::freeze_complete_profile(&f.worker, &f.store, changed).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(f.counts().await, before);
    let old: RegisteredFormalHost = serde_json::from_str(OLD_HOST).unwrap();
    let host_value = serde_json::to_value(old).unwrap();
    let mut new_value = host_value;
    new_value["schema_version"] = json!("rsia.registered_formal_host.v2");
    new_value["snapshot"] = serde_json::to_value(&profile.system_snapshot).unwrap();
    let mut host: RegisteredFormalHostV2 = serde_json::from_value(new_value).unwrap();
    let principal = |label: &str, uid: u32| RegisteredPrincipal {
        uid,
        actor: label.into(),
        principal: d(label),
        credential_digest: d(&format!("credential-{label}")),
    };
    let initial_set = RegisteredPrincipalSet {
        id: String::new(),
        schema_version: "rsia.registered_formal_principals.v1".into(),
        namespace: "n".into(),
        host_registration_id: String::new(),
        version: 1,
        host: principal("host", 11),
        grader: principal("grader", 12),
        administrator: principal("admin", 13),
        worker: principal("worker", 14),
    };
    let (paired_host, paired_set) = bind_complete_pair(host.clone(), initial_set);
    paired_set.validate_complete_host(&paired_host).unwrap();
    let credentials = json!({"schema_version":"rsia.formal_principal_credentials.v2","namespace":paired_set.namespace,"version":paired_set.version,"host":paired_set.host,"grader":paired_set.grader,"administrator":paired_set.administrator,"worker":paired_set.worker});
    let independent_k2 =
        canonical_fingerprint(&("rsia.formal_host.credential_binding.v2", credentials)).unwrap();
    assert_eq!(paired_host.credential_binding_digest, independent_k2);
    assert_eq!(
        paired_set.binding_digest().unwrap(),
        fingerprint(&paired_set).unwrap()
    );
    assert_ne!(paired_set.binding_digest().unwrap(), independent_k2);
    let mut payload = serde_json::to_value(&paired_host).unwrap();
    payload.as_object_mut().unwrap().remove("id");
    assert_eq!(
        paired_host.id,
        format!(
            "formal-host-v2-{}",
            canonical_fingerprint(&("rsia.registered_formal_host.v2", payload)).unwrap()
        )
    );
    for role in 0..4 {
        for field in 0..4 {
            let mut changed = paired_set.clone();
            let principal = match role {
                0 => &mut changed.host,
                1 => &mut changed.grader,
                2 => &mut changed.administrator,
                _ => &mut changed.worker,
            };
            match field {
                0 => principal.uid += 100,
                1 => principal.actor.push_str("-changed"),
                2 => principal.principal = d(&format!("new-principal-{role}")),
                _ => principal.credential_digest = d(&format!("new-credential-{role}")),
            }
            assert_ne!(
                changed.credential_binding_digest_v2().unwrap(),
                independent_k2
            );
            assert!(matches!(
                changed.validate_complete_host(&paired_host),
                Err(Error::Forbidden)
            ));
            let (new_host, new_set) = bind_complete_pair(paired_host.clone(), changed);
            new_set.validate_complete_host(&new_host).unwrap();
            assert_ne!(new_host.id, paired_host.id);
        }
    }
    for change_namespace in [false, true] {
        let mut changed = paired_set.clone();
        if change_namespace {
            changed.namespace = "other-namespace".into();
        } else {
            changed.version += 1;
        }
        assert_ne!(
            changed.credential_binding_digest_v2().unwrap(),
            independent_k2
        );
        assert!(matches!(
            changed.validate_complete_host(&paired_host),
            Err(Error::Forbidden)
        ));
        let (h, p) = bind_complete_pair(paired_host.clone(), changed);
        p.validate_complete_host(&h).unwrap();
        assert_ne!(h.id, paired_host.id);
    }
    for reference in [false, true] {
        let mut changed = paired_set.clone();
        if reference {
            changed.host_registration_id = "wrong-host-reference".into();
        } else {
            changed.id = "wrong-principal-set-id".into();
        }
        assert_eq!(
            changed.credential_binding_digest_v2().unwrap(),
            independent_k2
        );
        assert_ne!(
            changed.binding_digest().unwrap(),
            paired_set.binding_digest().unwrap()
        );
        assert!(matches!(
            changed.validate_complete_host(&paired_host),
            Err(Error::Forbidden)
        ));
    }
    let mut swapped = paired_set.clone();
    std::mem::swap(&mut swapped.host, &mut swapped.grader);
    assert_ne!(
        swapped.credential_binding_digest_v2().unwrap(),
        independent_k2
    );
    assert!(matches!(
        swapped.validate_complete_host(&paired_host),
        Err(Error::Forbidden)
    ));
    for field in 0..4 {
        let mut duplicate = paired_set.clone();
        match field {
            0 => duplicate.worker.uid = duplicate.host.uid,
            1 => duplicate.worker.actor = duplicate.host.actor.clone(),
            2 => duplicate.worker.principal = duplicate.host.principal.clone(),
            _ => duplicate.worker.credential_digest = duplicate.host.credential_digest.clone(),
        }
        let (h, p) = bind_complete_pair(paired_host.clone(), duplicate);
        assert!(matches!(
            p.validate_complete_host(&h),
            Err(Error::Forbidden)
        ));
    }
    let mut stale = paired_host.clone();
    stale.credential_binding_digest = d("stale-k2");
    stale.id = stale.recompute_id().unwrap();
    assert!(matches!(
        paired_set.validate_complete_host(&stale),
        Err(Error::Forbidden)
    ));
    // Legacy pairing still uses the entire nine-field Set F, including id/ref.
    let mut old: RegisteredFormalHost = serde_json::from_str(OLD_HOST).unwrap();
    assert_eq!(serde_json::to_string(&old).unwrap(), OLD_HOST);
    let mut legacy_set = paired_set.clone();
    legacy_set.id = format!("formal-principals-{}", old.id);
    legacy_set.host_registration_id = old.id.clone();
    old.host_uid = legacy_set.host.uid;
    old.grader_uid = legacy_set.grader.uid;
    old.administrator_uid = legacy_set.administrator.uid;
    old.host_principal = legacy_set.host.principal.clone();
    old.grader_principal = legacy_set.grader.principal.clone();
    old.administrator_principal = legacy_set.administrator.principal.clone();
    old.worker_principal = legacy_set.worker.principal.clone();
    old.credential_binding_digest = legacy_set.binding_digest().unwrap();
    legacy_set.validate(&old).unwrap();
    assert_eq!(
        legacy_set.binding_digest().unwrap(),
        fingerprint(&legacy_set).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&legacy_set)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        9
    );
    old.credential_binding_digest = legacy_set.credential_binding_digest_v2().unwrap();
    assert!(matches!(legacy_set.validate(&old), Err(Error::Forbidden)));
    host.id = host.recompute_id().unwrap();
    assert_eq!(
        serde_json::to_value(&host)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        25
    );
    host.snapshot.tool_source.producer_version = "unregistered-adapter-version".into();
    let principal = |label: &str, uid: u32| RegisteredPrincipal {
        uid,
        actor: label.into(),
        principal: d(label),
        credential_digest: d(&format!("credential-{label}")),
    };
    let set = RegisteredPrincipalSet {
        id: "data-only-invalid-set".into(),
        schema_version: "rsia.registered_formal_principals.v1".into(),
        namespace: "n".into(),
        host_registration_id: host.id.clone(),
        version: 1,
        host: principal("host", 11),
        grader: principal("grader", 12),
        administrator: principal("admin", 13),
        worker: principal("worker", 14),
    };
    assert!(
        matches!(PrincipalAuthorityService::new(host,set),Err(Error::Conflict(message)) if message=="registered adapter tool source differs")
    );
    let duplicate = raw.replacen(
        "\"producer_version\":",
        "\"producer_version\":\"first\",\"producer_version\":",
        1,
    );
    assert!(serde_json::from_str::<FormalOperatorCommand>(&duplicate).is_err());
    f.store.close().await;
}

#[test]
fn complete_authenticated_baseline_and_parent_reload_keep_full_originals() {
    let content = complete_authenticated(source(), None);
    content.validate_content_version().unwrap();
    let raw = serde_json::to_string(&content).unwrap();
    let loaded: LoadedAuthenticatedBundleRecord = serde_json::from_str(&raw).unwrap();
    assert_eq!(serde_json::to_string(&loaded).unwrap(), raw);
    assert_eq!(
        loaded
            .original_baseline
            .complete()
            .unwrap()
            .system_snapshot
            .tool_source,
        source()
    );
    let previous = serde_json::from_str::<CurrentAuthenticatedBundleRecord>(OLD_BASELINE_AUTH)
        .unwrap()
        .bundle;
    let with_parent = complete_authenticated(source(), Some(previous.clone()));
    with_parent.validate_content_version().unwrap();
    assert_eq!(with_parent.bundle.parent_digest, previous.digest);
    assert_ne!(with_parent.bundle.baseline_digest, previous.baseline_digest);
    let roundtrip: LoadedAuthenticatedBundleRecord =
        serde_json::from_slice(&serde_json::to_vec(&with_parent).unwrap()).unwrap();
    assert_eq!(
        fingerprint(&roundtrip).unwrap(),
        fingerprint(&with_parent).unwrap()
    );
    let mut mismatch = serde_json::to_value(&content).unwrap();
    mismatch["original_baseline"]["system_snapshot"]["tool_source"]["definitions"][0]["description"] =
        json!("uncommitted nested source");
    assert!(serde_json::from_value::<LoadedAuthenticatedBundleRecord>(mismatch).is_err());
    for key in [
        "namespace",
        "authorized_by",
        "resource_instrumentation_id",
        "dependencies",
    ] {
        let mut bad = serde_json::to_value(&content).unwrap();
        bad["original_baseline"][key] = json!("not the original typed field");
        assert!(
            serde_json::from_value::<LoadedAuthenticatedBundleRecord>(bad).is_err(),
            "{key}"
        );
    }
    // No Store persistence or legitimate new-Host2 authority is asserted here.
}

#[test]
fn complete_authenticated_loaded_original_fingerprint_keeps_nested_source() {
    let original = complete_authenticated(source(), None);
    let raw = serde_json::to_vec(&original).unwrap();
    let loaded: LoadedAuthenticatedBundleRecord = serde_json::from_slice(&raw).unwrap();
    assert_eq!(serde_json::to_vec(&loaded).unwrap(), raw);
    let mut changed_source = source();
    changed_source.definitions[0]["annotations"] =
        json!({"nested":{"readOnlyHint":true,"order":["a","b"]}});
    let changed = complete_authenticated(changed_source, None);
    changed.validate_content_version().unwrap();
    assert_ne!(
        fingerprint(&loaded).unwrap(),
        fingerprint(&changed).unwrap()
    );
    let raw = String::from_utf8(raw).unwrap();
    for duplicate in [
        raw.replacen(
            "\"schema_version\":",
            "\"schema_version\":\"rsia.authenticated_bundle.v2\",\"schema_version\":",
            1,
        ),
        raw.replacen(
            "\"description\":",
            "\"description\":\"first\",\"description\":",
            1,
        ),
    ] {
        assert!(serde_json::from_str::<LoadedAuthenticatedBundleRecord>(&duplicate).is_err());
    }
    let mut unknown = serde_json::to_value(&original).unwrap();
    unknown["schema_version"] = json!("rsia.authenticated_bundle.v99");
    assert!(serde_json::from_value::<LoadedAuthenticatedBundleRecord>(unknown).is_err());
    let mut downgrade = serde_json::to_value(&original).unwrap();
    downgrade["schema_version"] = json!("rsia.authenticated_bundle.v2");
    assert!(serde_json::from_value::<LoadedAuthenticatedBundleRecord>(downgrade).is_err());
    let old: LoadedAuthenticatedBundleRecord = serde_json::from_str(OLD_AUTH).unwrap();
    assert_eq!(serde_json::to_string(&old).unwrap(), OLD_AUTH);
    assert!(old.v2().is_some());
    // This is complete-wire/F evidence only; no Reserved/paid/4-UID dispatch is fabricated.
}

#[tokio::test]
async fn actual_historical_run2_env1_constants_remain_readable_cleanable_and_linked() {
    assert_eq!(
        hash(OLD_RUN.as_bytes()),
        "6c4b5627efb8cec84639a18de76f4781a608c888813d18a760959cf2d49691be"
    );
    assert_eq!(
        hash(OLD_ENV.as_bytes()),
        "d8d9b25c9dbc97972756de59ff9379e99e668d5183bdf547a1197bf86f7bb676"
    );
    let run: LoadedRunApplicationSnapshot = serde_json::from_str(OLD_RUN).unwrap();
    let environment: LoadedEnvironment = serde_json::from_str(OLD_ENV).unwrap();
    assert_eq!(serde_json::to_string(&run).unwrap(), OLD_RUN);
    assert_eq!(serde_json::to_string(&environment).unwrap(), OLD_ENV);
    assert!(run.current_v3().is_none());
    assert!(environment.complete().is_none());
    assert_eq!(
        run.request_digest,
        fingerprint(&run.request_material).unwrap()
    );
    assert_eq!(
        run.environment_digest,
        run.system_snapshot.digest().unwrap()
    );
    assert_eq!(run.id, format!("run-snapshot-{}", run.run_id));
    assert_eq!(environment.environment_digest, run.environment_digest);
    let identity = fingerprint(&(
        environment.namespace.as_str(),
        environment.evidence_scope,
        Some(run.run_id.as_str()),
        &run.profile_id,
        &run.release_id,
        &run.bundle_digest,
        &run.environment_digest,
        &run.system_snapshot,
        &run.host_surface_digest,
        &run.host_capabilities_digest,
        &environment.development_manifest_digest,
        &environment.grader_digest,
        &environment.generation_strategy_digest,
        environment.revoke_watermark,
    ))
    .unwrap();
    assert_eq!(identity, environment.identity_digest);
    assert_eq!(environment.id, format!("monitor-env-{}", &identity[..32]));

    let host: LoadedRegisteredFormalHost = serde_json::from_str(OLD_HOST).unwrap();
    let profile: LoadedFrozenCompilerProfile = serde_json::from_str(OLD_PROFILE).unwrap();
    assert_eq!(serde_json::to_string(&host).unwrap(), OLD_HOST);
    assert_eq!(serde_json::to_string(&profile).unwrap(), OLD_PROFILE);
    // Actual old read_config and today's complete four-tool source are distinct
    // inputs. Missing historical D is not invented as a same-input rerun.
    assert_ne!(run.system_snapshot.tools, source().names().unwrap());
    assert_eq!(run.system_snapshot.tools, vec!["read_config"]);
    let f = Fixture::new().await;
    let mut s = f.store.session().await.unwrap();
    s.put(&f.host, "artifact", &run.id, f.host.actor(), &run)
        .await
        .unwrap();
    s.put(
        &f.host,
        "artifact",
        &environment.id,
        f.host.actor(),
        &environment,
    )
    .await
    .unwrap();
    s.put_edge(&f.host, "artifact", &run.id, "run", "software-source")
        .await
        .unwrap();
    s.put_edge(&f.host, "artifact", &environment.id, "artifact", &run.id)
        .await
        .unwrap();
    s.commit().await.unwrap();
    let status = revoke(&f, "software-source").await;
    assert_eq!(status.state, CleanupState::Complete);
    let mut s = f.store.session().await.unwrap();
    let old_run: Value = s.need(&f.admin, "artifact", &run.id).await.unwrap();
    let old_environment: Value = s.need(&f.admin, "artifact", &environment.id).await.unwrap();
    s.commit().await.unwrap();
    assert_eq!(old_run["schema_version"], "rsia.redacted.v1");
    assert_eq!(old_run["original_schema"], "rsia.run_application.v2");
    assert_eq!(
        old_environment["original_schema"],
        "rsia.monitoring.environment.v1"
    );
    f.store.close().await;
}

#[test]
fn complete_tool_contract_versions_have_unique_three_state_classification() {
    let mut seen = BTreeSet::new();
    for version in evo_storage::PERSISTENT_CONTRACT_VERSIONS {
        assert!(
            seen.insert(version.schema),
            "duplicate schema {}",
            version.schema
        );
    }
    for schema in [
        "rsia.run_application.v3",
        "rsia.monitoring.environment.v3",
        "rsia.registered_formal_host.v2",
        "rsia.frozen_compiler_profile.v2",
        "rsia.authenticated_bundle.v3",
    ] {
        assert_eq!(
            persistent_contract_state(schema, true).unwrap(),
            PersistentContractState::CurrentVerified
        );
        assert_eq!(
            persistent_contract_state(schema, false).unwrap(),
            PersistentContractState::Invalid
        );
    }
    for schema in [
        "rsia.run_application.v1",
        "rsia.run_application.v2",
        "rsia.monitoring.environment.v1",
        "rsia.registered_formal_host.v1",
        "rsia.frozen_compiler_profile.v1",
        "rsia.authenticated_bundle.v2",
    ] {
        assert_eq!(
            persistent_contract_state(schema, true).unwrap(),
            PersistentContractState::Legacy
        );
        assert_eq!(
            persistent_contract_state(schema, false).unwrap(),
            PersistentContractState::Invalid
        );
    }
    assert!(persistent_contract_state("rsia.authenticated_bundle.v999", true).is_err());
}

#[tokio::test]
async fn complete_tool_original_redaction_drops_content_and_preserves_metadata() {
    let f = Fixture::new().await;
    f.budget().await;
    let (run, environment) = f.environment("cleanup").await;
    let first = f.cycle(&environment, 1).await;
    f.cycle(&environment, 2).await;
    let claim =
        match MonitoringCoordinator::claim_consolidation(&f.worker, &f.store, &first.scope_id)
            .await
            .unwrap()
        {
            ClaimOutcome::Claimed(c) => c,
            _ => unreachable!(),
        };
    let terminal = MonitoringCoordinator::complete_no_contrast(&f.worker, &f.store, &claim.id)
        .await
        .unwrap();
    let mut s = f.store.session().await.unwrap();
    s.put_edge(&f.host, "artifact", &run.id, "run", "software-source")
        .await
        .unwrap();
    let before: Value = s.need(&f.worker, "artifact", &terminal.id).await.unwrap();
    s.commit().await.unwrap();
    let budget = f
        .store
        .root_budget(&f.admin, "billing-scope")
        .await
        .unwrap()
        .unwrap();
    let status = revoke(&f, "software-source").await;
    assert_eq!(status.state, CleanupState::Complete);
    let mut s = f.store.session().await.unwrap();
    let after: Value = s.need(&f.worker, "artifact", &terminal.id).await.unwrap();
    let raw: Value = s
        .need(&f.admin, "artifact", &environment.environment.id)
        .await
        .unwrap();
    let raw_run: Value = s.need(&f.admin, "artifact", &run.id).await.unwrap();
    s.commit().await.unwrap();
    assert_eq!(after, before);
    assert_eq!(
        fingerprint(&budget).unwrap(),
        fingerprint(
            &f.store
                .root_budget(&f.admin, "billing-scope")
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(raw["schema_version"], "rsia.redacted.v1");
    assert_eq!(raw["original_schema"], "rsia.monitoring.environment.v3");
    assert!(raw.get("system_snapshot").is_none());
    assert_eq!(raw_run["original_schema"], "rsia.run_application.v3");
    assert!(raw_run.get("request_material").is_none());
    f.store.close().await;
    let f = Fixture::new().await;
    let mut s = f.store.session().await.unwrap();
    s.put(&f.worker,"artifact","unknown-content",f.worker.actor(),&json!({"id":"unknown-content","schema_version":"rsia.authenticated_bundle.v99","private":"must not be preserved"})).await.unwrap();
    s.put_edge(
        &f.worker,
        "artifact",
        "unknown-content",
        "run",
        "software-source",
    )
    .await
    .unwrap();
    s.commit().await.unwrap();
    assert_eq!(
        revoke(&f, "software-source").await.state,
        CleanupState::Failed
    );
    f.store.close().await;
}
