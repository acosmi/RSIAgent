//! Returned consumer results survive a later management cancellation.
//! Only local deterministic program fixtures; no provider execution or grade.
use super::*;
use crate::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use crate::exploration::{ExplorationDependency, RootOpportunity, WorldState};
use crate::release_store::{
    CurrentReleaseCandidateRecord, CurrentStageBundleRequest, ReadCandidate, ReleaseStore,
    TypedSourceRef,
};
use crate::streaming_evaluator::{
    EvaluationEvidenceScope, FixedGraderMethod, FixedGraderSpec, FrozenOracleEntry,
    ProtectedInputRef,
};
use evo_core::evaluation::{
    ControlCondition, ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec,
    FormalExperimentPlanV41, FormalStatisticalUnit, Money, OptimizationBudgetPlan,
    OptimizerComparisonContract,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::holdout::{
    AnchorCoverageMatrix, CriticalCapabilityCoverage, FrozenCandidatePair, HoldoutManifest,
    SequentialMode,
};
use evo_core::optimization::{
    OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
};
use evo_core::sequential::{AlphaAllocation, FormalClaimKind, ResearchFamilyAlphaPlan};
use evo_core::skill_edit::EvidenceRef;
use evo_core::strategy::SimulationContext;
use evo_storage::budget::RootBudgetAuthorization;
use serde_json::json;

// These are the necessary data builders from the existing management and
// streaming-evaluator fixtures, without importing their test functions.
fn d(label: &str) -> String {
    hash(label.as_bytes())
}

mod compiler_fixture {
    use crate::formal_authority::{
        CompilerInputRequest, EmptyRootAuthorization, FormalRegistry, FrozenCompilerProfile,
        HOST_SCHEMA, PROFILE_SCHEMA, PROVIDER_SCHEMA, ROOT_SCHEMA, RegisteredExternalProvider,
        RegisteredFormalHost, empty_root_strategy,
    };
    use evo_core::contract::{
        HostCapabilities, ImproverPatch, PatchOp, Profile, ResolvedBundleV3, SkillPatch,
        SkillSnapshot, SystemSnapshot,
    };
    use evo_core::evidence::Purpose;
    use evo_core::{Context, Result, Role, Strategy, fingerprint, hash};
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
    // ProgramFixture content only. The existing no-principal EmptyRoot route
    // cannot execute a provider or issue registered-host formal authority.
    pub async fn software_candidate(
        store: &Store,
        proposer: &Context,
        environment: &str,
        content: &str,
        watermark: u64,
    ) -> Result<ResolvedBundleV3> {
        let f = inputs(
            proposer.namespace(),
            "development-fixture",
            SkillSnapshot::empty(),
            Strategy::default(),
            HostCapabilities {
                available: Default::default(),
                granted: Default::default(),
            },
            environment,
        );
        install(store, &f).await?;
        let input = CompilerInputRequest {
            profile: f.profile.clone(),
            search_parent: f.root.skill.clone(),
            baseline: f.frozen.baseline_skill.clone(),
            search_parent_strategy: f.root.strategy.clone(),
            expected_parent_strategy_digest: Some(fingerprint(&f.root.strategy)?),
            baseline_strategy: f.frozen.baseline_strategy.clone(),
            capabilities: f.frozen.capabilities.clone(),
            environment_digest: environment.into(),
            purpose: Purpose::Development,
            revoke_watermark: watermark,
        };
        let mut session = store.session().await?;
        let bundle = crate::compiler::compile_current_in_session(
            &mut session,
            proposer,
            &input,
            &SkillPatch {
                content: PatchOp::Set {
                    value: content.into(),
                },
                ..SkillPatch::default()
            },
            &ImproverPatch::default(),
        )
        .await?;
        let stored = crate::compiler::verify_current_bundle_in_session(
            &mut session,
            proposer,
            &bundle.digest,
            environment,
            watermark,
        )
        .await?;
        assert_eq!(
            serde_json::to_vec(&stored.bundle).unwrap(),
            serde_json::to_vec(&bundle).unwrap()
        );
        assert_eq!(
            fingerprint(&stored.original_baseline)?,
            fingerprint(&f.frozen)?
        );
        session.commit().await?;
        Ok(bundle)
    }
}
const EXPLORATION_ENVELOPE_SCHEMA: &str = "rsia.exploration_artifact_envelope.v1";
const EXPLORATION_WORLD_KIND: &str = "exploration_world_v1";
const EXPLORATION_RUNS: [&str; 2] = ["run-failure", "run-success"];
fn exploration_world(id: &str, source_watermark: u64) -> ExplorationWorldV1 {
    let parent_skill = hash(b"parent-skill");
    let parent_bundle = hash(b"parent-bundle");
    let environment = hash(b"environment");
    let model = hash(b"model");
    let tools = hash(b"tools");
    let grader = hash(b"grader");
    let rules = hash(b"rules");
    let context_signature = evo_core::fingerprint(&(
        &parent_skill,
        &parent_bundle,
        &environment,
        &model,
        &tools,
        &grader,
        &rules,
        source_watermark,
    ))
    .unwrap();
    ExplorationWorldV1 {
        schema_version: ExplorationWorldV1::SCHEMA.into(),
        id: id.into(),
        approved_parent_digest: hash(b"approved-parent"),
        context_signature,
        parent_skill_digest: parent_skill,
        parent_bundle_digest: parent_bundle,
        environment_digest: environment,
        model_digest: model,
        tools_digest: tools,
        grader_digest: grader,
        rules_digest: rules,
        source_watermark,
        caps: ExplorationCapsV1::online(),
        policy: ElasticPolicyV1::default(),
        simulation: SimulationContext::Online { fixed_seed: 7 },
        root_opportunities: vec![
            RootOpportunity {
                root_slot: 2,
                branch_seq: 2,
                action_seq: 2,
                estimated_cost_upper_micros: 10,
            },
            RootOpportunity {
                root_slot: 1,
                branch_seq: 1,
                action_seq: 1,
                estimated_cost_upper_micros: 10,
            },
        ],
        dependencies: EXPLORATION_RUNS
            .iter()
            .map(|run| ExplorationDependency {
                kind: "run".into(),
                id: (*run).into(),
            })
            .collect(),
        successor_cost_upper_micros: 10,
        initial_baseline_quality_micros: 500_000,
        remaining_root_micros: 1_000,
        remaining_recovery_dispatches: 2,
        state: WorldState::Collecting,
        node_ids: vec![],
        dispatch_ids: vec![],
        history_ids: vec![],
        current_branch_seq: None,
        current_branch_focus_actions: 0,
        decision_round: 0,
        waits: vec![],
    }
}

fn exploration_trace(run: &str, family: &str, outcome: TraceOutcome) -> OptimizationTrace {
    OptimizationTrace {
        run_id: run.into(),
        parent_family: family.into(),
        source_digest: hash(run.as_bytes()),
        purpose: Purpose::Development,
        outcome,
        diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
            kind: SkillFailureKind::SkillDefect,
            skill_id: "skill".into(),
            bundle_digest: hash(b"bundle"),
            request_digest: hash(b"request"),
            rule_id: Some("rule".into()),
            support: vec![EvidenceRef {
                id: run.into(),
                digest: hash(run.as_bytes()),
            }],
            counterexamples: vec![],
            reason: "fixture".into(),
        }),
        excerpt: run.into(),
        seed: 1,
    }
}

/// Seeds the trusted source closure exactly like `exploration_v41`: two
/// Host-issued trace authorities, a source selection grant, and one watermark
/// bump so `source_watermark == 1`.
async fn setup_exploration(store: &Store) {
    let host = Context::new("n", "host", Role::Host).unwrap();
    for (id, family, outcome) in [
        ("run-failure", "family-a", TraceOutcome::TaskFailure),
        ("run-success", "family-b", TraceOutcome::Success),
    ] {
        let authority = StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: id.into(),
                body: id.as_bytes().to_vec(),
                parent_family: family.into(),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: exploration_trace(id, family, outcome),
            excerpt_start: 0,
            excerpt_end: id.len(),
        };
        store_trace_authority(store, &host, &authority)
            .await
            .unwrap();
    }
    store_source_selection(
        store,
        &host,
        &SourceSelection {
            roots: vec![],
            run_ids: EXPLORATION_RUNS.iter().map(|run| (*run).into()).collect(),
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&host, "e09-initial").await.unwrap();
    session.commit().await.unwrap();
}

fn exploration_start_request(request_key: &str, world: &ExplorationWorldV1) -> Value {
    json!({
        "schema_version": "rsia.management.exploration_start.v1",
        "request_key": request_key,
        "world": serde_json::to_value(world).unwrap(),
    })
}

/// Persisted world envelopes, read straight from the store so the assertions
/// do not depend on the consumer's own read path.
async fn exploration_worlds(admin: &Context, store: &Store) -> Vec<Value> {
    let mut session = store.session().await.unwrap();
    let values = session.list::<Value>(admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    values
        .into_iter()
        .filter(|value| {
            value["schema_version"] == EXPLORATION_ENVELOPE_SCHEMA
                && value["record_kind"] == EXPLORATION_WORLD_KIND
        })
        .map(|value| value["payload"].clone())
        .collect()
}

const CURRICULUM_PROFILE_ID: &str = "pure_function_test_proposal.v1";
const CURRICULUM_STATE_ID: &str = "learner-state";
const CURRICULUM_ENVELOPE_SCHEMA: &str = "rsia.curriculum_artifact_envelope.v1";
const PROBE_JOB_KIND: &str = "coverage_probe_job_v1";
const PROBE_RECEIPT_KIND: &str = "probe_schedule_receipt_v1";

fn curriculum_profile() -> evo_core::curriculum::CurriculumControlProfileV1 {
    let registered = crate::curriculum_profiles::RegisteredPureFunctionProfileV1::clamp_i64();
    registered.validate().unwrap();
    evo_core::curriculum::CurriculumControlProfileV1::offline_default(
        CURRICULUM_PROFILE_ID,
        d("task-space"),
        registered.oracle_digest,
        registered.runner_digest,
    )
    .unwrap()
}

fn curriculum_source(
    id: &str,
    source_kind: crate::curriculum::CurriculumSourceKindV1,
    subject_digest: String,
    body: serde_json::Value,
) -> crate::curriculum::CurriculumSourceArtifactV1 {
    crate::curriculum::CurriculumSourceArtifactV1 {
        schema_version: crate::curriculum::CurriculumSourceArtifactV1::SCHEMA.into(),
        id: id.into(),
        source_kind,
        data_use: evo_core::evaluation::DataUse::Development,
        subject_digest,
        body_digest: evo_core::fingerprint(&body).unwrap(),
        body,
        dependency_ids: vec![],
    }
}

fn learner_state() -> evo_core::curriculum::LearnerStateV2 {
    let registered = crate::curriculum_profiles::RegisteredPureFunctionProfileV1::clamp_i64();
    evo_core::curriculum::LearnerStateV2 {
        schema_version: evo_core::curriculum::LearnerStateV2::SCHEMA.into(),
        id: CURRICULUM_STATE_ID.into(),
        profile_id: CURRICULUM_PROFILE_ID.into(),
        skill_snapshot_digest: d("skill"),
        improver_snapshot_digest: d("improver"),
        environment_digest: d("environment"),
        grader_digest: d("grader"),
        model_tools_digest: d("model-tools"),
        runner_digest: registered.runner_digest,
        rules_digest: d("rules"),
        source_watermark: 1,
        source_artifact_ids: vec![
            registered.oracle_source_id,
            registered.runner_source_id,
            registered.target_source_id,
            "task-space-source".into(),
        ],
        development_fact_ids: vec![],
        completed_cycles: vec![],
        failure_clusters: vec![],
        coverage_buckets: vec![],
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    }
}

async fn setup_curriculum(admin: &Context, store: &Store) {
    use crate::curriculum::CurriculumSourceKindV1;
    let coordinator = crate::curriculum::PersistentCurriculumCoordinator::new(
        store.clone(),
        admin.clone(),
        admin.actor(),
    )
    .unwrap();
    coordinator
        .register_source(curriculum_source(
            "task-space-source",
            CurriculumSourceKindV1::TaskSpace,
            d("task-space"),
            json!({"buckets":["clamp-boundary"]}),
        ))
        .await
        .unwrap();
    for (id, body) in crate::curriculum_profiles::registered_profile_source_bodies() {
        let kind = if id.contains("target") || id.contains("source") {
            CurriculumSourceKindV1::TargetSpec
        } else if id.contains("oracle") {
            CurriculumSourceKindV1::OracleSpec
        } else {
            CurriculumSourceKindV1::RunnerSpec
        };
        coordinator
            .register_source(curriculum_source(
                &id,
                kind,
                evo_core::fingerprint(&body).unwrap(),
                body,
            ))
            .await
            .unwrap();
    }
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(admin, "curriculum-initial-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    coordinator
        .register_profile_and_state(curriculum_profile(), learner_state())
        .await
        .unwrap();
}

fn curriculum_step_request(request_key: &str) -> serde_json::Value {
    json!({
        "schema_version": "rsia.management.curriculum_step.v1",
        "request_key": request_key,
        "profile_id": CURRICULUM_PROFILE_ID,
        "state_id": CURRICULUM_STATE_ID,
        "root_budget_limit_micros": 1_000_000u64,
    })
}

/// Typed curriculum records of one kind, read straight from the store so the
/// assertions do not depend on the consumer's own read path.
async fn curriculum_records(
    admin: &Context,
    store: &Store,
    record_kind: &str,
) -> Vec<serde_json::Value> {
    let mut session = store.session().await.unwrap();
    let values = session
        .list::<serde_json::Value>(admin, "artifact")
        .await
        .unwrap();
    session.commit().await.unwrap();
    values
        .into_iter()
        .filter(|value| {
            value["schema_version"] == CURRICULUM_ENVELOPE_SCHEMA
                && value["record_kind"] == record_kind
        })
        .map(|value| value["payload"].clone())
        .collect()
}

fn money(amount: &str) -> Money {
    Money {
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
        amount: amount.into(),
    }
}

fn optimizer(arm: FixedOptimizerArm, implementation: &str) -> FixedOptimizerSpec {
    FixedOptimizerSpec {
        arm,
        implementation_digest: d(implementation),
        initial_s0_digest: d("s0"),
        base_model_digest: d("model"),
        tools_digest: d("tools"),
        authorized_materials_digest: d("materials"),
        task_partition_digest: d("partition"),
        context_limit_tokens: 4096,
        root_budget_scope_id: "billing-1".into(),
        budget_limit: money("0.001"),
    }
}

async fn stage_fixture_candidate(store: &Store) -> CurrentReleaseCandidateRecord {
    let host = Context::new("n", "host", Role::Host).unwrap();
    let proposer = Context::new("n", "proposer", Role::Worker).unwrap();
    let body = "trusted streaming fixture source";
    let source = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "streaming-fixture-source".into(),
            body: body.as_bytes().to_vec(),
            parent_family: "streaming-fixture-family".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: "streaming-fixture-source".into(),
            parent_family: "streaming-fixture-family".into(),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    };
    store_trace_authority(store, &host, &source).await.unwrap();
    let mut session = store.session().await.unwrap();
    let watermark = session
        .bump_watermark(&host, &d("streaming-fixture-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let bundle = compiler_fixture::software_candidate(
        store,
        &proposer,
        &d("environment"),
        "streaming execution fixture",
        watermark as u64,
    )
    .await
    .unwrap();
    let candidate = ReleaseStore::stage_current_bundle(
        &proposer,
        store,
        CurrentStageBundleRequest {
            candidate_id: d("candidate"),
            bundle,
            environment_digest: d("environment"),
            proposer_actor: proposer.actor().into(),
            sources: vec![TypedSourceRef {
                kind: "run".into(),
                id: source.record.id,
                content_digest: source.trace.source_digest,
            }],
            revoke_watermark: watermark as u64,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    let stored: ReadCandidate = session
        .need(&proposer, "artifact", &candidate.id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_vec(stored.current().unwrap()).unwrap(),
        serde_json::to_vec(&candidate).unwrap(),
    );
    let edges = session
        .outgoing_edges_bounded(&proposer, "artifact", &candidate.id, 10_000)
        .await
        .unwrap();
    assert!(edges.contains(&("run".into(), "streaming-fixture-source".into())));
    assert!(edges.contains(&(
        "artifact".into(),
        format!("formal-bundle-{}", candidate.bundle_digest)
    )));
    session.commit().await.unwrap();
    candidate
}

async fn setup_evaluation(store: &Store) {
    let n: u32 = 2;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "billing-1".into(),
                allowed_namespaces: vec!["n".into(), "other".into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "fixture-only".into(),
                authorization_receipt_digest: d("admin-budget-receipt"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();

    let host = Context::new("n", "host", Role::Host).unwrap();
    let body = "trusted streaming fixture source";
    let source = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "streaming-fixture-source".into(),
            body: body.as_bytes().to_vec(),
            parent_family: "streaming-fixture-family".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: "streaming-fixture-source".into(),
            parent_family: "streaming-fixture-family".into(),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    };
    store_trace_authority(store, &host, &source).await.unwrap();

    let mut v1 = ExperimentPlan::first_low_risk("formal-e05", 60).unwrap();
    v1.n_planned = n as usize;
    v1.monetary_budget = money("0.001");
    v1.frozen = true;
    v1.frozen_at = Some(1);
    v1.validate().unwrap();

    let alpha = ResearchFamilyAlphaPlan::new(
        "family-e05",
        "0.05",
        vec![
            AlphaAllocation {
                claim_id: "fixed-gain".into(),
                attempt_id: "attempt-1".into(),
                kind: FormalClaimKind::FixedSampleGain,
                alpha: "0.04".into(),
            },
            AlphaAllocation {
                claim_id: "early-reject".into(),
                attempt_id: "attempt-1".into(),
                kind: FormalClaimKind::SequentialReject,
                alpha: "0.01".into(),
            },
        ],
    )
    .unwrap();
    let anchors = AnchorCoverageMatrix::new(
        "quality-gain",
        "anchors-v1",
        vec![CriticalCapabilityCoverage {
            capability_id: "safety".into(),
            anchor_ids: vec!["anchor-1".into()],
        }],
    )
    .unwrap();
    let mut budget = OptimizationBudgetPlan::unfunded("billing-1", "payer", "USD").unwrap();
    budget.root_total = money("0.001");
    budget.admin_authorization_receipt_digest = Some(d("admin-budget-receipt"));
    for stage in &mut budget.stages {
        stage.per_call_limit.pricing_version = "pricing-v1".into();
        stage.experiment_total.pricing_version = "pricing-v1".into();
    }
    budget.validate().unwrap();
    let comparison = OptimizerComparisonContract::new(
        optimizer(FixedOptimizerArm::CSimple, "simple"),
        optimizer(FixedOptimizerArm::CSkillopt, "skillopt"),
        FixedOptimizerArm::CSkillopt,
    )
    .unwrap();
    assert!(v1.conditions.contains(&ControlCondition::FixedImproverC));
    let formal = FormalExperimentPlanV41::new(
        &v1,
        d("preregistration"),
        &alpha,
        &comparison,
        &budget,
        anchors.digest().unwrap(),
        d("sampling"),
        d("common-mean"),
        FormalStatisticalUnit::IndependentTaskCluster,
        n,
    )
    .unwrap();
    let ordered_clusters: Vec<_> = (0..n).map(|index| format!("cluster-{index}")).collect();
    let grader = FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    };
    let control = crate::streaming_evaluator::RegisteredEvaluationControlV3 {
        schema_version: crate::streaming_evaluator::RegisteredEvaluationControlV3::SCHEMA.into(),
        id: "control-1".into(),
        v1_plan_snapshot: v1.clone(),
        max_regression: evo_core::evaluation::FixedRegressionThreshold::new(0.125).unwrap(),
        formal_plan: formal,
        alpha_plan: alpha.clone(),
        optimizer_comparison: comparison,
        optimization_budget: budget,
        early_stop_plan: None,
        anchor_matrix: anchors.clone(),
        attempt_id: "attempt-1".into(),
        fixed_sample_claim_id: "fixed-gain".into(),
        billing_scope: "billing-1".into(),
        executor_actor: "executor".into(),
        evaluator_actor: "evaluator".into(),
        proposer_actor: "proposer".into(),
        approver_actor: "approver".into(),
        oracle_digest: d("oracle-version"),
        grader_digest: evo_core::fingerprint(&grader).unwrap(),
        fixed_grader: grader,
        evidence_scope: EvaluationEvidenceScope::ProgramFixture,
        created_at_unix_seconds: 2,
    };
    IndependentEvaluationControl::register_current_control(&evaluator, store, control.clone())
        .await
        .unwrap();

    // Registration has committed before the first candidate construction and compile.
    let progress = IndependentEvaluationControl::registration_progress(
        &evaluator,
        store,
        &control.id,
        "holdout-1",
    )
    .await
    .unwrap();
    assert_eq!(
        progress.control_digest.as_deref(),
        Some(control.digest().unwrap().as_str())
    );
    let candidate = stage_fixture_candidate(store).await;

    // The candidate pair binds the actual E01 snapshot, not an arbitrary label.
    let pair = FrozenCandidatePair::new(
        v1.digest().unwrap(),
        candidate.id.clone(),
        candidate.parent_digest.clone(),
        3,
    )
    .unwrap();
    let manifest = HoldoutManifest::issue_after_candidates_frozen(
        &pair,
        "family-e05",
        alpha.digest().unwrap(),
        SequentialMode::Disabled,
        "epoch-1",
        "slice-1",
        ordered_clusters.clone(),
        ordered_clusters
            .iter()
            .map(|id| d(&format!("source-{id}")))
            .collect(),
        "sampler-v1",
        d("oracle-version"),
        anchors.digest().unwrap(),
        evo_core::fingerprint(&FixedGraderSpec {
            schema_version: FixedGraderSpec::SCHEMA.into(),
            version: "exact-json-v1".into(),
            method: FixedGraderMethod::ExactJsonAnswerV1,
        })
        .unwrap(),
        d("environment"),
        d("private-seed"),
        4,
    )
    .unwrap();
    let rotating_inputs: Vec<_> = ordered_clusters
        .iter()
        .enumerate()
        .map(|(index, cluster)| ProtectedInputRef {
            target_id: format!("r{index}"),
            cluster_or_anchor_id: cluster.clone(),
            input_artifact_id: format!("input-r{index}"),
            input_digest: d(&format!("input-r{index}")),
        })
        .collect();
    let anchor_inputs = vec![ProtectedInputRef {
        target_id: "a0".into(),
        cluster_or_anchor_id: "anchor-1".into(),
        input_artifact_id: "input-a0".into(),
        input_digest: d("input-a0"),
    }];
    let oracle_entries: Vec<_> = rotating_inputs
        .iter()
        .chain(anchor_inputs.iter())
        .map(|input| FrozenOracleEntry {
            target_id: input.target_id.clone(),
            expected_answer_json: json!("ok").to_string(),
        })
        .collect();
    let holdout = crate::streaming_evaluator::ProtectedHoldoutRecordV2 {
        schema_version: crate::streaming_evaluator::ProtectedHoldoutRecordV2::SCHEMA.into(),
        id: "holdout-1".into(),
        registration_id: "control-1".into(),
        frozen_configuration_digest: control.frozen_configuration().unwrap().digest().unwrap(),
        pair,
        manifest,
        candidate_bundle_digest: candidate.bundle_digest,
        baseline_bundle_digest: d("baseline-bundle"),
        rotating_inputs,
        anchor_inputs,
        oracle_payload_digest: evo_core::fingerprint(&oracle_entries).unwrap(),
        oracle_entries,
    };
    IndependentEvaluationControl::register_current_holdout(&evaluator, store, holdout.clone())
        .await
        .unwrap();
}

#[cfg(test)]
async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("late-cancel.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

async fn raw_job(ctx: &Context, store: &Store, id: &str) -> ManagementJob {
    let mut session = store.session().await.unwrap();
    let job = session.need(ctx, "job", id).await.unwrap();
    session.commit().await.unwrap();
    job
}

async fn artifacts(ctx: &Context, store: &Store) -> Vec<Value> {
    let mut session = store.session().await.unwrap();
    let mut values = session.list::<Value>(ctx, "artifact").await.unwrap();
    session.commit().await.unwrap();
    values.sort_by_cached_key(|value| serde_json::to_string(value).unwrap());
    values
}

fn assert_job_eq(actual: &ManagementJob, expected: &ManagementJob) {
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

async fn execute_claimed(
    dispatcher: &ManagementDispatcher,
    store: &Store,
    id: &str,
) -> (Context, ManagementJob, JobOutcome) {
    let (ctx, claimed) = dispatcher.claim(id).await.unwrap();
    let claimed = claimed.expect("real management claim");
    let request = load_private_request(&ctx, store, &claimed)
        .await
        .map_err(InputFault::into_error)
        .unwrap();
    let outcome = execute_request(&ctx, store, &claimed, request)
        .await
        .unwrap();
    (ctx, claimed, outcome)
}

// Compare every job field, allowing only the documented finish transition.
fn preserved_job(mut before: ManagementJob, outcome: &JobOutcome, late: bool) -> ManagementJob {
    before.state = outcome.state;
    before.step = outcome.step.clone();
    if late {
        before.step.push_str(":cancel_after_side_effect");
    }
    before.result = outcome.result.clone();
    before.error_code = outcome.error_code.clone();
    before.lease_token = None;
    before.lease_until = 0;
    before
}

async fn assert_replay_and_cancel(
    dispatcher: &ManagementDispatcher,
    ctx: &Context,
    store: &Store,
    operation: &str,
    request: Value,
    expected: &ManagementJob,
) {
    let before = artifacts(ctx, store).await;
    assert_job_eq(
        &dispatcher.submit(ctx, operation, request).await.unwrap(),
        expected,
    );
    assert_job_eq(
        &dispatcher.cancel(ctx, &expected.id).await.unwrap(),
        expected,
    );
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .list::<ManagementJob>(ctx, "job")
            .await
            .unwrap()
            .len(),
        1
    );
    session.commit().await.unwrap();
    assert_eq!(artifacts(ctx, store).await, before);
}

async fn real_world_completion(late: bool) {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_exploration(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let world = exploration_world("late-cancel-world", 1);
    let request = exploration_start_request("late-cancel-world-key", &world);
    let queued = persist_management(&admin, &store, "exploration.start", request.clone())
        .await
        .unwrap();
    let (ctx, claimed, outcome) = execute_claimed(&dispatcher, &store, &queued.id).await;
    assert_eq!(outcome.state, ManagementJobState::Succeeded);
    assert!(outcome.error_code.is_none());
    assert!(
        matches!(&outcome.result, Some(ManagementResult::ExplorationStarted { world_id, .. }) if world_id == &world.id)
    );
    let worlds = exploration_worlds(&admin, &store).await;
    assert_eq!(worlds.len(), 1);
    assert_eq!(worlds[0]["dispatch_ids"], json!([]));
    let before = if late {
        dispatcher.cancel(&admin, &queued.id).await.unwrap()
    } else {
        raw_job(&admin, &store, &queued.id).await
    };
    assert_eq!(before.state, ManagementJobState::Running);
    assert_eq!(before.cancel_requested, late);
    let expected = preserved_job(before, &outcome, late);
    let committed = artifacts(&admin, &store).await;
    finish_claim(&ctx, &store, &claimed, outcome).await.unwrap();
    let done = dispatcher.status(&admin, &queued.id).await.unwrap();
    assert_eq!(done.state, ManagementJobState::Succeeded);
    assert_job_eq(&done, &expected);
    assert_eq!(exploration_worlds(&admin, &store).await, worlds);
    assert_eq!(artifacts(&admin, &store).await, committed);
    assert_replay_and_cancel(
        &dispatcher,
        &admin,
        &store,
        "exploration.start",
        request,
        &done,
    )
    .await;
}

#[tokio::test]
async fn world_commit_survives_late_cancel_with_same_key_and_no_dispatch() {
    real_world_completion(true).await;
}

#[tokio::test]
async fn uncancelled_world_keeps_original_success_without_annotation() {
    real_world_completion(false).await;
}

#[tokio::test]
async fn zero_budget_probe_commit_survives_late_cancel_without_new_probe_or_receipt() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    setup_curriculum(&admin, &store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let request = curriculum_step_request("late-cancel-probe-key");
    assert!(
        curriculum_records(&admin, &store, PROBE_JOB_KIND)
            .await
            .is_empty()
    );
    let queued = persist_management(&admin, &store, "curriculum.step", request.clone())
        .await
        .unwrap();
    let (ctx, claimed, outcome) = execute_claimed(&dispatcher, &store, &queued.id).await;
    assert_eq!(outcome.state, ManagementJobState::Succeeded);
    assert!(matches!(
        &outcome.result,
        Some(ManagementResult::ProbeScheduled {
            terminal: Some(ProbeTerminal::BudgetExhausted),
            ..
        })
    ));
    let probes = curriculum_records(&admin, &store, PROBE_JOB_KIND).await;
    let receipts = curriculum_records(&admin, &store, PROBE_RECEIPT_KIND).await;
    assert_eq!(probes.len(), 1);
    assert_eq!(receipts.len(), 1);
    let before = dispatcher.cancel(&admin, &queued.id).await.unwrap();
    assert_eq!(before.state, ManagementJobState::Running);
    assert!(before.cancel_requested);
    let expected = preserved_job(before, &outcome, true);
    let committed = artifacts(&admin, &store).await;
    finish_claim(&ctx, &store, &claimed, outcome).await.unwrap();
    let done = dispatcher.status(&admin, &queued.id).await.unwrap();
    assert_eq!(done.state, ManagementJobState::Succeeded);
    assert_job_eq(&done, &expected);
    assert_eq!(artifacts(&admin, &store).await, committed);
    assert_replay_and_cancel(
        &dispatcher,
        &admin,
        &store,
        "curriculum.step",
        request,
        &done,
    )
    .await;
    assert_eq!(
        curriculum_records(&admin, &store, PROBE_JOB_KIND).await,
        probes
    );
    assert_eq!(
        curriculum_records(&admin, &store, PROBE_RECEIPT_KIND).await,
        receipts
    );
}

fn evaluation_start_request() -> Value {
    json!({
        "schema_version": "rsia.management.evaluation_start.v1",
        "request_key": "late-cancel-ticket",
        "ticket": {
            "ticket_id": "ticket-1", "registration_id": "control-1",
            "holdout_id": "holdout-1", "issued_at_unix_seconds": 5
        }
    })
}

#[tokio::test]
async fn committed_blocked_ticket_keeps_result_error_and_exposure_after_late_cancel() {
    let (_dir, store) = store().await;
    setup_evaluation(&store).await;
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![evaluator.clone()]).unwrap();
    let request = evaluation_start_request();
    let queued = persist_management(&evaluator, &store, "evaluation.start", request.clone())
        .await
        .unwrap();
    let (ctx, claimed, outcome) = execute_claimed(&dispatcher, &store, &queued.id).await;
    assert_eq!(outcome.state, ManagementJobState::Blocked);
    assert_eq!(
        outcome.error_code.as_deref(),
        Some("execution_provider_unconfigured")
    );
    let ticket_id = match &outcome.result {
        Some(ManagementResult::EvaluationTicketBlocked { ticket_id }) => ticket_id.clone(),
        result => panic!("expected the committed blocked ticket, got {result:?}"),
    };
    let status = IndependentEvaluationControl::status(&evaluator, &store, &ticket_id)
        .await
        .unwrap();
    assert!(
        matches!(&outcome.result, Some(ManagementResult::EvaluationTicketBlocked { ticket_id })
            if ticket_id == &status.ticket.id)
    );
    assert_eq!(
        status
            .ticket
            .source_plan_binding
            .as_ref()
            .unwrap()
            .logical_ticket_id,
        "ticket-1"
    );
    assert_ne!(status.ticket.id, "ticket-1");
    assert!(matches!(
        IndependentEvaluationControl::status(&evaluator, &store, "ticket-1").await,
        Err(Error::NotFound)
    ));
    let ticket = serde_json::to_value(status).unwrap();
    let before = dispatcher.cancel(&evaluator, &queued.id).await.unwrap();
    assert_eq!(before.state, ManagementJobState::Running);
    let expected = preserved_job(before, &outcome, true);
    let committed = artifacts(&evaluator, &store).await;
    finish_claim(&ctx, &store, &claimed, outcome).await.unwrap();
    let done = dispatcher.status(&evaluator, &queued.id).await.unwrap();
    assert_eq!(done.state, ManagementJobState::Blocked);
    assert_job_eq(&done, &expected);
    assert_replay_and_cancel(
        &dispatcher,
        &evaluator,
        &store,
        "evaluation.start",
        request,
        &done,
    )
    .await;
    assert_eq!(
        serde_json::to_value(
            IndependentEvaluationControl::status(&evaluator, &store, &ticket_id)
                .await
                .unwrap()
        )
        .unwrap(),
        ticket
    );
    assert_eq!(artifacts(&evaluator, &store).await, committed);
}

#[tokio::test]
async fn readonly_status_some_is_not_a_committed_write() {
    let (_dir, store) = store().await;
    setup_evaluation(&store).await;
    let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
    let ticket = IndependentEvaluationControl::issue_ticket(
        &evaluator,
        &store,
        IssueTicketRequest {
            ticket_id: "ticket-1".into(),
            registration_id: "control-1".into(),
            holdout_id: "holdout-1".into(),
            issued_at_unix_seconds: 5,
        },
    )
    .await
    .unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![evaluator.clone()]).unwrap();
    let request = json!({
        "schema_version": "rsia.management.evaluation_status.v1",
        "request_key": "late-cancel-status", "ticket_id": ticket.id
    });
    let queued = persist_management(&evaluator, &store, "evaluation.status", request.clone())
        .await
        .unwrap();
    let (ctx, claimed, outcome) = execute_claimed(&dispatcher, &store, &queued.id).await;
    assert_eq!(outcome.state, ManagementJobState::Succeeded);
    assert!(matches!(
        &outcome.result,
        Some(ManagementResult::EvaluationStatus { ticket_id, .. }) if ticket_id == &ticket.id
    ));
    let mut expected = dispatcher.cancel(&evaluator, &queued.id).await.unwrap();
    let committed = artifacts(&evaluator, &store).await;
    expected.state = ManagementJobState::Cancelled;
    expected.step = "cancelled".into();
    expected.result = None;
    expected.error_code = Some("cancelled".into());
    expected.lease_token = None;
    expected.lease_until = 0;
    finish_claim(&ctx, &store, &claimed, outcome).await.unwrap();
    let done = dispatcher.status(&evaluator, &queued.id).await.unwrap();
    assert_job_eq(&done, &expected);
    assert_replay_and_cancel(
        &dispatcher,
        &evaluator,
        &store,
        "evaluation.status",
        request,
        &done,
    )
    .await;
    assert_eq!(artifacts(&evaluator, &store).await, committed);
}

fn meta_request(key: &str) -> Value {
    json!({"schema_version": "rsia.management.meta_start.v1", "request_key": key})
}

#[tokio::test]
async fn blocked_without_committed_result_keeps_original_cancellation() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let request = meta_request("late-cancel-blocked");
    let queued = persist_management(&admin, &store, "meta.start", request.clone())
        .await
        .unwrap();
    let (ctx, claimed, outcome) = execute_claimed(&dispatcher, &store, &queued.id).await;
    assert_eq!(outcome.state, ManagementJobState::Blocked);
    assert!(outcome.result.is_none());
    let mut expected = dispatcher.cancel(&admin, &queued.id).await.unwrap();
    expected.state = ManagementJobState::Cancelled;
    expected.step = "cancelled".into();
    expected.result = None;
    expected.error_code = Some("cancelled".into());
    expected.lease_token = None;
    expected.lease_until = 0;
    finish_claim(&ctx, &store, &claimed, outcome).await.unwrap();
    let done = dispatcher.status(&admin, &queued.id).await.unwrap();
    assert_job_eq(&done, &expected);
    assert_replay_and_cancel(&dispatcher, &admin, &store, "meta.start", request, &done).await;
}

#[tokio::test]
async fn queued_cancel_still_prevents_consumers_and_replays_one_job() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let request = meta_request("late-cancel-queued");
    let queued = persist_management(&admin, &store, "meta.start", request.clone())
        .await
        .unwrap();
    let before = artifacts(&admin, &store).await;
    let cancelled = dispatcher.cancel(&admin, &queued.id).await.unwrap();
    assert_eq!(cancelled.state, ManagementJobState::Cancelled);
    assert!(cancelled.cancel_requested);
    assert!(cancelled.result.is_none());
    assert_eq!(cancelled.step, "cancelled_before_claim");
    assert_eq!(cancelled.error_code.as_deref(), Some("cancelled"));
    assert!(dispatcher.claim(&queued.id).await.unwrap().1.is_none());
    assert_eq!(artifacts(&admin, &store).await, before);
    assert_replay_and_cancel(
        &dispatcher,
        &admin,
        &store,
        "meta.start",
        request,
        &cancelled,
    )
    .await;
}

// These synthetic returned values isolate the finisher's persistence/fence
// contract. They do not replace the real-commit producer tests above.
fn running_job(id: &str) -> ManagementJob {
    ManagementJob {
        id: id.into(),
        schema_version: JOB_SCHEMA.into(),
        operation: "meta.start".into(),
        request_key: id.into(),
        payload_digest: hash(id.as_bytes()),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Running,
        step: "claimed".into(),
        private_input_ref: format!("private-{id}"),
        result: None,
        error_code: Some("prior-diagnostic".into()),
        cancel_requested: true,
        lease_token: Some(format!("lease-{id}")),
        lease_until: now().unwrap() + 3600,
        generation: 7,
        diagnostics: vec![ManagementDiagnostic {
            code: "original-diagnostic".into(),
            observed_generation: 6,
            recorded_at: 2,
        }],
        created_at: 1,
    }
}

fn returned_write_results() -> Vec<ManagementResult> {
    let world = exploration_world("classified-world", 1);
    let decision = crate::exploration::first_decision(&world).unwrap();
    vec![
        ManagementResult::ExperimentRegistered {
            control_id: "control".into(),
            control_digest: hash(b"control"),
            holdout_id: "holdout".into(),
            holdout_digest: hash(b"holdout"),
        },
        ManagementResult::EvaluationTicketBlocked {
            ticket_id: "ticket".into(),
        },
        ManagementResult::ReplayStored {
            report_id: "report".into(),
            pool_digest: hash(b"pool"),
            semantic_reports_digest: hash(b"semantic"),
        },
        ManagementResult::ProbeScheduled {
            probe_job_id: "probe".into(),
            trigger_digest: hash(b"trigger"),
            terminal: Some(ProbeTerminal::BudgetExhausted),
        },
        ManagementResult::ExplorationStarted {
            world_id: world.id,
            context_signature: world.context_signature,
            prefix_digest: decision.prefix_digest,
            legal_actions_digest: decision.legal_actions_digest,
            policy_digest: decision.policy_digest,
            caps_digest: decision.caps_digest,
            action: decision.action,
        },
    ]
}

async fn put_job(ctx: &Context, store: &Store, job: &ManagementJob) {
    let mut session = store.session().await.unwrap();
    session
        .put(ctx, "job", &job.id, ctx.actor(), job)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

#[tokio::test]
async fn all_five_returned_write_results_preserve_every_other_job_field() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    for (index, result) in returned_write_results().into_iter().enumerate() {
        let current = running_job(&format!("returned-write-{index}"));
        put_job(&admin, &store, &current).await;
        let blocked = matches!(result, ManagementResult::EvaluationTicketBlocked { .. });
        let outcome = JobOutcome {
            state: if blocked {
                ManagementJobState::Blocked
            } else {
                ManagementJobState::Succeeded
            },
            step: format!("returned-step-{index}"),
            result: Some(result),
            error_code: blocked.then(|| "execution_provider_unconfigured".into()),
        };
        let expected = preserved_job(current.clone(), &outcome, true);
        finish_claim(&admin, &store, &current, outcome)
            .await
            .unwrap();
        assert_job_eq(&raw_job(&admin, &store, &current.id).await, &expected);
    }
}

#[tokio::test]
async fn owner_token_generation_and_expired_fences_refuse_without_any_job_change() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let intruder = Context::new("n", "intruder", Role::Admin).unwrap();
    for fault in ["owner", "token", "generation", "expired"] {
        let mut current = running_job(&format!("refused-{fault}"));
        if fault == "expired" {
            current.lease_until = 0;
        }
        put_job(&admin, &store, &current).await;
        let mut claimed = current.clone();
        if fault == "token" {
            claimed.lease_token = Some("stale-token".into());
        }
        if fault == "generation" {
            claimed.generation -= 1;
        }
        let ctx = if fault == "owner" { &intruder } else { &admin };
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "replay_stored".into(),
            result: Some(ManagementResult::ReplayStored {
                report_id: "returned-report".into(),
                pool_digest: hash(b"pool"),
                semantic_reports_digest: hash(b"semantic"),
            }),
            error_code: None,
        };
        let error = finish_claim(ctx, &store, &claimed, outcome)
            .await
            .unwrap_err();
        if fault == "owner" {
            assert!(matches!(error, Error::NotFound));
        } else {
            assert!(matches!(error, Error::Conflict(_)));
        }
        assert_job_eq(&raw_job(&admin, &store, &current.id).await, &current);
    }
}

#[tokio::test]
async fn terminal_jobs_keep_complete_original_objects_even_with_stale_finish_and_cancel() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    for (index, state) in [
        ManagementJobState::Succeeded,
        ManagementJobState::Failed,
        ManagementJobState::Cancelled,
        ManagementJobState::Blocked,
    ]
    .into_iter()
    .enumerate()
    {
        let mut current = running_job(&format!("terminal-{index}"));
        current.state = state;
        current.cancel_requested = false;
        current.step = "original-terminal-step".into();
        put_job(&admin, &store, &current).await;
        let mut stale = current.clone();
        stale.generation = 0;
        stale.lease_token = None;
        let outcome = JobOutcome {
            state: ManagementJobState::Succeeded,
            step: "replacement".into(),
            result: Some(ManagementResult::EvaluationTicketBlocked {
                ticket_id: "other-ticket".into(),
            }),
            error_code: None,
        };
        finish_claim(&admin, &store, &stale, outcome).await.unwrap();
        assert_job_eq(&raw_job(&admin, &store, &current.id).await, &current);
        assert_job_eq(
            &dispatcher.cancel(&admin, &current.id).await.unwrap(),
            &current,
        );
    }
}

#[tokio::test]
async fn clock_error_with_a_returned_result_never_rewrites_the_job() {
    let (_dir, store) = store().await;
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let mut current = running_job("clock-returned-result");
    current.result = Some(ManagementResult::EvaluationTicketBlocked {
        ticket_id: "ticket".into(),
    });
    put_job(&admin, &store, &current).await;
    let clock =
        evo_core::clock::unix_seconds_at(std::time::UNIX_EPOCH - std::time::Duration::from_secs(1))
            .unwrap_err();
    let error = execution_outcome(current.clone(), Err(clock)).unwrap_err();
    assert!(matches!(error, Error::ClockRegressed));
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    assert!(matches!(
        dispatcher
            .record_background_error(&current.id, &error)
            .await,
        Err(Error::ClockRegressed)
    ));
    assert_job_eq(&raw_job(&admin, &store, &current.id).await, &current);
}
