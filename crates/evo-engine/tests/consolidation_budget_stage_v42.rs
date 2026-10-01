//! E13 / V096.a (plan v4.2, AG-030): the model calls of a consolidation run are
//! metered under their own `Consolidation` budget stage while they stay inside the
//! claim's root budget (plan §3.6, §11.5).
//!
//! * `ModelStage::Consolidate` maps to `BudgetStage::Consolidation` at the broker.
//! * A step whose base stage is `Consolidate` makes all of its model calls (the
//!   reflection calls and the rank call) under that stage; any other base stage
//!   keeps the old reflection / ranking mapping.
//! * Only `run_consolidation` may run a `Consolidate` step, and it runs nothing
//!   else; the ordinary entry refuses the stage.
//! * The five V096.a injections (timeout, cancellation, invalid JSON, missing cost,
//!   late result) go through the real broker and ledger under the consolidation
//!   stage: a dispatched call's cost is kept and never re-dispatched.
//! * The old stage labels, request digests and journal ids do not move.
//!
//! The fixtures are copied from `tests/monitoring_consolidation_v42.rs` (environment,
//! cycles, claims, step material, the ledger development runner) because test crates
//! cannot import one another; the transport is scripted instead of a model port so
//! every model call reaches the real `PersistentModelBroker` and the real ledger.

use async_trait::async_trait;
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, ImproverPatch, Profile, SkillSnapshot,
    SurfaceCoverage, SurfaceItem, SystemSnapshot,
};
use evo_core::evidence::{EvidenceSet, ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::optimization::{
    EditSuggestion, ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
    OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::broker::{
    BrokerConfig, BudgetPortBinding, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, ingest_trusted_run_records, store_source_selection,
    store_trace_authority,
};
use evo_engine::model::{ModelExecutionProvenance, ModelPort, ModelResponse};
use evo_engine::monitoring::{
    ClaimOutcome, CompleteDevelopmentCycleRequest, ConsolidationClaim, ConsolidationClaimState,
    ConsolidationDevRunner, ConsolidationRunOutcome, ConsolidationRunRecord,
    DevelopmentCycleRecord, EnvironmentEvidenceScope, MonitoringCoordinator,
    RecordEnvironmentRequest, RecordedEnvironment,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationJournalStage, OptimizationStepOutcome,
    OptimizationStepRequest, PairedTaskResult, StageDependency, StageFact, StageFactKind,
    StoreOptimizationJournal, run_optimization_step,
};
use evo_engine::release_store::{PrepareRunRequest, ReleaseStore, TrustedHostExecutionEvidence};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetCallState, BudgetStage,
    RootBudgetAuthorization, RootBudgetRecord, UsageCharge,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TENANT: &str = "tenant";
const SCOPE: &str = "billing-scope";
const ROOT: &str = "root-budget";
/// What a fixture model call costs, and what the broker reserves for it.
const COST_MICROS: i64 = 10;
const RESERVED_MICROS: i64 = 20;

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn context(actor: &str, role: Role) -> Context {
    Context::new(TENANT, actor, role).unwrap()
}

// ---------------------------------------------------------------------------
// Pinned digests: nothing issued before the new stage may move.
// ---------------------------------------------------------------------------

/// `input_digest` of the golden request: the stage is not part of it.
const GOLDEN_INPUT_DIGEST: &str =
    "52098db6579e98739c4f42d67948c9c22c8870832b7800ff1b0cf8f9ee7c036e";
/// Golden `cache_key_digest` per stage, taken from the code before `Consolidate`
/// existed (the last entry is the new stage and is pinned from its first issue).
const GOLDEN_CACHE_KEYS: [(ModelStage, &str, &str); 5] = [
    (
        ModelStage::ReflectFailure,
        "reflect_failure",
        "72085c8d70703a5027ea954185184b1702b5a4429d9628f35c5a1603be88bd6d",
    ),
    (
        ModelStage::ReflectSuccess,
        "reflect_success",
        "9e9b0ac373770352e8146e0a125226b1c17b76def13af714315de7c49177a135",
    ),
    (
        ModelStage::Merge,
        "merge",
        "0da7ac633fc9eb383d4616cdcf72780a3469746099bc48edde2dc0a78e37adea",
    ),
    (
        ModelStage::Rank,
        "rank",
        "32d6b7b9fd722188206f20da9f17f79bd6120646410a79dd64cc989423d890a4",
    ),
    (
        ModelStage::Consolidate,
        "consolidate",
        "8814a01af970f1d1a8d53df6ff8184ec50424941028aeed89ef755507bcca7a0",
    ),
];
/// Golden canonical id of a `RequestPrepared` stage fact per journal stage, and the
/// `StepCompleted` fact under `Merge` that a step's terminal record is keyed by.
const GOLDEN_FACT_IDS: [(OptimizationJournalStage, &str, &str); 7] = [
    (
        OptimizationJournalStage::ReflectFailure,
        "reflect_failure",
        "optstage-53c7d7eb2b9dbbc46c84da659e6b1d87d3ba5fd1038886a3c5f8a9fa86eaeb2d",
    ),
    (
        OptimizationJournalStage::ReflectSuccess,
        "reflect_success",
        "optstage-56d029cae2f0656aa68ec903f2dc8e944443285f9208a0f3a5b70ce8216e870a",
    ),
    (
        OptimizationJournalStage::Merge,
        "merge",
        "optstage-d7d8c995f27b7d1cbf781e6b4d50947663938235b16c139e5126619808efc1b6",
    ),
    (
        OptimizationJournalStage::Rank,
        "rank",
        "optstage-cd7f9f9ac7415084ce43758f1fb0e715bf10d1d18e4e53e399ad84e597f53d96",
    ),
    (
        OptimizationJournalStage::EditCompile,
        "edit_compile",
        "optstage-7c83edb250a1bde41cee569b96f466052f49a3346c0237da4a68f1abbec45ab3",
    ),
    (
        OptimizationJournalStage::Development,
        "development",
        "optstage-8a5740e60bba07eeee976d0e03a766f3806c995cf016c8d003671e56bdc6f8ae",
    ),
    (
        OptimizationJournalStage::Consolidate,
        "consolidate",
        "optstage-92b3240d5ee2aedd0449e02850e91b0a3b2181f813b9d007ae3ba9831a0a750e",
    ),
];
const GOLDEN_STEP_COMPLETED_MERGE_ID: &str =
    "optstage-ef5483f62a203e8933c13a978d1dc85356402d2bb3cc2d332b1067ec1c8526d4";
/// Digest of the golden fact's payload: the same for every stage.
const GOLDEN_PAYLOAD_DIGEST: &str =
    "1948a2de2094aaeb62c64398c666b846a36df279a5ef8d9e3f75b8d95526bfd3";

fn golden_request(stage: ModelStage) -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: "golden-request".into(),
            namespace: "golden-namespace".into(),
            purpose: Purpose::Development,
            stage,
            episode_id: "golden-episode".into(),
            step: 3,
            attempt: 2,
            parent_skill_digest: hash(b"golden-parent"),
            bundle_digest: hash(b"golden-bundle"),
            source_closure: vec![EvidenceRef {
                id: "golden-source".into(),
                digest: hash(b"golden-source"),
            }],
            model_digest: hash(b"golden-model"),
            tools_digest: hash(b"golden-tools"),
            rules_digest: hash(b"golden-rules"),
            sampling_digest: hash(b"golden-sampling"),
            revoke_watermark: 7,
            max_suggestions: 4,
        },
        vec![ModelInputPart {
            role: ModelInputRole::User,
            label: "golden-input".into(),
            content: "golden content".into(),
        }],
    )
    .unwrap()
}

fn golden_fact(stage: OptimizationJournalStage, kind: StageFactKind) -> StageFact {
    StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: "golden-namespace".into(),
        episode_id: "golden-episode".into(),
        step: 3,
        attempt: 2,
        stage,
        kind,
        request_id: "golden-request".into(),
        input_digest: hash(b"golden-input-digest"),
        output_digest: None,
        dependencies: vec![],
        payload: json!({"golden": true}),
    }
    .seal()
    .unwrap()
}

#[test]
fn the_old_model_stage_labels_and_request_digests_do_not_move() {
    for (stage, label, cache_key) in GOLDEN_CACHE_KEYS {
        assert_eq!(serde_json::to_value(stage).unwrap(), json!(label));
        let request = golden_request(stage);
        assert_eq!(request.input_digest, GOLDEN_INPUT_DIGEST, "{label}");
        assert_eq!(request.cache_key_digest, cache_key, "{label}");
        request.validate().unwrap();
        // The wire form round-trips to the same stage and the same digests.
        let wire = serde_json::to_string(&request).unwrap();
        let restored: ModelRequest = serde_json::from_str(&wire).unwrap();
        assert_eq!(restored.stage, stage);
        restored.validate().unwrap();
    }
    // The stage label is part of the cache key: five stages, five distinct keys.
    let keys: BTreeSet<_> = GOLDEN_CACHE_KEYS.iter().map(|entry| entry.2).collect();
    assert_eq!(keys.len(), GOLDEN_CACHE_KEYS.len());
    // A request relabelled after it was built no longer validates.
    let mut relabelled = golden_request(ModelStage::Consolidate);
    relabelled.stage = ModelStage::Rank;
    assert!(matches!(relabelled.validate(), Err(Error::Conflict(_))));
}

#[test]
fn the_old_journal_stage_labels_and_canonical_ids_do_not_move() {
    for (stage, label, id) in GOLDEN_FACT_IDS {
        assert_eq!(serde_json::to_value(stage).unwrap(), json!(label));
        let fact = golden_fact(stage, StageFactKind::RequestPrepared);
        assert_eq!(fact.artifact_id, id, "{label}");
        assert_eq!(
            fact.output_digest.as_deref(),
            Some(GOLDEN_PAYLOAD_DIGEST),
            "{label}"
        );
        fact.validate().unwrap();
        let wire = serde_json::to_string(&fact).unwrap();
        let restored: StageFact = serde_json::from_str(&wire).unwrap();
        assert_eq!(restored.stage, stage);
        restored.validate().unwrap();
    }
    // The id is keyed by the stage: seven stages, seven distinct ids.
    let ids: BTreeSet<_> = GOLDEN_FACT_IDS.iter().map(|entry| entry.2).collect();
    assert_eq!(ids.len(), GOLDEN_FACT_IDS.len());
    // A step's own terminal fact stays under `Merge`: the consolidation terminal
    // fact id that `run_consolidation` derives depends on it.
    assert_eq!(
        golden_fact(
            OptimizationJournalStage::Merge,
            StageFactKind::StepCompleted
        )
        .artifact_id,
        GOLDEN_STEP_COMPLETED_MERGE_ID
    );
}

// ---------------------------------------------------------------------------
// Broker: the stage mapping, in the style of tests/broker.rs.
// ---------------------------------------------------------------------------

/// A transport that answers every request with a fixed, valid completion.
struct FixedTransport;

#[async_trait]
impl ModelTransport for FixedTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        Ok(completion(request, "fixture output".into()))
    }
}

fn completion(request: &ModelRequest, output: String) -> TransportCompletion {
    TransportCompletion {
        response_id: format!("response-{}", request.request_id),
        provider_request_id: format!("provider-{}", request.request_id),
        usage_record_id: format!("usage-{}", request.request_id),
        actual_model_digest: request.model_digest.clone(),
        output,
        actual_cost_micros: COST_MICROS,
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
    }
}

fn broker_config(lease_seconds: i64) -> BrokerConfig {
    BrokerConfig {
        billing_scope: SCOPE.into(),
        actor: "trusted-broker".into(),
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
        max_cost_micros: RESERVED_MICROS,
        lease_seconds,
    }
}

async fn authorize_root(store: &Store, admin: &Context) -> RootBudgetRecord {
    store
        .authorize_root_budget(
            admin,
            &RootBudgetAuthorization {
                root_budget_id: ROOT.into(),
                billing_scope: SCOPE.into(),
                allowed_namespaces: vec![TENANT.into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "payer".into(),
                authorization_receipt_digest: d("authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap()
}

fn plain_request(id: &str, stage: ModelStage) -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: id.into(),
            namespace: TENANT.into(),
            purpose: Purpose::Development,
            stage,
            episode_id: "episode-1".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: hash(b"parent-v1"),
            bundle_digest: hash(b"bundle-v1"),
            source_closure: vec![EvidenceRef {
                id: "evidence-1".into(),
                digest: hash(b"evidence"),
            }],
            model_digest: hash(b"model-v1"),
            tools_digest: hash(b"tools-v1"),
            rules_digest: hash(b"rules-v1"),
            sampling_digest: hash(b"sampling-v1"),
            revoke_watermark: 0,
            max_suggestions: 4,
        },
        vec![ModelInputPart {
            role: ModelInputRole::User,
            label: "task".into(),
            content: "analyze the development evidence".into(),
        }],
    )
    .unwrap()
}

#[tokio::test]
async fn the_broker_reserves_a_consolidate_request_under_the_consolidation_stage() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("rsia.sqlite3"))
        .await
        .unwrap();
    let admin = context("admin", Role::Admin);
    authorize_root(&store, &admin).await;
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        FixedTransport,
        broker_config(60),
        Arc::new(|| 10),
    )
    .unwrap();
    // Each model stage, the ledger stage it is reserved under and that stage's label.
    let stages = [
        (
            ModelStage::ReflectFailure,
            BudgetStage::Reflection,
            "reflection",
        ),
        (
            ModelStage::ReflectSuccess,
            BudgetStage::Reflection,
            "reflection",
        ),
        (ModelStage::Merge, BudgetStage::Merge, "merge"),
        (ModelStage::Rank, BudgetStage::Ranking, "ranking"),
        (
            ModelStage::Consolidate,
            BudgetStage::Consolidation,
            "consolidation",
        ),
    ];
    let worker = context("worker", Role::Worker);
    for (index, (stage, expected, label)) in stages.into_iter().enumerate() {
        let request = plain_request(&format!("stage-call-{index}"), stage);
        assert!(matches!(
            broker.dispatch(request.clone()).await.unwrap(),
            ModelResponse::Completed { .. }
        ));
        let call = store
            .budget_call(&worker, SCOPE, &request.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(call.stage, expected, "{stage:?}");
        assert_eq!(call.stage.as_str(), label, "{stage:?}");
        assert_eq!(call.state, BudgetCallState::Finalized);
        assert_eq!(call.actual_cost_micros, Some(COST_MICROS));
        assert_eq!(call.dispatch_group_id, "episode-1");
    }
    // A consolidation call is one more spend from the one root, not a second one.
    let root = store.root_budget(&worker, SCOPE).await.unwrap().unwrap();
    assert_eq!(root.root_budget_id, ROOT);
    assert_eq!(root.spent_micros, COST_MICROS * 5);
    assert_eq!(root.reserved_micros, 0);
}

// ---------------------------------------------------------------------------
// Fixtures copied from tests/monitoring_consolidation_v42.rs
// ---------------------------------------------------------------------------

fn surface() -> HostSurfaceManifest {
    HostSurfaceManifest {
        schema_version: "rsia.host_surface.v1".into(),
        host: "monitor-host".into(),
        host_version: "1.0.0".into(),
        adapter_version: "adapter-v1".into(),
        source_digest: d("surface-source"),
        items: vec![SurfaceItem {
            name: "model".into(),
            coverage: SurfaceCoverage::Supported,
            mapped_field: Some("host.model".into()),
            consumer: Some("runner".into()),
            reason: "fixture projection".into(),
        }],
    }
}

fn system(model: &str) -> SystemSnapshot {
    SystemSnapshot {
        schema_version: "rsia.system_snapshot.v2".into(),
        profile_id: "profile".into(),
        host_id: "monitor-host".into(),
        host_version: "1.0.0".into(),
        model_id: model.into(),
        tools: vec!["tool-a".into()],
        mandatory_context_digest: d("mandatory"),
    }
}

/// A trusted Host development source. `source-1` is a diagnosed failure and
/// `source-2` a success, so the optimizer has a failure and a success batch.
fn authority(id: &str, body: &str) -> StoredTraceAuthority {
    let failing = id == "source-1";
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.as_bytes().to_vec(),
            parent_family: format!("family-{id}"),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: format!("family-{id}"),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: if failing {
                TraceOutcome::TaskFailure
            } else {
                TraceOutcome::Success
            },
            diagnosis: failing.then(|| SkillFailureDiagnosis {
                kind: SkillFailureKind::SkillDefect,
                skill_id: "skill-a".into(),
                bundle_digest: d("parent-bundle"),
                request_digest: d("task-request"),
                rule_id: Some("rule-a".into()),
                support: vec![EvidenceRef {
                    id: id.into(),
                    digest: hash(body.as_bytes()),
                }],
                counterexamples: vec![],
                reason: "fixture defect".into(),
            }),
            excerpt: body.into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    host: Context,
    worker: Context,
    environment: RecordedEnvironment,
    authorities: Vec<StoredTraceAuthority>,
    manifest: DevelopmentManifest,
    parent_skill: SkillSnapshot,
    manifest_digest: String,
    grader_digest: String,
    strategy_digest: String,
    parent_skill_digest: String,
    parent_bundle_digest: String,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("consolidation-stage.sqlite3"))
            .await
            .unwrap();
        let admin = context("admin", Role::Admin);
        let host = context("host", Role::Host);
        let worker = context("worker", Role::Worker);
        ReleaseStore::register_host_surface(
            &admin,
            &store,
            "surface",
            surface(),
            vec!["model".into()],
        )
        .await
        .unwrap();
        prepare_host_run(&host, &store, "host-run-1", "model-v1", "task-input").await;
        let authorities = vec![
            authority("source-1", "first development source"),
            authority("source-2", "second development source"),
        ];
        for source in &authorities {
            store_trace_authority(&store, &host, source).await.unwrap();
        }
        let mut session = store.session().await.unwrap();
        assert_eq!(
            session
                .bump_watermark(&host, &d("initial-watermark"))
                .await
                .unwrap(),
            1
        );
        session.commit().await.unwrap();
        let manifest = DevelopmentManifest::build(
            "development-manifest",
            vec![DevelopmentTask {
                id: "task-a".into(),
                parent_family: "development-family".into(),
                input_digest: d("development-task-input"),
            }],
        )
        .unwrap();
        let manifest_digest = manifest.digest.clone();
        let grader_digest = d("development-grader");
        let strategy_digest = fingerprint(&Strategy::default()).unwrap();
        let parent_skill = SkillSnapshot {
            content: "parent rule".into(),
            applicability: "development only".into(),
            counterexample: "counterexample".into(),
            required_capabilities: vec![],
            dependencies: vec![],
        };
        let environment = MonitoringCoordinator::record_environment_from_run(
            &host,
            &store,
            RecordEnvironmentRequest {
                run_id: "host-run-1".into(),
                development_manifest_digest: manifest_digest.clone(),
                grader_digest: grader_digest.clone(),
                generation_strategy_digest: strategy_digest.clone(),
                revoke_watermark: 1,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            environment.environment.evidence_scope,
            EnvironmentEvidenceScope::ProgramFixture
        );
        authorize_root(&store, &admin).await;
        Self {
            _dir: dir,
            store,
            host,
            worker,
            environment,
            authorities,
            manifest,
            parent_skill: parent_skill.clone(),
            manifest_digest,
            grader_digest,
            strategy_digest,
            parent_skill_digest: skill_snapshot_digest(&parent_skill).unwrap(),
            parent_bundle_digest: d("parent-bundle"),
        }
    }

    fn cycle_request(&self, fact_id: String) -> CompleteDevelopmentCycleRequest {
        CompleteDevelopmentCycleRequest {
            skill_id: "skill-a".into(),
            profile_id: "profile".into(),
            environment_id: self.environment.environment.id.clone(),
            development_manifest_digest: self.manifest_digest.clone(),
            grader_digest: self.grader_digest.clone(),
            generation_strategy_digest: self.strategy_digest.clone(),
            parent_skill_digest: self.parent_skill_digest.clone(),
            parent_bundle_digest: self.parent_bundle_digest.clone(),
            billing_scope: SCOPE.into(),
            root_budget_id: ROOT.into(),
            report_fact_id: fact_id,
            source_ids: vec!["source-2".into(), "source-1".into()],
            revoke_watermark: 1,
        }
    }

    /// Stages a development report for `cycle` and closes it as a cycle.
    async fn close_cycle(&self, cycle: u32) -> Result<DevelopmentCycleRecord> {
        let fact = self.stage_report(cycle).await;
        MonitoringCoordinator::close_development_cycle(
            &self.worker,
            &self.store,
            self.cycle_request(fact),
        )
        .await
    }

    /// Closes two cycles with a contrast and claims the first generation.
    async fn claimed(&self) -> ConsolidationClaim {
        for cycle in 1..=2 {
            self.close_cycle(cycle).await.unwrap();
        }
        let scope_id = self.scope_id().await;
        let ClaimOutcome::Claimed(claim) =
            MonitoringCoordinator::claim_consolidation(&self.worker, &self.store, &scope_id)
                .await
                .unwrap()
        else {
            panic!("the next consolidation generation was not claimed")
        };
        claim
    }

    async fn scope_id(&self) -> String {
        let mut session = self.store.session().await.unwrap();
        let scopes: Vec<Value> = session.list(&self.worker, "artifact").await.unwrap();
        session.commit().await.unwrap();
        let ids: Vec<_> = scopes
            .iter()
            .filter(|value| value["schema_version"] == "rsia.monitoring.consolidation_scope.v1")
            .map(|value| value["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids.len(), 1, "exactly one consolidation scope");
        ids[0].clone()
    }

    /// Stages a DevelopmentObserved fact whose report pairs one task for `cycle`
    /// (score 500k -> 600k, both pass: a contrast made of a score change), with
    /// the fixture receipts the stage fact depends on.
    async fn stage_report(&self, cycle: u32) -> String {
        let runner_execution = format!("runner-execution-{cycle}");
        let parent_execution = format!("parent-execution-{cycle}-0");
        let candidate_execution = format!("candidate-execution-{cycle}-0");
        let grader_receipt = d(&format!("grader-receipt-{cycle}-0"));
        let receipts = [
            ("execution", runner_execution.clone()),
            ("execution", parent_execution.clone()),
            ("execution", candidate_execution.clone()),
            ("grader", grader_receipt.clone()),
        ];
        let mut dependencies = vec![
            StageDependency {
                kind: "execution".into(),
                id: parent_execution.clone(),
            },
            StageDependency {
                kind: "execution".into(),
                id: candidate_execution.clone(),
            },
            StageDependency {
                kind: "grader".into(),
                id: grader_receipt.clone(),
            },
        ];
        let report = DevelopmentRunReport {
            request_id: format!("development-request-{cycle}"),
            manifest_digest: self.manifest_digest.clone(),
            parent_bundle_digest: self.parent_bundle_digest.clone(),
            candidate_bundle_digest: d(&format!("candidate-bundle-{cycle}")),
            environment_digest: self.environment.environment.environment_digest.clone(),
            grader_digest: self.grader_digest.clone(),
            results: vec![PairedTaskResult {
                task_id: "task-a".into(),
                parent_score_micros: 500_000,
                candidate_score_micros: 600_000,
                parent_passed: true,
                candidate_passed: true,
                parent_execution_id: parent_execution,
                candidate_execution_id: candidate_execution,
                grader_receipt_digest: grader_receipt,
            }],
            execution_receipt_id: runner_execution,
            usage_record_ids: vec![],
            provenance: DevelopmentExecutionProvenance::Fixture,
        };
        let mut session = self.store.session().await.unwrap();
        for (logical_kind, id) in &receipts {
            session
                .put(
                    &self.worker,
                    "artifact",
                    id,
                    self.worker.actor(),
                    &json!({"id":id,"schema_version":"rsia.development_receipt.fixture.v1","logical_kind":logical_kind,"cycle":cycle,"fixture":true}),
                )
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
        for source in ["source-1", "source-2"] {
            dependencies.push(StageDependency {
                kind: "run".into(),
                id: source.into(),
            });
        }
        dependencies.push(StageDependency {
            kind: "revoke_watermark".into(),
            id: "1".into(),
        });
        let fact = StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: "placeholder".into(),
            namespace: TENANT.into(),
            episode_id: format!("development-episode-{cycle}"),
            step: cycle,
            attempt: 1,
            stage: OptimizationJournalStage::Development,
            kind: StageFactKind::DevelopmentObserved,
            request_id: report.request_id.clone(),
            input_digest: d(&format!("development-input-{cycle}")),
            output_digest: None,
            dependencies,
            payload: serde_json::to_value(&report).unwrap(),
        }
        .seal()
        .unwrap();
        let id = fact.artifact_id.clone();
        self.journal().commit(fact).await.unwrap();
        id
    }

    fn journal(&self) -> StoreOptimizationJournal {
        StoreOptimizationJournal::new(self.store.clone(), self.worker.clone(), "worker").unwrap()
    }
}

async fn prepare_host_run(host: &Context, store: &Store, run_id: &str, model: &str, task: &str) {
    let snapshot = ReleaseStore::prepare_run(
        host,
        store,
        PrepareRunRequest {
            run_id: run_id.into(),
            profile_id: "profile".into(),
            system_snapshot: system(model),
            host_surface_id: "surface".into(),
            host_capabilities: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            task_input_digest: d(task),
            evolution_enabled: true,
            capability_level: CapabilityLevel::Attached,
        },
    )
    .await
    .unwrap();
    ReleaseStore::record_applied_request(
        host,
        store,
        run_id,
        TrustedHostExecutionEvidence {
            actual_request_material: snapshot.request_material.clone(),
            environment_digest: snapshot.environment_digest.clone(),
            host_surface_digest: snapshot.host_surface_digest.clone(),
            host_capabilities_digest: snapshot.host_capabilities_digest.clone(),
            offered: vec![],
            attached: vec![],
            used: vec![],
            execution_receipt_id: None,
            truncated: false,
        },
    )
    .await
    .unwrap();
}

/// Development runner that spends through the real root ledger under the
/// episode's dispatch group (as the root-binding wrapper requires) and reports a
/// strict improvement over the frozen manifest.
struct LedgerDevRunner {
    store: Store,
    worker: Context,
    calls: AtomicUsize,
}

impl LedgerDevRunner {
    fn new(fixture: &Fixture) -> Self {
        Self {
            store: fixture.store.clone(),
            worker: fixture.worker.clone(),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl DevRunner for LedgerDevRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let call_id = format!("dev-call-{}", request.request_id);
        let input = d(&request.request_id);
        let lease = format!("lease-{}", request.request_id);
        let call = self
            .store
            .reserve_budget_call(
                &self.worker,
                &BudgetCallReservation {
                    billing_scope: SCOPE.into(),
                    call_id: call_id.clone(),
                    dispatch_group_id: request.episode_id.clone(),
                    stage: BudgetStage::DevelopmentExecution,
                    actual_input_digest: input.clone(),
                    request_artifact: None,
                    max_cost_micros: 10,
                    lease_token: lease.clone(),
                    lease_until: 10_000,
                    now: 100,
                },
            )
            .await?;
        let fence = BudgetCallFence {
            billing_scope: SCOPE.into(),
            call_id,
            actual_input_digest: input,
            lease_token: lease,
            lease_epoch: call.lease_epoch,
            now: 101,
        };
        let dispatched = self
            .store
            .begin_budget_dispatch(&self.worker, &fence)
            .await?;
        let usage = format!("usage-{}", request.request_id);
        self.store
            .finalize_budget_call(
                &self.worker,
                &fence,
                &UsageCharge {
                    amount_micros: 10,
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    provider_request_id: "fixture-runner".into(),
                    usage_record_id: usage.clone(),
                    output_digest: d("development-output"),
                },
            )
            .await?;
        // The root admits one open execution at a time: close this one.
        self.store
            .close_budget_call_execution(
                &self.worker,
                SCOPE,
                &fence.call_id,
                dispatched.call.dispatch_id.as_deref().unwrap(),
                "fixture_execution_closed",
                102,
            )
            .await?;
        Ok(DevelopmentRunReport {
            request_id: request.request_id.clone(),
            manifest_digest: request.manifest.digest.clone(),
            parent_bundle_digest: request.parent_bundle_digest.clone(),
            candidate_bundle_digest: request.candidate_bundle_digest.clone(),
            environment_digest: request.environment_digest.clone(),
            grader_digest: request.grader_digest.clone(),
            results: request
                .manifest
                .tasks
                .iter()
                .map(|task| PairedTaskResult {
                    task_id: task.id.clone(),
                    parent_score_micros: 500_000,
                    candidate_score_micros: 700_000,
                    parent_passed: true,
                    candidate_passed: true,
                    parent_execution_id: format!("parent-{}", task.id),
                    candidate_execution_id: format!("candidate-{}", task.id),
                    grader_receipt_digest: d(&format!("grade-{}", task.id)),
                })
                .collect(),
            execution_receipt_id: "development-runner-execution".into(),
            usage_record_ids: vec![usage],
            provenance: DevelopmentExecutionProvenance::Fixture,
        })
    }
}

#[async_trait]
impl ConsolidationDevRunner for LedgerDevRunner {
    async fn trusted_budget_binding(&self, _namespace: &str) -> Result<BudgetPortBinding> {
        Ok(BudgetPortBinding {
            billing_scope: SCOPE.into(),
            root_budget_id: ROOT.into(),
        })
    }
}

/// Everything an `OptimizationStepRequest` borrows, frozen once per fixture.
struct StepMaterial {
    selection: SourceSelection,
    evidence: EvidenceSet,
    bindings: Vec<TrustedSourceBinding>,
    allowed: Vec<EvidenceRef>,
    edit_context: TrustedEditContext,
    edit_template: SkillEditBatch,
    profile: Profile,
    baseline: SkillSnapshot,
    strategy: Strategy,
    patch: ImproverPatch,
    caps: HostCapabilities,
    revoked: BTreeSet<String>,
}

/// What varies between the steps this file runs.
#[derive(Clone, Copy)]
struct Shape {
    /// The step's base stage (`model_context.stage`).
    stage: ModelStage,
    /// Whether the step carries traces; none leaves it without a reflection batch.
    traces: bool,
    /// Whether the step may make its preregistered rank call.
    rank: bool,
}

impl Shape {
    const CONSOLIDATION: Shape = Shape {
        stage: ModelStage::Consolidate,
        traces: true,
        rank: false,
    };

    fn at(stage: ModelStage) -> Self {
        Self {
            stage,
            ..Self::CONSOLIDATION
        }
    }
}

impl StepMaterial {
    async fn new(fixture: &Fixture) -> Self {
        let selection = SourceSelection {
            roots: vec![],
            run_ids: vec!["source-1".into(), "source-2".into()],
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        };
        store_source_selection(&fixture.store, &fixture.host, &selection)
            .await
            .unwrap();
        let records = fixture
            .authorities
            .iter()
            .map(|authority| authority.record.clone())
            .collect::<Vec<_>>();
        let evidence = ingest_trusted_run_records(&selection, &records).unwrap();
        let bindings = fixture
            .authorities
            .iter()
            .map(|authority| TrustedSourceBinding {
                source_id: authority.record.id.clone(),
                source_digest: authority.trace.source_digest.clone(),
                parent_family: authority.record.parent_family.clone(),
            })
            .collect::<Vec<_>>();
        let allowed = fixture
            .authorities
            .iter()
            .map(|authority| EvidenceRef {
                id: authority.record.id.clone(),
                digest: authority.trace.source_digest.clone(),
            })
            .collect::<Vec<_>>();
        let approved_parent = d("approved-parent");
        let safe_baseline = d("safe-baseline");
        let edit_context = TrustedEditContext::new(
            TENANT,
            "profile",
            "skill-a",
            "v1",
            approved_parent.clone(),
            safe_baseline.clone(),
            &fixture.parent_skill,
            allowed.clone(),
        )
        .unwrap();
        let edit_template = SkillEditBatch {
            schema_version: SKILL_EDIT_SCHEMA.into(),
            compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
            namespace: TENANT.into(),
            profile_id: "profile".into(),
            skill_id: "skill-a".into(),
            skill_version: "v1".into(),
            input_digest: skill_snapshot_digest(&fixture.parent_skill).unwrap(),
            approved_parent_digest: approved_parent.clone(),
            safe_baseline_digest: safe_baseline.clone(),
            evidence: EvidenceClosure {
                support: vec![allowed[0].clone()],
                counterexamples: vec![allowed[1].clone()],
                dependencies: allowed.clone(),
            },
            edits: vec![],
        };
        Self {
            selection,
            evidence,
            bindings,
            allowed,
            edit_context,
            edit_template,
            profile: Profile {
                id: "profile".into(),
                evolution_enabled: true,
                parent_digest: approved_parent,
                baseline_digest: safe_baseline,
            },
            baseline: fixture.parent_skill.clone(),
            strategy: Strategy::default(),
            patch: ImproverPatch::default(),
            caps: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            revoked: BTreeSet::new(),
        }
    }

    /// The optimization request of `episode` (a claim id for a consolidation).
    fn request<'a>(
        &'a self,
        fixture: &'a Fixture,
        episode: &str,
        shape: Shape,
    ) -> OptimizationStepRequest<'a> {
        OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.selection,
            source_bindings: &self.bindings,
            traces: if shape.traces {
                fixture
                    .authorities
                    .iter()
                    .map(|authority| authority.trace.clone())
                    .collect()
            } else {
                vec![]
            },
            model_context: ModelRequestContext {
                request_id: "consolidation-request-1".into(),
                namespace: TENANT.into(),
                purpose: Purpose::Development,
                stage: shape.stage,
                episode_id: episode.into(),
                step: 1,
                attempt: 1,
                parent_skill_digest: fixture.parent_skill_digest.clone(),
                bundle_digest: fixture.parent_bundle_digest.clone(),
                source_closure: self.allowed.clone(),
                model_digest: d("fixture-model"),
                tools_digest: d("tools"),
                rules_digest: d("rules"),
                sampling_digest: d("sampling"),
                revoke_watermark: 1,
                max_suggestions: 4,
            },
            parent_skill: &fixture.parent_skill,
            edit_context: &self.edit_context,
            edit_batch_template: self.edit_template.clone(),
            protected_ranges: &[],
            bundle_context: BundleCompileContext {
                profile: &self.profile,
                baseline: &self.baseline,
                parent_strategy: &self.strategy,
                baseline_strategy: &self.strategy,
                improver_patch: &self.patch,
                caps: &self.caps,
                revoked: &self.revoked,
            },
            development_request: DevelopmentRunRequest {
                request_id: "consolidation-development-1".into(),
                namespace: TENANT.into(),
                purpose: Purpose::Development,
                episode_id: episode.into(),
                step: 1,
                attempt: 1,
                manifest: fixture.manifest.clone(),
                parent_bundle_digest: fixture.parent_bundle_digest.clone(),
                candidate_bundle_digest: d("candidate-1"),
                environment_digest: fixture.environment.environment.environment_digest.clone(),
                grader_digest: fixture.grader_digest.clone(),
                rules_digest: d("rules"),
                tools_digest: d("tools"),
                revoke_watermark: 1,
                idempotency_key: "consolidation-idempotency-1".into(),
            },
            allow_rank_call: shape.rank,
        }
    }
}

// ---------------------------------------------------------------------------
// The scripted transport: real broker, real ledger, a provider that misbehaves
// on demand.
// ---------------------------------------------------------------------------

/// What the provider does to the first model call. Every injection ends the step
/// at that call, so the ledger holds exactly one model call for it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Injection {
    /// Answers with valid edit suggestions.
    None,
    /// Never answers: the transport gives up at its deadline.
    Timeout,
    /// The dispatch group is cancelled while the provider call is in flight.
    Cancel,
    /// Answers, and is billed, with text that is not JSON.
    InvalidJson,
    /// Answers without a usage record: nothing to reconcile the cost against.
    MissingCost,
    /// Answers only after the call's lease has expired.
    LateResult,
}

#[derive(Clone, Copy)]
struct Script {
    injection: Injection,
    /// Suggestions each reflection call returns. More than two per batch makes
    /// the pool exceed four, which takes the rank call.
    suggestions_per_batch: usize,
    lease_seconds: i64,
}

impl Script {
    const PLAIN: Script = Script {
        injection: Injection::None,
        suggestions_per_batch: 1,
        lease_seconds: 60,
    };
    /// Three suggestions per batch: six in the pool, so the step ranks.
    const RANKING: Script = Script {
        suggestions_per_batch: 3,
        ..Script::PLAIN
    };

    fn injecting(injection: Injection) -> Self {
        Self {
            injection,
            // The late result needs a lease short enough to expire under the clock.
            lease_seconds: if injection == Injection::LateResult {
                20
            } else {
                Script::PLAIN.lease_seconds
            },
            ..Script::PLAIN
        }
    }
}

/// Clock time of the broker before the provider answers, and after a late answer.
const CLOCK_START: i64 = 10;
const CLOCK_LATE: i64 = 100;

struct ScriptedTransport {
    script: Script,
    store: Store,
    evaluator: Context,
    clock: Arc<AtomicI64>,
    calls: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ScriptedTransport {
    /// The provider's answer, read from the labelled inputs of the request: the
    /// stage cannot say what is asked, because a consolidation step makes every
    /// call under `Consolidate`.
    fn output(&self, request: &ModelRequest) -> Result<String> {
        if request
            .input
            .iter()
            .any(|part| part.label == "rank-controls")
        {
            return serde_json::to_string(&["fix-rule-0", "preserve-rule-0"])
                .map_err(|_| Error::Internal);
        }
        let input = |label: &str| -> Result<&str> {
            request
                .input
                .iter()
                .find(|part| part.label == label)
                .map(|part| part.content.as_str())
                .ok_or_else(|| Error::Invalid(format!("fixture input {label} missing")))
        };
        let parent: SkillSnapshot = serde_json::from_str(input("parent-skill")?)
            .map_err(|_| Error::Invalid("fixture parent input invalid".into()))?;
        let strategy: Strategy = serde_json::from_str(input("generation-strategy")?)
            .map_err(|_| Error::Invalid("fixture strategy input invalid".into()))?;
        let success = request
            .input
            .iter()
            .any(|part| part.label == "reflection-success");
        let (id, batch_id, field, start) = if success {
            (
                "preserve-rule",
                "reflection-success",
                SkillTextField::Applicability,
                parent.applicability.len(),
            )
        } else {
            (
                "fix-rule",
                "reflection-failure",
                SkillTextField::Content,
                parent.content.len(),
            )
        };
        let source = request.source_closure[0].clone();
        let count = self.script.suggestions_per_batch;
        let suggestions: Vec<EditSuggestion> = (0..count)
            .map(|index| EditSuggestion {
                id: if count == 1 {
                    id.into()
                } else {
                    format!("{id}-{index}")
                },
                hypothesis: "bounded fixture hypothesis".into(),
                batch_ids: vec![batch_id.into()],
                support: vec![source.clone()],
                counterexamples: if success {
                    vec![source.clone()]
                } else {
                    vec![]
                },
                dependencies: vec![source.clone()],
                edit: SkillTextEdit {
                    field,
                    start,
                    end: start,
                    expected_text_digest: hash(b""),
                    exact_anchor: None,
                    operation: TextEditOperation::Insert {
                        text: format!(" [{} consolidation-stage]", strategy.instruction),
                    },
                },
            })
            .collect();
        serde_json::to_string(&suggestions).map_err(|_| Error::Internal)
    }
}

#[async_trait]
impl ModelTransport for ScriptedTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(request.clone());
        match self.script.injection {
            Injection::Timeout => {
                // The provider never answers; the transport's own deadline fires.
                tokio::time::timeout(Duration::from_millis(20), std::future::pending::<()>())
                    .await
                    .unwrap_err();
                return Err(Error::Conflict("provider request timed out".into()));
            }
            Injection::Cancel => {
                self.store
                    .stop_dispatch_group(
                        &self.evaluator,
                        SCOPE,
                        &request.episode_id,
                        "cancelled_during_consolidation",
                        15,
                    )
                    .await?;
            }
            Injection::LateResult => self.clock.store(CLOCK_LATE, Ordering::SeqCst),
            Injection::None | Injection::InvalidJson | Injection::MissingCost => {}
        }
        let output = match self.script.injection {
            Injection::InvalidJson => "not json".to_string(),
            _ => self.output(request)?,
        };
        let mut answer = completion(request, output);
        if self.script.injection == Injection::MissingCost {
            answer.usage_record_id.clear();
        }
        Ok(answer)
    }
}

/// A fixture, its step material and a real broker over the scripted transport.
struct World {
    fixture: Fixture,
    material: StepMaterial,
    broker: PersistentModelBroker<ScriptedTransport>,
    runner: LedgerDevRunner,
    /// Provider calls the transport has executed.
    provider_calls: Arc<AtomicUsize>,
    /// The requests those calls carried.
    seen: Arc<Mutex<Vec<ModelRequest>>>,
}

impl World {
    async fn new(script: Script) -> Self {
        let fixture = Fixture::new().await;
        let material = StepMaterial::new(&fixture).await;
        let runner = LedgerDevRunner::new(&fixture);
        let clock = Arc::new(AtomicI64::new(CLOCK_START));
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let read_clock: Arc<dyn Fn() -> i64 + Send + Sync> = {
            let clock = clock.clone();
            Arc::new(move || clock.load(Ordering::SeqCst))
        };
        let broker = PersistentModelBroker::with_clock(
            fixture.store.clone(),
            ScriptedTransport {
                script,
                store: fixture.store.clone(),
                evaluator: context("evaluator", Role::Evaluator),
                clock,
                calls: provider_calls.clone(),
                seen: seen.clone(),
            },
            broker_config(script.lease_seconds),
            read_clock,
        )
        .unwrap();
        Self {
            fixture,
            material,
            broker,
            runner,
            provider_calls,
            seen,
        }
    }

    /// A world with a first-generation claim to run.
    async fn claimed(script: Script) -> (Self, ConsolidationClaim) {
        let world = Self::new(script).await;
        let claim = world.fixture.claimed().await;
        (world, claim)
    }

    async fn consolidate(
        &self,
        claim: &ConsolidationClaim,
        shape: Shape,
    ) -> Result<ConsolidationRunRecord> {
        MonitoringCoordinator::run_consolidation(
            &self.fixture.worker,
            &self.fixture.store,
            &claim.id,
            Some(&self.broker),
            Some(&self.runner),
            Some(&self.fixture.journal()),
            self.material.request(&self.fixture, &claim.id, shape),
        )
        .await
    }

    /// An ordinary E03 step over `episode`, through the same broker.
    async fn ordinary(&self, episode: &str, shape: Shape) -> Result<OptimizationStepOutcome> {
        run_optimization_step(
            Some(&self.broker),
            Some(&self.runner),
            Some(&self.fixture.journal()),
            self.material.request(&self.fixture, episode, shape),
        )
        .await
    }

    fn provider_calls(&self) -> usize {
        self.provider_calls.load(Ordering::SeqCst)
    }

    /// Every ledger row of a dispatch group, oldest first.
    async fn ledger(&self, group: &str) -> Vec<BudgetCallRecord> {
        let mut session = self.fixture.store.session().await.unwrap();
        let mut calls = session
            .budget_calls_for_group(&self.fixture.worker, SCOPE, group)
            .await
            .unwrap();
        session.commit().await.unwrap();
        calls.sort_by_key(|call| (call.created_at, call.call_id.clone()));
        calls
    }

    /// The ledger rows of the model calls of a group (development runs excluded).
    async fn model_calls(&self, group: &str) -> Vec<BudgetCallRecord> {
        self.ledger(group)
            .await
            .into_iter()
            .filter(|call| {
                call.call_id.starts_with("optrequest-") || call.call_id.starts_with("optrank-")
            })
            .collect()
    }

    async fn root(&self) -> RootBudgetRecord {
        self.fixture
            .store
            .root_budget(&self.fixture.worker, SCOPE)
            .await
            .unwrap()
            .unwrap()
    }

    async fn claim_state(&self, claim: &ConsolidationClaim) -> ConsolidationClaimState {
        let mut session = self.fixture.store.session().await.unwrap();
        let stored: ConsolidationClaim = session
            .need(&self.fixture.worker, "artifact", &claim.id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        stored.state
    }

    /// The stage facts of an episode.
    async fn stage_facts(&self, episode: &str) -> Vec<FactView> {
        let mut session = self.fixture.store.session().await.unwrap();
        let all: Vec<Value> = session
            .list(&self.fixture.worker, "artifact")
            .await
            .unwrap();
        session.commit().await.unwrap();
        all.into_iter()
            .filter(|fact| {
                fact["schema_version"] == OPTIMIZATION_STAGE_FACT_SCHEMA
                    && fact["episode_id"] == json!(episode)
            })
            .map(|fact| FactView {
                kind: fact["kind"].as_str().unwrap().to_string(),
                stage: fact["stage"].as_str().unwrap().to_string(),
                request_id: fact["request_id"].as_str().unwrap().to_string(),
            })
            .collect()
    }
}

/// The parts of a stored stage fact this file reads.
struct FactView {
    kind: String,
    stage: String,
    request_id: String,
}

/// The stages the model-call facts of an episode are filed under: the facts a model
/// call writes (prepared, observed, dispatched) whose request is one of the step's
/// reflection or rank requests. The development run's own dispatch facts are not
/// model calls.
fn model_call_fact_stages(facts: &[FactView]) -> BTreeSet<String> {
    facts
        .iter()
        .filter(|fact| {
            [
                "request_prepared",
                "response_observed",
                "dispatch_prepared",
                "dispatch_observed",
            ]
            .contains(&fact.kind.as_str())
                && (fact.request_id.starts_with("optrequest-")
                    || fact.request_id.starts_with("optrank-"))
        })
        .map(|fact| fact.stage.clone())
        .collect()
}

/// The stages the step's own prepared/completed facts are filed under.
fn step_fact_stages(facts: &[FactView]) -> BTreeSet<String> {
    facts
        .iter()
        .filter(|fact| fact.kind == "step_prepared" || fact.kind == "step_completed")
        .map(|fact| fact.stage.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// 1. A consolidation run is metered as Consolidation; an ordinary step is not.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_model_call_of_a_consolidation_run_is_metered_as_consolidation() {
    let (world, claim) = World::claimed(Script::RANKING).await;
    let record = world
        .consolidate(
            &claim,
            Shape {
                rank: true,
                ..Shape::CONSOLIDATION
            },
        )
        .await
        .unwrap();
    assert_eq!(
        record.outcome,
        ConsolidationRunOutcome::Candidate,
        "{record:?}"
    );

    // Two reflection calls and the rank call: three provider calls in all.
    assert_eq!(world.provider_calls(), 3);
    let models = world.model_calls(&claim.id).await;
    assert_eq!(models.len(), 3, "two reflection calls and one rank call");
    assert_eq!(
        models
            .iter()
            .filter(|call| call.call_id.starts_with("optrank-"))
            .count(),
        1
    );
    for call in &models {
        assert_eq!(call.stage, BudgetStage::Consolidation, "{}", call.call_id);
        assert_eq!(call.dispatch_group_id, claim.id);
        assert_eq!(call.billing_scope, SCOPE);
        assert_eq!(call.state, BudgetCallState::Finalized);
        assert_eq!(call.actual_cost_micros, Some(COST_MICROS));
    }
    // Nothing of the run was metered as ordinary optimization...
    let ledger = world.ledger(&claim.id).await;
    assert!(ledger.iter().all(|call| !matches!(
        call.stage,
        BudgetStage::Reflection | BudgetStage::Ranking | BudgetStage::Merge
    )));
    // ...and the development run keeps its own stage: only model calls moved.
    let development: Vec<_> = ledger
        .iter()
        .filter(|call| call.call_id.starts_with("dev-call-"))
        .collect();
    assert_eq!(development.len(), 1);
    assert_eq!(development[0].stage, BudgetStage::DevelopmentExecution);
    assert_eq!(record.budget_call_ids.len(), 4);

    // Still the original root: its spend is the model calls plus the development run.
    let root = world.root().await;
    assert_eq!(root.root_budget_id, ROOT);
    assert_eq!(root.spent_micros, 3 * COST_MICROS + 10);
    assert_eq!(root.reserved_micros, 0);

    // The journal's model-call facts carry the consolidation stage; the step's own
    // prepared/completed facts keep the stage they always had.
    let facts = world.stage_facts(&claim.id).await;
    assert_eq!(
        model_call_fact_stages(&facts),
        BTreeSet::from(["consolidate".to_string()])
    );
    assert_eq!(
        step_fact_stages(&facts),
        BTreeSet::from(["merge".to_string()])
    );
}

#[tokio::test]
async fn an_ordinary_e03_step_keeps_its_reflection_and_ranking_ledger_stages() {
    // The base stage of an ordinary step has never decided its calls' stages, and
    // still does not: whatever it is, reflection stays reflection, rank stays ranking.
    for base in [
        ModelStage::ReflectFailure,
        ModelStage::ReflectSuccess,
        ModelStage::Merge,
        ModelStage::Rank,
    ] {
        let world = World::new(Script::RANKING).await;
        let outcome = world
            .ordinary(
                "e03-episode",
                Shape {
                    rank: true,
                    ..Shape::at(base)
                },
            )
            .await
            .unwrap();
        assert!(
            matches!(outcome, OptimizationStepOutcome::Candidate { .. }),
            "{base:?}: {outcome:?}"
        );
        assert_eq!(world.provider_calls(), 3, "{base:?}");
        let mut stages: Vec<_> = world
            .model_calls("e03-episode")
            .await
            .iter()
            .map(|call| call.stage)
            .collect();
        stages.sort();
        assert_eq!(
            stages,
            vec![
                BudgetStage::Reflection,
                BudgetStage::Reflection,
                BudgetStage::Ranking
            ],
            "{base:?}"
        );
        let facts = world.stage_facts("e03-episode").await;
        assert_eq!(
            model_call_fact_stages(&facts),
            BTreeSet::from([
                "reflect_failure".to_string(),
                "reflect_success".to_string(),
                "rank".to_string()
            ]),
            "{base:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. The stage and the entry agree: Consolidate only through run_consolidation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_ordinary_step_cannot_use_the_consolidate_stage() {
    let world = World::new(Script::PLAIN).await;
    let refused = world.ordinary("e03-episode", Shape::CONSOLIDATION).await;
    assert!(matches!(refused, Err(Error::Forbidden)), "{refused:?}");
    // Refused before anything was journaled, dispatched or spent.
    assert_eq!(world.provider_calls(), 0);
    assert_eq!(world.runner.calls.load(Ordering::SeqCst), 0);
    assert!(world.ledger("e03-episode").await.is_empty());
    assert!(world.stage_facts("e03-episode").await.is_empty());
    assert_eq!(world.root().await.spent_micros, 0);
    assert_eq!(world.root().await.reserved_micros, 0);
    // The same request under the stage it always had runs as before.
    let outcome = world
        .ordinary("e03-episode", Shape::at(ModelStage::ReflectFailure))
        .await
        .unwrap();
    assert!(
        matches!(outcome, OptimizationStepOutcome::Candidate { .. }),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_consolidation_request_without_the_consolidate_stage_is_refused_untouched() {
    let (world, claim) = World::claimed(Script::PLAIN).await;
    for stage in [
        ModelStage::ReflectFailure,
        ModelStage::ReflectSuccess,
        ModelStage::Merge,
        ModelStage::Rank,
    ] {
        let refused = world.consolidate(&claim, Shape::at(stage)).await;
        assert!(
            matches!(refused, Err(Error::Forbidden)),
            "{stage:?}: {refused:?}"
        );
        // Refused before the claim was taken: nothing was dispatched or spent, and
        // the claim is exactly as it was.
        assert_eq!(
            world.claim_state(&claim).await,
            ConsolidationClaimState::Claimed
        );
        assert_eq!(world.provider_calls(), 0);
        assert!(world.ledger(&claim.id).await.is_empty());
        assert!(world.stage_facts(&claim.id).await.is_empty());
        assert_eq!(world.root().await.spent_micros, 0);
    }
    // The refusals did not wedge the claim: the right request still runs it.
    let record = world
        .consolidate(
            &claim,
            Shape {
                traces: false,
                ..Shape::CONSOLIDATION
            },
        )
        .await
        .unwrap();
    assert_eq!(
        record.outcome,
        ConsolidationRunOutcome::NoChange,
        "{record:?}"
    );
    assert_eq!(
        world.claim_state(&claim).await,
        ConsolidationClaimState::CompletedNoChange
    );
}

// ---------------------------------------------------------------------------
// 3. V096.a: timeout, cancellation, invalid JSON, missing cost and a late result
//    in the consolidation stage.
// ---------------------------------------------------------------------------

/// What the caller expects of one injection.
struct Expected {
    outcome: ConsolidationRunOutcome,
    claim: ConsolidationClaimState,
}

/// One injected consolidation run and what every injection must leave behind.
struct Injected {
    world: World,
    claim: ConsolidationClaim,
    record: ConsolidationRunRecord,
    /// The one model call that was dispatched.
    call: BudgetCallRecord,
    /// The root budget after the run.
    root: RootBudgetRecord,
}

/// Runs a claimed consolidation into `injection` and checks what every injection
/// must leave: one consolidation-stage ledger row for the call that was dispatched,
/// in the claim's own group and root, no development run, and the terminal record
/// and claim state the caller expects.
async fn inject(injection: Injection, expected: Expected) -> Injected {
    let (world, claim) = World::claimed(Script::injecting(injection)).await;
    let record = world
        .consolidate(&claim, Shape::CONSOLIDATION)
        .await
        .unwrap();
    assert_eq!(record.outcome, expected.outcome, "{record:?}");
    assert_eq!(world.claim_state(&claim).await, expected.claim);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.runner.calls.load(Ordering::SeqCst), 0);
    let models = world.model_calls(&claim.id).await;
    assert_eq!(models.len(), 1);
    let call = models[0].clone();
    assert_eq!(call.stage, BudgetStage::Consolidation);
    assert_eq!(call.stage.as_str(), "consolidation");
    assert_eq!(call.dispatch_group_id, claim.id);
    assert_eq!(call.billing_scope, SCOPE);
    assert_eq!(record.budget_call_ids, vec![call.call_id.clone()]);
    assert_eq!(world.ledger(&claim.id).await.len(), 1);
    // The provider was asked under the consolidation stage, for that very call.
    let seen = world.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].stage, ModelStage::Consolidate);
    assert_eq!(seen[0].request_id, call.call_id);
    let root = world.root().await;
    assert_eq!(root.root_budget_id, ROOT);
    Injected {
        world,
        claim,
        record,
        call,
        root,
    }
}

impl Injected {
    /// Runs the same consolidation again and dispatches the very same model request
    /// again. The stored record comes back, the provider is not called a second time,
    /// and neither the ledger row nor any money moves. Returns the broker's answer to
    /// the repeated dispatch.
    async fn assert_never_redispatched(&self) -> ModelResponse {
        let again = self
            .world
            .consolidate(&self.claim, Shape::CONSOLIDATION)
            .await
            .unwrap();
        assert_eq!(again, self.record);
        let request = self.world.seen.lock().unwrap()[0].clone();
        let redispatched = self.world.broker.dispatch(request).await.unwrap();
        assert_eq!(
            self.world.provider_calls(),
            1,
            "the dispatched call is never sent again"
        );
        let calls = self.world.model_calls(&self.claim.id).await;
        assert_eq!(
            calls,
            vec![self.call.clone()],
            "the ledger row did not move"
        );
        assert_eq!(self.world.root().await, self.root, "no money moved");
        redispatched
    }
}

#[tokio::test]
async fn a_consolidation_timeout_is_uncertain_and_keeps_its_reservation() {
    let run = inject(
        Injection::Timeout,
        Expected {
            outcome: ConsolidationRunOutcome::Uncertain,
            claim: ConsolidationClaimState::CompletedUncertain,
        },
    )
    .await;
    // Dispatched and never answered: the whole reservation stays held, nothing is
    // charged, and nothing is released.
    assert_eq!(run.call.state, BudgetCallState::Uncertain);
    assert_eq!(run.call.reserved_micros, RESERVED_MICROS);
    assert_eq!(run.call.actual_cost_micros, None);
    assert!(run.call.dispatch_id.is_some());
    assert_eq!(run.root.reserved_micros, RESERVED_MICROS);
    assert_eq!(run.root.spent_micros, 0);
    assert_eq!(run.record.reason, "model_usage_unknown");
    assert!(matches!(
        run.assert_never_redispatched().await,
        ModelResponse::Uncertain { .. }
    ));
}

#[tokio::test]
async fn a_consolidation_cancelled_in_flight_bills_the_answer_and_blocks_it() {
    let run = inject(
        Injection::Cancel,
        Expected {
            outcome: ConsolidationRunOutcome::Rejected,
            claim: ConsolidationClaimState::CompletedRejected,
        },
    )
    .await;
    // The provider had already answered when the cancellation landed: its cost is
    // charged, its answer is kept for audit and never used.
    assert_eq!(run.call.actual_cost_micros, Some(COST_MICROS));
    assert_eq!(run.call.response_usable, Some(false));
    assert_eq!(
        run.call.response_block_reason.as_deref(),
        Some("dispatch_group_stopped")
    );
    assert!(run.call.execution_closed);
    assert_eq!(run.root.spent_micros, COST_MICROS);
    assert_eq!(run.root.reserved_micros, 0);
    assert!(matches!(
        run.assert_never_redispatched().await,
        ModelResponse::Rejected { .. }
    ));
}

#[tokio::test]
async fn a_consolidation_answer_that_is_not_json_is_billed_and_rejected() {
    let run = inject(
        Injection::InvalidJson,
        Expected {
            outcome: ConsolidationRunOutcome::Rejected,
            claim: ConsolidationClaimState::CompletedRejected,
        },
    )
    .await;
    // The transport's completion was valid and is billed; the text is what is invalid.
    assert_eq!(run.call.state, BudgetCallState::Finalized);
    assert_eq!(run.call.actual_cost_micros, Some(COST_MICROS));
    assert_eq!(run.call.response_usable, Some(true));
    assert_eq!(run.root.spent_micros, COST_MICROS);
    assert_eq!(run.root.reserved_micros, 0);
    // The step's reason is the fixed code of its class: the parser's own error, which
    // echoes the answer, is not kept.
    assert_eq!(
        run.record.reason, "suggestion_shape_invalid",
        "{}",
        run.record.reason
    );
    // The stored answer is what a repeat gets back, not a second provider call.
    assert!(matches!(
        run.assert_never_redispatched().await,
        ModelResponse::Completed { .. }
    ));
}

#[tokio::test]
async fn a_consolidation_answer_without_a_usage_record_is_uncertain_and_keeps_its_reservation() {
    let run = inject(
        Injection::MissingCost,
        Expected {
            outcome: ConsolidationRunOutcome::Uncertain,
            claim: ConsolidationClaimState::CompletedUncertain,
        },
    )
    .await;
    // No usage record to reconcile: the cost stays unknown and the reservation held,
    // never guessed and never released.
    assert_eq!(run.call.state, BudgetCallState::Uncertain);
    assert_eq!(run.call.reserved_micros, RESERVED_MICROS);
    assert_eq!(run.call.actual_cost_micros, None);
    assert_eq!(run.call.usage_record_id, None);
    assert!(run.call.response_artifact.is_none());
    assert_eq!(run.root.reserved_micros, RESERVED_MICROS);
    assert_eq!(run.root.spent_micros, 0);
    assert_eq!(
        run.record.reason, "model_transport_outcome_unknown",
        "{}",
        run.record.reason
    );
    assert!(matches!(
        run.assert_never_redispatched().await,
        ModelResponse::Uncertain { .. }
    ));
}

#[tokio::test]
async fn a_late_consolidation_answer_is_billed_and_never_used() {
    let run = inject(
        Injection::LateResult,
        Expected {
            outcome: ConsolidationRunOutcome::Rejected,
            claim: ConsolidationClaimState::CompletedRejected,
        },
    )
    .await;
    // The answer arrived after the lease: it is charged and kept, never completed.
    assert_eq!(run.call.actual_cost_micros, Some(COST_MICROS));
    assert_eq!(run.call.response_usable, Some(false));
    assert_eq!(
        run.call.response_block_reason.as_deref(),
        Some("lease_expired")
    );
    assert!(run.call.execution_closed);
    assert_eq!(run.root.spent_micros, COST_MICROS);
    assert_eq!(run.root.reserved_micros, 0);
    assert!(matches!(
        run.assert_never_redispatched().await,
        ModelResponse::Rejected { .. }
    ));
}
