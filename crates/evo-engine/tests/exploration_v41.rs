use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
use evo_core::evaluation::DataUse;
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, Purpose, SourceCoverage, SourceSelection,
    TaskOrigin,
};
use evo_core::optimization::{
    EditSuggestion, ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace,
    SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::strategy::{
    BatchActionV1, ElasticPolicyV1, ExplorationCapsV1, HistoryOutcome, HistoryQuery,
    OptimizationHistoryEntry, SimulationContext,
};
use evo_core::{Context, Error, Result, Role, Strategy, hash};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    ExplorationDependency, ExplorationWorldV1, PersistentCoordinator, RootOpportunity, WorldState,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OptimizationJournal,
    OptimizationStepRequest, PairedTaskResult, StageFact, StageFactKind, StoreOptimizationJournal,
};
use evo_storage::Store;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn world() -> ExplorationWorldV1 {
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
        1u64,
    ))
    .unwrap();
    ExplorationWorldV1 {
        schema_version: ExplorationWorldV1::SCHEMA.into(),
        id: "world-1".into(),
        approved_parent_digest: hash(b"approved-parent"),
        context_signature,
        parent_skill_digest: parent_skill,
        parent_bundle_digest: parent_bundle,
        environment_digest: environment,
        model_digest: model,
        tools_digest: tools,
        grader_digest: grader,
        rules_digest: rules,
        source_watermark: 1,
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
        dependencies: vec![
            ExplorationDependency {
                kind: "run".into(),
                id: "run-failure".into(),
            },
            ExplorationDependency {
                kind: "run".into(),
                id: "run-success".into(),
            },
        ],
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

async fn coordinator() -> (tempfile::TempDir, PersistentCoordinator, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("exploration.sqlite3"))
        .await
        .unwrap();
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
            trace: trace(id, family, outcome),
            excerpt_start: 0,
            excerpt_end: id.len(),
        };
        store_trace_authority(&store, &host, &authority)
            .await
            .unwrap();
    }
    store_source_selection(
        &store,
        &host,
        &SourceSelection {
            roots: vec![],
            run_ids: vec!["run-failure".into(), "run-success".into()],
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&host, "e09-initial").await.unwrap();
    session.commit().await.unwrap();
    let context = Context::new("n", "worker", Role::Worker).unwrap();
    let coordinator = PersistentCoordinator::new(store.clone(), context, "worker").unwrap();
    (dir, coordinator, store)
}

struct NoChangeModel;

#[async_trait]
impl ModelPort for NoChangeModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        let output = "[]".to_string();
        Ok(ModelResponse::Completed {
            request_id: request.request_id,
            response_id: "response".into(),
            actual_model_digest: request.model_digest,
            input_digest: request.input_digest,
            output_digest: hash(output.as_bytes()),
            output,
            execution_receipt: ModelExecutionReceipt {
                call_id: "call".into(),
                dispatch_id: "dispatch".into(),
                root_budget_id: "budget".into(),
                provider_request_id: "provider".into(),
                usage_record_id: "usage".into(),
                provenance: ModelExecutionProvenance::Fixture,
            },
        })
    }
}

struct EditingFixtureModel;

#[async_trait]
impl ModelPort for EditingFixtureModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        let parent: SkillSnapshot = serde_json::from_str(
            &request
                .input
                .iter()
                .find(|part| part.label == "parent-skill")
                .ok_or_else(|| Error::Invalid("fixture parent input missing".into()))?
                .content,
        )
        .map_err(|_| Error::Invalid("fixture parent input invalid".into()))?;
        let source = request.source_closure[0].clone();
        let (id, batch_id, field, start) = match request.stage {
            ModelStage::ReflectFailure => (
                "repair-content",
                "reflection-failure",
                SkillTextField::Content,
                parent.content.len(),
            ),
            ModelStage::ReflectSuccess => (
                "preserve-applicability",
                "reflection-success",
                SkillTextField::Applicability,
                parent.applicability.len(),
            ),
            _ => return Err(Error::Invalid("unexpected fixture stage".into())),
        };
        let suggestion = EditSuggestion {
            id: id.into(),
            hypothesis: "bounded deterministic fixture repair".into(),
            batch_ids: vec![batch_id.into()],
            support: vec![source.clone()],
            counterexamples: (request.stage == ModelStage::ReflectSuccess)
                .then_some(source.clone())
                .into_iter()
                .collect(),
            dependencies: vec![source],
            edit: SkillTextEdit {
                field,
                start,
                end: start,
                expected_text_digest: hash(b""),
                exact_anchor: None,
                operation: TextEditOperation::Insert {
                    text: " [repair]".into(),
                },
            },
        };
        let output = serde_json::to_string(&vec![suggestion]).unwrap();
        Ok(ModelResponse::Completed {
            request_id: request.request_id,
            response_id: format!("fixture-edit-response-{id}"),
            actual_model_digest: request.model_digest,
            input_digest: request.input_digest,
            output_digest: hash(output.as_bytes()),
            output,
            execution_receipt: ModelExecutionReceipt {
                call_id: format!("fixture-edit-call-{id}"),
                dispatch_id: format!("fixture-edit-dispatch-{id}"),
                root_budget_id: "fixture-budget".into(),
                provider_request_id: format!("fixture-provider-{id}"),
                usage_record_id: format!("fixture-usage-{id}"),
                provenance: ModelExecutionProvenance::Fixture,
            },
        })
    }
}

#[derive(Default)]
struct CountingEditingFixtureModel(AtomicUsize);

#[async_trait]
impl ModelPort for CountingEditingFixtureModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        EditingFixtureModel.dispatch(request).await
    }
}

#[derive(Default)]
struct NotFoundAfterDispatchModel(AtomicUsize);

#[async_trait]
impl ModelPort for NotFoundAfterDispatchModel {
    async fn dispatch(&self, _: ModelRequest) -> Result<ModelResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(Error::NotFound)
    }
}

struct ImprovingFixtureRunner;

#[async_trait]
impl DevRunner for ImprovingFixtureRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        Ok(DevelopmentRunReport {
            request_id: request.request_id,
            manifest_digest: request.manifest.digest,
            parent_bundle_digest: request.parent_bundle_digest,
            candidate_bundle_digest: request.candidate_bundle_digest,
            environment_digest: request.environment_digest,
            grader_digest: request.grader_digest,
            results: vec![PairedTaskResult {
                task_id: "task".into(),
                parent_score_micros: 500_000,
                candidate_score_micros: 900_000,
                parent_passed: true,
                candidate_passed: true,
                parent_execution_id: "fixture-parent-execution".into(),
                candidate_execution_id: "fixture-candidate-execution".into(),
                grader_receipt_digest: hash(b"fixture-grade"),
            }],
            execution_receipt_id: "fixture-development-execution".into(),
            usage_record_ids: vec!["fixture-development-usage".into()],
            provenance: DevelopmentExecutionProvenance::Fixture,
        })
    }
}

#[derive(Default)]
struct CountingNoChangeModel(AtomicUsize);

#[async_trait]
impl ModelPort for CountingNoChangeModel {
    async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        NoChangeModel.dispatch(request).await
    }
}

struct UnusedRunner;

#[async_trait]
impl DevRunner for UnusedRunner {
    async fn run(&self, _: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        Err(Error::Invalid(
            "no-change fixture must not run development".into(),
        ))
    }
}

struct CrashAfterDispatchJournal {
    inner: StoreOptimizationJournal,
    failed: AtomicBool,
}

#[async_trait]
impl OptimizationJournal for CrashAfterDispatchJournal {
    async fn commit(&self, fact: StageFact) -> Result<()> {
        if fact.kind == StageFactKind::StepCompleted && !self.failed.swap(true, Ordering::SeqCst) {
            return Err(Error::Internal);
        }
        self.inner.commit(fact).await
    }

    async fn lookup(&self, artifact_id: &str) -> Result<Option<StageFact>> {
        self.inner.lookup(artifact_id).await
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

    async fn check_live(&self, source_ids: &[String], watermark: u64) -> Result<()> {
        self.inner.check_live(source_ids, watermark).await
    }
}

fn trace(run: &str, family: &str, outcome: TraceOutcome) -> OptimizationTrace {
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

#[tokio::test]
async fn persistent_world_reconnects_to_the_same_pure_decision() {
    let (_dir, coordinator, store) = coordinator().await;
    coordinator.register_world(world()).await.unwrap();
    let first = coordinator.decide_next("world-1").await.unwrap();
    assert!(matches!(
        first.action,
        BatchActionV1::Dispatch {
            ref action_seqs,
            ..
        } if action_seqs == &[1]
    ));
    store.close().await;

    let reopened = Store::open(&_dir.path().join("exploration.sqlite3"))
        .await
        .unwrap();
    let resumed = PersistentCoordinator::new(
        reopened.clone(),
        Context::new("n", "worker", Role::Worker).unwrap(),
        "worker",
    )
    .unwrap();
    let second = resumed.decide_next("world-1").await.unwrap();
    assert_eq!(first.prefix_digest, second.prefix_digest);
    assert_eq!(first.legal_actions_digest, second.legal_actions_digest);
    assert_eq!(
        serde_json::to_value(first.action).unwrap(),
        serde_json::to_value(second.action).unwrap()
    );
}

#[tokio::test]
async fn coordinator_calls_the_existing_optimization_consumer_before_observing() {
    let (_dir, coordinator, _store) = coordinator().await;
    let evidence = EvidenceSet::build(
        "evidence",
        vec![
            EvidenceMember {
                source_id: "run-failure".into(),
                content_digest: hash(b"run-failure"),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            EvidenceMember {
                source_id: "run-success".into(),
                content_digest: hash(b"run-success"),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
        ],
        SourceCoverage {
            discovery_exhausted: true,
            files_known: true,
            parsed_ok: 2,
            bytes_read: 22,
            ..SourceCoverage::default()
        },
        ["family-a".into(), "family-b".into()].into_iter().collect(),
    )
    .unwrap();
    let parent = SkillSnapshot {
        content: "old".into(),
        applicability: "applies".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    let mut registered_world = world();
    registered_world.parent_skill_digest = skill_snapshot_digest(&parent).unwrap();
    registered_world.context_signature = evo_core::fingerprint(&(
        &registered_world.parent_skill_digest,
        &registered_world.parent_bundle_digest,
        &registered_world.environment_digest,
        &registered_world.model_digest,
        &registered_world.tools_digest,
        &registered_world.grader_digest,
        &registered_world.rules_digest,
        registered_world.source_watermark,
    ))
    .unwrap();
    coordinator.register_world(registered_world).await.unwrap();
    let allowed = vec![
        EvidenceRef {
            id: "run-failure".into(),
            digest: hash(b"run-failure"),
        },
        EvidenceRef {
            id: "run-success".into(),
            digest: hash(b"run-success"),
        },
    ];
    let edit_context = TrustedEditContext::new(
        "n",
        "profile",
        "skill",
        "v1",
        hash(b"approved-parent"),
        hash(b"baseline"),
        &parent,
        allowed.clone(),
    )
    .unwrap();
    let source_selection = SourceSelection {
        roots: vec![],
        run_ids: vec!["run-failure".into(), "run-success".into()],
        purpose: Purpose::Development,
        allow_model_excerpts: true,
    };
    let bindings = vec![
        TrustedSourceBinding {
            source_id: "run-failure".into(),
            source_digest: hash(b"run-failure"),
            parent_family: "family-a".into(),
        },
        TrustedSourceBinding {
            source_id: "run-success".into(),
            source_digest: hash(b"run-success"),
            parent_family: "family-b".into(),
        },
    ];
    let profile = Profile {
        id: "profile".into(),
        evolution_enabled: true,
        parent_digest: hash(b"approved-parent"),
        baseline_digest: hash(b"baseline"),
    };
    let baseline = parent.clone();
    let parent_strategy = Strategy::default();
    let baseline_strategy = Strategy::default();
    let improver = ImproverPatch::default();
    let caps = HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    };
    let revoked = BTreeSet::new();
    let journal = StoreOptimizationJournal::new(
        _store.clone(),
        Context::new("n", "worker", Role::Worker).unwrap(),
        "worker",
    )
    .unwrap();
    let initial_parent_bundle = hash(b"parent-bundle");
    macro_rules! request {
        ($world:expr, $step:expr, $request_id:literal, $dev_id:literal, $idempotency:literal, $parent:ident, $edit_context:ident, $parent_bundle:ident) => {
            OptimizationStepRequest {
                evidence: &evidence,
                source_selection: &source_selection,
                source_bindings: &bindings,
                traces: vec![
                    trace("run-failure", "family-a", TraceOutcome::TaskFailure),
                    trace("run-success", "family-b", TraceOutcome::Success),
                ],
                model_context: ModelRequestContext {
                    request_id: $request_id.into(),
                    namespace: "n".into(),
                    purpose: Purpose::Development,
                    stage: ModelStage::ReflectFailure,
                    episode_id: $world.into(),
                    step: $step,
                    attempt: 1,
                    parent_skill_digest: skill_snapshot_digest(&$parent).unwrap(),
                    bundle_digest: $parent_bundle.clone(),
                    source_closure: allowed.clone(),
                    model_digest: hash(b"model"),
                    tools_digest: hash(b"tools"),
                    rules_digest: hash(b"rules"),
                    sampling_digest: hash(b"sampling"),
                    revoke_watermark: 1,
                    max_suggestions: 4,
                },
                parent_skill: &$parent,
                edit_context: &$edit_context,
                edit_batch_template: SkillEditBatch {
                    schema_version: SKILL_EDIT_SCHEMA.into(),
                    compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
                    namespace: "n".into(),
                    profile_id: "profile".into(),
                    skill_id: "skill".into(),
                    skill_version: "v1".into(),
                    input_digest: skill_snapshot_digest(&$parent).unwrap(),
                    approved_parent_digest: hash(b"approved-parent"),
                    safe_baseline_digest: hash(b"baseline"),
                    evidence: EvidenceClosure {
                        support: vec![],
                        counterexamples: vec![],
                        dependencies: vec![],
                    },
                    edits: vec![],
                },
                protected_ranges: &[],
                bundle_context: BundleCompileContext {
                    profile: &profile,
                    baseline: &baseline,
                    parent_strategy: &parent_strategy,
                    baseline_strategy: &baseline_strategy,
                    improver_patch: &improver,
                    caps: &caps,
                    revoked: &revoked,
                },
                development_request: DevelopmentRunRequest {
                    request_id: $dev_id.into(),
                    namespace: "n".into(),
                    purpose: Purpose::Development,
                    episode_id: $world.into(),
                    step: $step,
                    attempt: 1,
                    manifest: DevelopmentManifest::build(
                        format!("manifest-{}", $step),
                        vec![DevelopmentTask {
                            id: "task".into(),
                            parent_family: "family-a".into(),
                            input_digest: hash(b"task"),
                        }],
                    )
                    .unwrap(),
                    parent_bundle_digest: $parent_bundle.clone(),
                    candidate_bundle_digest: hash(format!("candidate-{0}", $step).as_bytes()),
                    environment_digest: hash(b"environment"),
                    grader_digest: hash(b"grader"),
                    rules_digest: hash(b"rules"),
                    tools_digest: hash(b"tools"),
                    revoke_watermark: 1,
                    idempotency_key: $idempotency.into(),
                },
                allow_rank_call: false,
            }
        };
    }
    let candidate_model = CountingEditingFixtureModel::default();
    let first = coordinator
        .run_next(
            Some(&candidate_model),
            Some(&ImprovingFixtureRunner),
            Some(&journal),
            request!(
                "world-1",
                1,
                "optimizer-request-1",
                "dev-request-1",
                "dev-idempotency-1",
                parent,
                edit_context,
                initial_parent_bundle
            ),
        )
        .await
        .unwrap();
    assert_eq!(first.outcome, "candidate_observed");
    assert_eq!(candidate_model.0.load(Ordering::SeqCst), 2);
    let reconnected = coordinator
        .run_next(
            None,
            None,
            None,
            request!(
                "world-1",
                1,
                "optimizer-request-1",
                "dev-request-1",
                "dev-idempotency-1",
                parent,
                edit_context,
                initial_parent_bundle
            ),
        )
        .await
        .unwrap();
    assert_eq!(reconnected.node_id, first.node_id);
    assert_eq!(reconnected.dispatch_id, first.dispatch_id);
    assert_eq!(candidate_model.0.load(Ordering::SeqCst), 2);
    let mut changed_reconnect = request!(
        "world-1",
        1,
        "optimizer-request-1",
        "dev-request-1",
        "dev-idempotency-1",
        parent,
        edit_context,
        initial_parent_bundle
    );
    changed_reconnect.allow_rank_call = true;
    assert!(matches!(
        coordinator
            .run_next(None, None, None, changed_reconnect)
            .await,
        Err(Error::Conflict(_))
    ));
    let selected = coordinator
        .final_candidate_request("world-1", first.node_id.as_deref().unwrap())
        .await
        .unwrap();
    let second_parent_bundle = selected.selected_bundle_digest;
    let mut evolved_parent = parent.clone();
    evolved_parent.content.push_str(" [repair]");
    evolved_parent.applicability.push_str(" [repair]");
    let edit_context_2 = TrustedEditContext::new(
        "n",
        "profile",
        "skill",
        "v1",
        hash(b"approved-parent"),
        hash(b"baseline"),
        &evolved_parent,
        allowed.clone(),
    )
    .unwrap();
    let second = coordinator
        .run_next(
            Some(&candidate_model),
            Some(&ImprovingFixtureRunner),
            Some(&journal),
            request!(
                "world-1",
                2,
                "optimizer-request-2",
                "dev-request-2",
                "dev-idempotency-2",
                evolved_parent,
                edit_context_2,
                second_parent_bundle
            ),
        )
        .await
        .unwrap();
    assert_eq!(second.outcome, "candidate_observed");
    assert_ne!(first.node_id, second.node_id);
    let next = coordinator.decide_next("world-1").await.unwrap();
    assert!(matches!(
        next.action,
        BatchActionV1::Dispatch {
            ref action_seqs, ..
        } if action_seqs == &[2]
    ));
    let mut other_world = world();
    other_world.id = "world-2".into();
    coordinator.register_world(other_world).await.unwrap();
    assert!(
        coordinator
            .final_candidate_request("world-2", first.node_id.as_deref().unwrap())
            .await
            .is_err()
    );

    let mut claimed_world = world();
    claimed_world.id = "world-claimed".into();
    claimed_world.parent_skill_digest = skill_snapshot_digest(&parent).unwrap();
    claimed_world.context_signature = evo_core::fingerprint(&(
        &claimed_world.parent_skill_digest,
        &claimed_world.parent_bundle_digest,
        &claimed_world.environment_digest,
        &claimed_world.model_digest,
        &claimed_world.tools_digest,
        &claimed_world.grader_digest,
        &claimed_world.rules_digest,
        claimed_world.source_watermark,
    ))
    .unwrap();
    coordinator.register_world(claimed_world).await.unwrap();
    let missing_port = coordinator
        .run_next(
            None,
            Some(&UnusedRunner),
            Some(&journal),
            request!(
                "world-claimed",
                1,
                "claimed-request",
                "claimed-dev",
                "claimed-idempotency",
                parent,
                edit_context,
                initial_parent_bundle
            ),
        )
        .await;
    assert!(matches!(missing_port, Err(Error::NotFound)));
    assert!(matches!(
        coordinator
            .decide_next("world-claimed")
            .await
            .unwrap()
            .action,
        BatchActionV1::Dispatch {
            ref action_seqs, ..
        } if action_seqs == &[1]
    ));

    let resumed = PersistentCoordinator::new(
        _store.clone(),
        Context::new("n", "worker", Role::Worker).unwrap(),
        "worker",
    )
    .unwrap();
    let resumed_journal = StoreOptimizationJournal::new(
        _store.clone(),
        Context::new("n", "worker", Role::Worker).unwrap(),
        "worker",
    )
    .unwrap();
    let model = CountingNoChangeModel::default();
    let recovered = resumed
        .run_next(
            Some(&model),
            Some(&UnusedRunner),
            Some(&resumed_journal),
            request!(
                "world-claimed",
                1,
                "claimed-request",
                "claimed-dev",
                "claimed-idempotency",
                parent,
                edit_context,
                initial_parent_bundle
            ),
        )
        .await
        .unwrap();
    assert!(recovered.node_id.is_some());
    assert_eq!(model.0.load(Ordering::SeqCst), 2);

    let mut crash_world = world();
    crash_world.id = "world-crash".into();
    crash_world.parent_skill_digest = skill_snapshot_digest(&parent).unwrap();
    crash_world.context_signature = evo_core::fingerprint(&(
        &crash_world.parent_skill_digest,
        &crash_world.parent_bundle_digest,
        &crash_world.environment_digest,
        &crash_world.model_digest,
        &crash_world.tools_digest,
        &crash_world.grader_digest,
        &crash_world.rules_digest,
        crash_world.source_watermark,
    ))
    .unwrap();
    resumed.register_world(crash_world).await.unwrap();
    let crashing_journal = CrashAfterDispatchJournal {
        inner: StoreOptimizationJournal::new(
            _store.clone(),
            Context::new("n", "worker", Role::Worker).unwrap(),
            "worker",
        )
        .unwrap(),
        failed: AtomicBool::new(false),
    };
    let crash_model = CountingNoChangeModel::default();
    let uncertain = resumed
        .run_next(
            Some(&crash_model),
            Some(&UnusedRunner),
            Some(&crashing_journal),
            request!(
                "world-crash",
                1,
                "crash-request",
                "crash-dev",
                "crash-idempotency",
                parent,
                edit_context,
                initial_parent_bundle
            ),
        )
        .await
        .unwrap();
    assert!(uncertain.outcome.contains("outcome uncertain"));
    assert!(uncertain.node_id.is_some());
    assert_eq!(crash_model.0.load(Ordering::SeqCst), 2);
    assert!(matches!(
        resumed.decide_next("world-crash").await.unwrap().action,
        BatchActionV1::Dispatch {
            ref action_seqs, ..
        } if action_seqs == &[2]
    ));
    let crash_reconnect = resumed
        .run_next(
            Some(&crash_model),
            Some(&UnusedRunner),
            Some(&crashing_journal),
            request!(
                "world-crash",
                1,
                "crash-request",
                "crash-dev",
                "crash-idempotency",
                parent,
                edit_context,
                initial_parent_bundle
            ),
        )
        .await
        .unwrap();
    assert_eq!(crash_reconnect.node_id, uncertain.node_id);
    assert_eq!(crash_model.0.load(Ordering::SeqCst), 2);

    let mut not_found_world = world();
    not_found_world.id = "world-model-not-found".into();
    not_found_world.parent_skill_digest = skill_snapshot_digest(&parent).unwrap();
    not_found_world.context_signature = evo_core::fingerprint(&(
        &not_found_world.parent_skill_digest,
        &not_found_world.parent_bundle_digest,
        &not_found_world.environment_digest,
        &not_found_world.model_digest,
        &not_found_world.tools_digest,
        &not_found_world.grader_digest,
        &not_found_world.rules_digest,
        not_found_world.source_watermark,
    ))
    .unwrap();
    resumed.register_world(not_found_world).await.unwrap();
    let not_found_model = NotFoundAfterDispatchModel::default();
    let not_found = resumed
        .run_next(
            Some(&not_found_model),
            Some(&UnusedRunner),
            Some(&resumed_journal),
            request!(
                "world-model-not-found",
                1,
                "not-found-request",
                "not-found-dev",
                "not-found-idempotency",
                parent,
                edit_context,
                initial_parent_bundle
            ),
        )
        .await
        .unwrap();
    assert!(not_found.outcome.contains("not found"));
    assert!(not_found.node_id.is_some());
    assert_eq!(not_found_model.0.load(Ordering::SeqCst), 1);
    let not_found_reconnect = resumed
        .run_next(
            None,
            None,
            None,
            request!(
                "world-model-not-found",
                1,
                "not-found-request",
                "not-found-dev",
                "not-found-idempotency",
                parent,
                edit_context,
                initial_parent_bundle
            ),
        )
        .await
        .unwrap();
    assert_eq!(not_found_reconnect.node_id, not_found.node_id);
    assert_eq!(not_found_model.0.load(Ordering::SeqCst), 1);
    assert!(matches!(
        resumed
            .decide_next("world-model-not-found")
            .await
            .unwrap()
            .action,
        BatchActionV1::Dispatch {
            ref action_seqs, ..
        } if action_seqs == &[2]
    ));

    let mut binding_world = world();
    binding_world.id = "world-binding".into();
    binding_world.parent_skill_digest = skill_snapshot_digest(&parent).unwrap();
    binding_world.context_signature = evo_core::fingerprint(&(
        &binding_world.parent_skill_digest,
        &binding_world.parent_bundle_digest,
        &binding_world.environment_digest,
        &binding_world.model_digest,
        &binding_world.tools_digest,
        &binding_world.grader_digest,
        &binding_world.rules_digest,
        binding_world.source_watermark,
    ))
    .unwrap();
    resumed.register_world(binding_world).await.unwrap();
    let mut wrong_parent = request!(
        "world-binding",
        1,
        "binding-request",
        "binding-dev",
        "binding-idempotency",
        parent,
        edit_context,
        initial_parent_bundle
    );
    wrong_parent.development_request.parent_bundle_digest = hash(b"wrong-parent");
    assert!(matches!(
        resumed
            .run_next(
                Some(&NoChangeModel),
                Some(&UnusedRunner),
                Some(&resumed_journal),
                wrong_parent,
            )
            .await,
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        resumed.decide_next("world-binding").await.unwrap().action,
        BatchActionV1::Dispatch {
            ref action_seqs, ..
        } if action_seqs == &[1]
    ));
}

fn history(sequence: u64, development_only: bool) -> OptimizationHistoryEntry {
    OptimizationHistoryEntry {
        entry_id: format!("history-{sequence}"),
        sequence,
        parent_digest: hash(b"approved-parent"),
        environment_digest: hash(b"env"),
        task_family: "family".into(),
        source_watermark: 1,
        input_digest: hash(b"input"),
        patch_digest: hash(b"patch"),
        evidence_digest: hash(b"evidence"),
        outcome: HistoryOutcome::NoChange,
        deterministic_error: true,
        data_use: if development_only {
            DataUse::Development
        } else {
            DataUse::AcceptanceEpoch
        },
        summary: format!("development result {sequence}"),
    }
}

#[tokio::test]
async fn history_is_persisted_bounded_and_hidden_feedback_is_rejected() {
    let (_dir, coordinator, store) = coordinator().await;
    coordinator.register_world(world()).await.unwrap();
    for sequence in 1..=10 {
        coordinator
            .record_history("world-1", history(sequence, true))
            .await
            .unwrap();
    }
    assert!(
        coordinator
            .record_history("world-1", history(11, false))
            .await
            .is_err()
    );
    let selected = coordinator
        .matching_history(
            "world-1",
            HistoryQuery {
                parent_digest: &hash(b"approved-parent"),
                environment_digest: &hash(b"env"),
                task_family: "family",
                source_watermark: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(selected.len(), 8);
    assert_eq!(selected[0].sequence, 3);

    let host = Context::new("n", "host", Role::Host).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&host, "source-revoked-after-world-freeze")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(coordinator.decide_next("world-1").await.is_err());
    assert!(
        coordinator
            .record_history("world-1", history(12, true))
            .await
            .is_err()
    );
    assert!(
        coordinator
            .matching_history(
                "world-1",
                HistoryQuery {
                    parent_digest: &hash(b"approved-parent"),
                    environment_digest: &hash(b"env"),
                    task_family: "family",
                    source_watermark: 1,
                },
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn intermediate_world_cannot_be_presented_as_final_candidate() {
    let (_dir, coordinator, _store) = coordinator().await;
    coordinator.register_world(world()).await.unwrap();
    assert!(
        coordinator
            .final_candidate_request("world-1", "missing-node")
            .await
            .is_err()
    );
}
