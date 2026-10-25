//! AG-105 (E04 root budget state machine, plan L1239-L1245; §7.2.1 L831, §11 L1077,
//! §6.6 L705/L709; V096(a), its bound-path sub-scope): settlement and execution close
//! are two commits, so a crash between them leaves a settled call whose execution is
//! still open, and the billing root admits no other dispatch while it is. A bound
//! continuation replayed over such a call records the lost close before its
//! compiler-authority and source checks, and those checks then refuse exactly as
//! before: the accounting fact comes before the content decision (the rule AG-097
//! set on the legacy replay path), and no content is revived.
//!
//! * T1, compiler authority changed (an Admin moves the profile pointer; the source
//!   watermark stays): the replay returns the very error the bound request's own
//!   `verify_current` returns. Before it, exactly the close is written: the call's
//!   three execution-close columns and one `execution_closed` event with the reason
//!   `recovered_bound_response`. State, charge, usage, receipt, response and the root
//!   totals are unchanged, and the next call on the same billing root is dispatched.
//! * T2, a request source revoked with the real `LifecycleStore::begin_revoke` (the
//!   watermark moves): the same.
//! * R1 (AG-105-R1): the same revocation with its real cleanup run to completion, so
//!   the call's request and response artifacts are redacted and the stored request
//!   artifact no longer has the bound request's fingerprint. The close does not
//!   depend on whether the content is still there: the same input digest closes the
//!   call first, and the replay returns the same error.
//! * T3, guard (green on the baseline as well): the same call id with another input
//!   digest, or the same input under another bound request artifact, is refused
//!   (`call_id_reused`, or the authority error first) and writes nothing; the call
//!   stays open.
//! * T4: a refused call replayed again records no second close. A live replay
//!   (authority unchanged; green on the baseline as well) returns the persisted
//!   response and records exactly one close.
//!
//! Not covered (and not claimed): a finalized call without a response artifact
//! (AG-106), reconciliation, a real provider or bill, the release chain, and entry
//! points other than `dispatch_bound`. Real SQLite store, real broker, fixture
//! transport, no provider, zero monetary cost. Fixture shapes are copied from
//! `step_terminal_classes_v42.rs` (`seeded_store`, `repair_*`) and
//! `broker_cache_revoke_v42.rs` (`settle_open`, `sqlite_snapshot`,
//! `assert_only_recovery_close`); those files are not used here.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, SkillSnapshot};
use evo_core::evidence::{ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
    OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
};
use evo_core::skill_edit::{EvidenceRef, skill_snapshot_digest};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::formal_authority::{
    CompilerInputRequest, resolve_compiler_inputs, resolve_current_compiler_inputs,
};
use evo_engine::model::{
    BoundModelRequest, CompilerInputUse, CurrentBoundModelRequest, CurrentCompilerInputUse,
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelRejectionKind, ModelResponse,
    ReadBoundModelRequest, RejectedDispatch,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const NAMESPACE: &str = "n";
const PROFILE: &str = "profile";
const SCOPE: &str = "binding-scope";
const ROOT: &str = "binding-root";
const RUNS: [&str; 2] = ["run-failure", "run-success"];
/// The reason the bound path records when a replay recovers a lost close.
const RECOVERY_CLOSE: &str = "recovered_bound_response";
/// The broker's clock: every close a replay records is written at this time.
const NOW: i64 = 10;
/// What the fixture transport charges for one call.
const COST: i64 = 5;
/// `CompilerInputUse::verify_current` when the live profile pointer moved.
const AUTHORITY_CHANGED: &str = "compiler authority changed before continuation";
/// `resolve_compiler_inputs` when the source revoke watermark moved.
const BINDING_MISMATCH: &str = "full P/B profile/environment/version/capability binding mismatch";
const CALL_ID_REUSED: &str = "broker_pre_dispatch_v1.call_id_reused";
/// The schema the revocation cleanup leaves in place of a redacted artifact.
const REDACTED: &str = "rsia.redacted.v1";

fn admin() -> Context {
    Context::new(NAMESPACE, "fixture-admin", Role::Admin).unwrap()
}

fn worker() -> Context {
    Context::new(NAMESPACE, "worker", Role::Worker).unwrap()
}

fn host() -> Context {
    Context::new(NAMESPACE, "host", Role::Host).unwrap()
}

// ---------------------------------------------------------------------------
// The trusted sources, the explicit compiler root and the bound request
// (shapes of step_terminal_classes_v42.rs)
// ---------------------------------------------------------------------------

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

fn parent_skill() -> SkillSnapshot {
    SkillSnapshot {
        content: "old".into(),
        applicability: "applies".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    }
}

fn no_capabilities() -> HostCapabilities {
    HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    }
}

/// The two Host-issued trusted runs, their selection grant and one watermark bump
/// (the watermark is 1 afterwards).
async fn seeded_store() -> (tempfile::TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bound-recovery-close.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let host = host();
    for ((id, family), outcome) in [(RUNS[0], "family-a"), (RUNS[1], "family-b")]
        .into_iter()
        .zip([TraceOutcome::TaskFailure, TraceOutcome::Success])
    {
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
            run_ids: RUNS.iter().map(|run| (*run).into()).collect(),
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&host, "e09-initial").await.unwrap();
    session.commit().await.unwrap();
    (dir, path, store)
}

/// The compiler input request the step fixture's `compiler_input_request()` yields:
/// its explicit empty root (`trusted_compiler_inputs::inputs`), the parent Skill as
/// search parent and baseline, default strategies and no capability, at `watermark`.
fn compiler_request(watermark: u64) -> CompilerInputRequest {
    let environment = hash(b"environment");
    let fixture = trusted_compiler_inputs::inputs(
        NAMESPACE,
        PROFILE,
        parent_skill(),
        Strategy::default(),
        no_capabilities(),
        &environment,
    );
    CompilerInputRequest {
        profile: fixture.profile,
        search_parent: parent_skill(),
        baseline: parent_skill(),
        search_parent_strategy: Strategy::default(),
        expected_parent_strategy_digest: None,
        baseline_strategy: Strategy::default(),
        capabilities: no_capabilities(),
        environment_digest: environment,
        purpose: Purpose::Development,
        revoke_watermark: watermark,
    }
}

/// `repair_compiler_use`: installs the explicit root once and binds the compiler
/// inputs as they are now.
async fn compiler_use(store: &Store, watermark: u64) -> CompilerInputUse {
    let request = compiler_request(watermark);
    Box::pin(trusted_compiler_inputs::install_request(store, &request))
        .await
        .unwrap();
    let mut s = store.session().await.unwrap();
    let verified = Box::pin(resolve_compiler_inputs(&mut s, &worker(), &request))
        .await
        .unwrap();
    s.commit().await.unwrap();
    CompilerInputUse::new(request, verified.binding().clone())
}

/// `repair_model_request`: the step fixture's reflection request `optimizer-{tag}`
/// over `sources` at `watermark`, with `content` as its one input.
fn model_request(tag: &str, watermark: u64, sources: &[&str], content: &str) -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: format!("optimizer-{tag}"),
            namespace: NAMESPACE.into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "binding-world".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: skill_snapshot_digest(&parent_skill()).unwrap(),
            bundle_digest: hash(b"parent-bundle"),
            source_closure: sources
                .iter()
                .map(|run| EvidenceRef {
                    id: (*run).into(),
                    digest: hash(run.as_bytes()),
                })
                .collect(),
            model_digest: hash(b"model"),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: hash(b"sampling"),
            revoke_watermark: watermark,
            max_suggestions: 4,
        },
        vec![ModelInputPart {
            role: ModelInputRole::User,
            label: "task".into(),
            content: content.into(),
        }],
    )
    .unwrap()
}

fn bound_request(
    tag: &str,
    watermark: u64,
    sources: &[&str],
    content: &str,
    inputs: &CompilerInputUse,
) -> BoundModelRequest {
    BoundModelRequest::new(
        model_request(tag, watermark, sources, content),
        inputs.clone(),
    )
    .unwrap()
}

// Preserve the complete original wire. Authority still comes exclusively from
// the real broker/resolver Session; this conversion is not an approval fixture.
fn read_bound<T: serde::Serialize>(bound: &T) -> ReadBoundModelRequest {
    let original = serde_json::to_vec(bound).expect("fixture bound wire serialization");
    let read: ReadBoundModelRequest =
        serde_json::from_slice(&original).expect("fixture exact supported bound wire");
    assert_eq!(serde_json::to_vec(&read).unwrap(), original);
    read
}

// Current health here means the existing ProgramFixture/EmptyRoot program
// boundary only. No OS identity, provider source, grade or paid authority is minted.
async fn current_compiler_use(store: &Store, watermark: u64) -> CurrentCompilerInputUse {
    let request = compiler_request(watermark);
    Box::pin(trusted_compiler_inputs::install_request(store, &request))
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let actual = Box::pin(resolve_current_compiler_inputs(
        &mut session,
        &worker(),
        &request,
    ))
    .await
    .unwrap();
    let inputs = CurrentCompilerInputUse::new(request, actual.binding().clone());
    inputs
        .verify_current(&mut session, &worker())
        .await
        .unwrap();
    session.commit().await.unwrap();
    inputs
}

fn current_bound_request(
    tag: &str,
    watermark: u64,
    sources: &[&str],
    content: &str,
    inputs: &CurrentCompilerInputUse,
) -> CurrentBoundModelRequest {
    CurrentBoundModelRequest::new(
        model_request(tag, watermark, sources, content),
        inputs.clone(),
    )
    .expect("fixture complete Current request")
}

/// `repair_move_pointer_epoch`: a genuine scoped Admin audit of an empty software
/// profile pointer. The bound compiler authority changes; the source watermark
/// does not. It creates no Approved release, report, provider capture or history.
async fn repair_move_pointer_epoch(store: &Store, epoch: u64) {
    let admin = admin();
    let id = "profile-pointer-profile";
    let body = evo_engine::release_store::PersistentProfilePointer {
        id: id.into(),
        schema_version: "rsia.profile_pointer.v1".into(),
        profile_id: PROFILE.into(),
        active_release_id: None,
        active_bundle_digest: None,
        epoch,
    };
    let mut s = store.session().await.unwrap();
    s.put(&admin, "pointer", id, admin.actor(), &body)
        .await
        .unwrap();
    let operation = format!(
        "formal.admin.content.v1.{}",
        fingerprint(&(admin.namespace(), "pointer", id)).unwrap()
    );
    let entity = fingerprint(&(
        admin.namespace(),
        "pointer",
        id,
        evo_engine::formal_authority::content_digest(&body).unwrap(),
    ))
    .unwrap();
    s.audit(&admin, &operation, &entity).await.unwrap();
    s.commit().await.unwrap();
}

/// `RepairBoundTransport`: a fixture completion of fixed cost; every call is counted.
#[derive(Clone)]
struct RepairBoundTransport {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ModelTransport for RepairBoundTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(TransportCompletion {
            response_id: format!("response-{index}"),
            provider_request_id: format!("provider-{index}"),
            usage_record_id: format!("usage-{index}"),
            actual_model_digest: request.model_digest.clone(),
            output: "[]".into(),
            actual_cost_micros: COST,
            currency: "USD".into(),
            pricing_version: "price".into(),
        })
    }
}

// ---------------------------------------------------------------------------
// The world: store, billing root and broker (repair_root / repair_broker)
// ---------------------------------------------------------------------------

struct World {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: Store,
    transport: RepairBoundTransport,
}

impl World {
    async fn new() -> Self {
        let (dir, path, store) = seeded_store().await;
        store
            .authorize_root_budget(
                &admin(),
                &RootBudgetAuthorization {
                    root_budget_id: ROOT.into(),
                    billing_scope: SCOPE.into(),
                    allowed_namespaces: vec![NAMESPACE.into()],
                    currency: "USD".into(),
                    pricing_version: "price".into(),
                    payment_subject: "software-fixture".into(),
                    authorization_receipt_digest: hash(b"fixture-authorization"),
                    per_call_cap_micros: 20,
                    total_limit_micros: 100,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
        Self {
            _dir: dir,
            path,
            store,
            transport: RepairBoundTransport {
                calls: Arc::new(AtomicUsize::new(0)),
            },
        }
    }

    fn broker(&self) -> PersistentModelBroker<RepairBoundTransport> {
        PersistentModelBroker::with_clock(
            self.store.clone(),
            self.transport.clone(),
            BrokerConfig {
                billing_scope: SCOPE.into(),
                actor: "host".into(),
                currency: "USD".into(),
                pricing_version: "price".into(),
                max_cost_micros: 20,
                lease_seconds: 60,
            },
            Arc::new(|| NOW),
        )
        .unwrap()
    }

    /// The real `dispatch_bound` of a fresh broker over the same store.
    async fn replay(&self, bound: &impl serde::Serialize) -> Result<ModelResponse> {
        self.broker().dispatch_compiler_bound(read_bound(bound)).await
    }

    async fn replay_current(&self, bound: &CurrentBoundModelRequest) -> Result<ModelResponse> {
        self.broker()
            .dispatch_compiler_bound(read_bound(bound))
            .await
    }

    async fn call(&self, bound: &impl serde::Serialize) -> BudgetCallRecord {
        let read = read_bound(bound);
        let bound = &read;
        self.store
            .budget_call(&host(), SCOPE, &bound.request.request_id)
            .await
            .unwrap()
            .unwrap()
    }

    /// The root's (reserved, spent) micros.
    async fn totals(&self) -> (i64, i64) {
        let root = self
            .store
            .root_budget(&admin(), SCOPE)
            .await
            .unwrap()
            .unwrap();
        (root.reserved_micros, root.spent_micros)
    }

    fn provider_calls(&self) -> usize {
        self.transport.calls.load(Ordering::SeqCst)
    }

    fn snapshot(&self) -> Value {
        sqlite_snapshot(&self.path)
    }
}

/// Real bound reservation -> begin dispatch -> fixture transport -> atomic
/// settlement, persisted as `dispatch_bound` and `execute_committed_model` persist
/// them, and no close: what a crash between the settlement commit and the close
/// commit leaves (shape of `broker_cache_revoke_v42::settle_open` with the manual
/// bound reservation of `step_terminal_classes_v42`). No execution flag is written
/// by SQL. Returns the persisted usable response.
async fn settle_open(world: &World, bound: &impl serde::Serialize) -> ModelResponse {
    let read = read_bound(bound);
    let bound = &read;
    let ctx = host();
    let request = &bound.request;
    let reservation = BudgetCallReservation {
        billing_scope: SCOPE.into(),
        call_id: request.request_id.clone(),
        dispatch_group_id: request.episode_id.clone(),
        stage: BudgetStage::Reflection,
        actual_input_digest: request.cache_key_digest.clone(),
        request_artifact: Some(
            BudgetArtifact::from_serializable(bound.schema_version.clone(), bound).unwrap(),
        ),
        max_cost_micros: 20,
        lease_token: fingerprint(&("host", SCOPE, bound)).unwrap(),
        lease_until: NOW + 60,
        now: NOW,
    };
    let sources = request
        .source_closure
        .iter()
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    let reserved = world
        .store
        .reserve_budget_call_with_sources(&ctx, &reservation, &sources)
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: SCOPE.into(),
        call_id: request.request_id.clone(),
        actual_input_digest: request.cache_key_digest.clone(),
        lease_token: reserved.lease_token,
        lease_epoch: reserved.lease_epoch,
        now: NOW,
    };
    let dispatched = world
        .store
        .begin_budget_dispatch(&ctx, &fence)
        .await
        .unwrap();
    assert!(dispatched.new_dispatch);
    let dispatch_id = dispatched.call.dispatch_id.unwrap();
    let completion = world
        .transport
        .execute(request, &dispatch_id)
        .await
        .unwrap();
    let receipt = ModelExecutionReceipt {
        call_id: request.request_id.clone(),
        dispatch_id,
        root_budget_id: ROOT.into(),
        provider_request_id: completion.provider_request_id.clone(),
        usage_record_id: completion.usage_record_id.clone(),
        provenance: ModelExecutionProvenance::Fixture,
    };
    let output_digest = hash(completion.output.as_bytes());
    let completed = ModelResponse::Completed {
        request_id: request.request_id.clone(),
        response_id: completion.response_id.clone(),
        actual_model_digest: completion.actual_model_digest.clone(),
        input_digest: request.input_digest.clone(),
        output: completion.output.clone(),
        output_digest: output_digest.clone(),
        execution_receipt: receipt.clone(),
    };
    completed.validate_against(request).unwrap();
    let blocked = ModelResponse::Rejected {
        request_id: request.request_id.clone(),
        kind: ModelRejectionKind::CancelledAfterDispatch,
        reason: "dispatch became ineligible after provider execution; result retained for audit"
            .into(),
        dispatch: RejectedDispatch::Dispatched { receipt },
    };
    let settlement = world
        .store
        .settle_model_budget_call(
            &ctx,
            &fence,
            &UsageCharge {
                amount_micros: completion.actual_cost_micros,
                currency: completion.currency.clone(),
                pricing_version: completion.pricing_version.clone(),
                provider_request_id: completion.provider_request_id.clone(),
                usage_record_id: completion.usage_record_id.clone(),
                output_digest,
            },
            &ModelCallSettlementEvidence {
                provenance: BudgetExecutionProvenance::Fixture,
                actual_model_digest: Some(completion.actual_model_digest.clone()),
                transport_artifact: BudgetArtifact::from_serializable(
                    "rsia.model_transport.artifact.v1",
                    &completion,
                )
                .unwrap(),
                usable_response: Some(
                    BudgetArtifact::from_serializable(
                        "rsia.model_response.artifact.v1",
                        &completed,
                    )
                    .unwrap(),
                ),
                blocked_response: BudgetArtifact::from_serializable(
                    "rsia.model_response.artifact.v1",
                    &blocked,
                )
                .unwrap(),
                forced_block_reason: None,
            },
        )
        .await
        .unwrap();
    assert!(settlement.response_usable);
    if bound.schema_version == evo_engine::model::CURRENT_BOUND_MODEL_SCHEMA {
        assert_current_settled_open(&settlement.call);
    } else {
        assert_settled_open(&settlement.call);
    }
    assert_eq!(world.totals().await, (0, COST));
    completed
}

/// The error the bound request's own authority check returns now, read in a
/// Session that is rolled back.
async fn current_refusal(store: &Store, bound: &impl serde::Serialize) -> Error {
    let read = read_bound(bound);
    let bound = &read;
    let mut s = store.session().await.unwrap();
    let error = Box::pin(bound.verify_current(&mut s, &host()))
        .await
        .unwrap_err();
    drop(s);
    error
}

/// The next call on the same billing root, through the real bound broker with the
/// compiler authority current now. It reaches the root's concurrency gate only
/// when every other call on the root has closed its execution.
async fn next_call(world: &World, watermark: u64, sources: &[&str]) -> ModelResponse {
    let inputs = current_compiler_use(&world.store, watermark).await;
    let next = current_bound_request(
        "same-root-next",
        watermark,
        sources,
        "software fixture",
        &inputs,
    );
    world.replay_current(&next).await.unwrap()
}

/// Runs the real revocation cleanup job to completion (shape of
/// `broker_cache_revoke_v42::finish_cleanup`).
async fn finish_cleanup(world: &World, job_id: &str) {
    for now in 31..200 {
        let status = Box::pin(LifecycleStore::cleanup_step(
            &admin(),
            &world.store,
            job_id,
            8,
            now,
        ))
        .await
        .unwrap();
        if status.state == CleanupState::Complete {
            return;
        }
        assert_ne!(status.state, CleanupState::Failed);
    }
    panic!("cleanup job {job_id} did not complete");
}

// ---------------------------------------------------------------------------
// Assertions (shapes of broker_cache_revoke_v42.rs)
// ---------------------------------------------------------------------------

fn assert_settled_open(call: &BudgetCallRecord) {
    assert_eq!(call.state, BudgetCallState::Finalized);
    assert_eq!(call.response_usable, Some(true));
    assert_eq!(call.actual_cost_micros, Some(COST));
    assert!(!call.execution_closed, "fixture execution was not open");
    assert_eq!(
        call.response_artifact.as_ref().unwrap().schema_version,
        "rsia.model_response.artifact.v1"
    );
    assert_eq!(
        call.request_artifact.as_ref().unwrap().schema_version,
        "rsia.bound_model_request.v1"
    );
    assert!(call.dispatch_id.is_some());
}

fn assert_current_settled_open(call: &BudgetCallRecord) {
    assert_eq!(call.state, BudgetCallState::Finalized);
    assert_eq!(call.response_usable, Some(true));
    assert_eq!(call.actual_cost_micros, Some(COST));
    assert!(!call.execution_closed, "fixture execution was not open");
    assert_eq!(
        call.response_artifact.as_ref().unwrap().schema_version,
        "rsia.model_response.artifact.v1"
    );
    assert_eq!(
        call.request_artifact.as_ref().unwrap().schema_version,
        evo_engine::model::CURRENT_BOUND_MODEL_SCHEMA
    );
    assert!(call.dispatch_id.is_some());
}

fn assert_conflict(error: &Error, reason: &str) {
    assert!(
        matches!(error, Error::Conflict(actual) if actual == reason),
        "expected Conflict({reason}), got {error:?}"
    );
}

fn assert_same_error(actual: Result<ModelResponse>, expected: &Error) {
    match actual {
        Err(error) => assert_eq!(format!("{error:?}"), format!("{expected:?}")),
        Ok(response) => panic!("expected {expected:?}, the replay returned {response:?}"),
    }
}

fn assert_same_response(actual: &ModelResponse, expected: &ModelResponse) {
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

fn assert_call_id_reused(response: ModelResponse) {
    match response {
        ModelResponse::Rejected {
            kind: ModelRejectionKind::InvalidRequest,
            reason,
            dispatch: RejectedDispatch::NotDispatched,
            ..
        } => assert_eq!(reason, CALL_ID_REUSED),
        other => panic!("expected the {CALL_ID_REUSED} refusal, got {other:?}"),
    }
}

fn assert_dispatched(next: &ModelResponse) {
    assert!(
        matches!(next, ModelResponse::Completed { .. }),
        "the replayed call still holds the billing root: {next:?}"
    );
}

fn receipt(response: &ModelResponse) -> &ModelExecutionReceipt {
    match response {
        ModelResponse::Completed {
            execution_receipt, ..
        } => execution_receipt,
        ModelResponse::Rejected {
            dispatch: RejectedDispatch::Dispatched { receipt },
            ..
        } => receipt,
        _ => panic!("fixture response has no dispatched receipt"),
    }
}

/// Exact rows of every table, including ledger/usage/events, references, edges,
/// tombstones, cleanup queues, audit, idempotency and FTS shadow tables. Read-only
/// Python SQLite is already used by engine tests; no new dependency is needed.
fn sqlite_snapshot(path: &Path) -> Value {
    let script = r#"import json,sqlite3,sys
from pathlib import Path
con=sqlite3.connect(Path(sys.argv[1]).resolve().as_uri()+'?mode=ro',uri=True)
con.execute('PRAGMA query_only=ON')
con.execute('BEGIN')
def quoted(name):
    return '"'+name.replace('"','""')+'"'
result={}
for (name,) in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    columns=[row[1] for row in con.execute('PRAGMA table_info('+quoted(name)+')')]
    sql='SELECT '+','.join('quote('+quoted(column)+')' for column in columns)+' FROM '+quoted(name)
    result[name]={'columns':columns,'rows':sorted([list(row) for row in con.execute(sql)])}
con.rollback()
con.close()
print(json.dumps(result,sort_keys=True))
"#;
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(script)
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot: Value = serde_json::from_slice(&output.stdout).unwrap();
    for table in [
        "objects",
        "dependencies",
        "audit",
        "root_budget_calls",
        "root_budget_events",
        "revoke_cleanup_jobs",
    ] {
        assert!(snapshot.get(table).is_some(), "snapshot omitted {table}");
    }
    snapshot
}

fn column(table: &Value, name: &str) -> usize {
    table["columns"]
        .as_array()
        .unwrap()
        .iter()
        .position(|column| column == name)
        .unwrap_or_else(|| panic!("snapshot column {name} is missing"))
}

/// Rows in the order `sqlite_snapshot` writes them: Python's `sorted` over lists
/// of `quote()` strings, which compares code points like `String` does.
fn sort_rows(rows: &mut [Value]) {
    let key = |row: &Value| {
        row.as_array()
            .unwrap()
            .iter()
            .map(|cell| cell.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    rows.sort_by_key(key);
}

/// The `execution_closed` ledger events of one call.
fn closed_events(snapshot: &Value, call_id: &str) -> usize {
    let events = &snapshot["root_budget_events"];
    let (call, kind) = (column(events, "call_id"), column(events, "event_kind"));
    let quoted_call = json!(format!("'{call_id}'"));
    events["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row[call] == quoted_call && row[kind] == json!("'execution_closed'"))
        .count()
}

/// What a replay of a settled, still open call may write: the call's three
/// execution-close columns, one `execution_closed` ledger event and the events
/// AUTOINCREMENT counter. Every other table, the root totals and the call's state,
/// charge, usage, receipt and response columns stay identical.
fn assert_only_recovery_close(before: &Value, after: &Value, call: &BudgetCallRecord) {
    let mut expected = before.clone();
    let quoted_call = json!(format!("'{}'", call.call_id));
    let calls = &mut expected["root_budget_calls"];
    let (call_id, closed, closed_at, reason) = (
        column(calls, "call_id"),
        column(calls, "execution_closed"),
        column(calls, "execution_closed_at"),
        column(calls, "execution_close_reason"),
    );
    let row = calls["rows"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row[call_id] == quoted_call)
        .unwrap();
    assert_eq!(row[closed], json!("0"), "fixture execution was not open");
    row[closed] = json!("1");
    row[closed_at] = json!(NOW.to_string());
    row[reason] = json!(format!("'{RECOVERY_CLOSE}'"));
    let events = &after["root_budget_events"];
    let previous = before["root_budget_events"]["rows"].as_array().unwrap();
    let added = events["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| !previous.contains(row))
        .collect::<Vec<_>>();
    assert_eq!(added.len(), 1, "unexpected ledger events: {added:?}");
    let event = added[0];
    assert_eq!(event[column(events, "call_id")], quoted_call);
    assert_eq!(
        event[column(events, "event_kind")],
        json!("'execution_closed'")
    );
    assert_eq!(event[column(events, "event_at")], json!(NOW.to_string()));
    let details = event[column(events, "details")].as_str().unwrap();
    let details: Value =
        serde_json::from_str(&details[1..details.len() - 1].replace("''", "'")).unwrap();
    assert_eq!(
        details,
        json!({"dispatch_id": call.dispatch_id, "reason": RECOVERY_CLOSE})
    );
    let rows = expected["root_budget_events"]["rows"]
        .as_array_mut()
        .unwrap();
    rows.push(event.clone());
    sort_rows(rows);
    let sequence = &mut expected["sqlite_sequence"];
    let (name, seq) = (column(sequence, "name"), column(sequence, "seq"));
    let rows = sequence["rows"].as_array_mut().unwrap();
    rows.iter_mut()
        .find(|row| row[name] == json!("'root_budget_events'"))
        .unwrap()[seq] = event[column(events, "seq")].clone();
    sort_rows(rows);
    assert_eq!(
        *after, expected,
        "the replay changed more than its recovery close"
    );
}

// ---------------------------------------------------------------------------
// T1-T4
// ---------------------------------------------------------------------------

/// T1. The compiler authority changed after the settlement (an Admin moved the
/// profile pointer; the watermark is unchanged), so the replay fails at
/// `verify_current`. It first records the lost close, an accounting fact, so the
/// settled call no longer counts as active, then returns the same error; the paid
/// call stays verifiable and its content is not returned.
#[tokio::test]
async fn authority_change_replay_closes_the_settled_call_then_refuses_unchanged() {
    let world = World::new().await;
    let inputs = current_compiler_use(&world.store, 1).await;
    let bound = current_bound_request("authority-changed", 1, &RUNS, "software fixture", &inputs);
    let completed = settle_open(&world, &bound).await;
    repair_move_pointer_epoch(&world.store, 1).await;
    let expected = current_refusal(&world.store, &bound).await;
    assert_conflict(&expected, AUTHORITY_CHANGED);
    let settled = world.call(&bound).await;
    assert_settled_open(&settled);
    let before = world.snapshot();
    assert_same_error(world.replay(&bound).await, &expected);
    let after = world.snapshot();
    let replayed = world.call(&bound).await;
    let next = next_call(&world, 1, &RUNS).await;
    assert!(
        replayed.execution_closed,
        "the refused replay left the settled call open on its root \
         (execution_closed=0); the next call on that root got {next:?}"
    );
    assert_only_recovery_close(&before, &after, &settled);
    assert_eq!(
        replayed.execution_close_reason.as_deref(),
        Some(RECOVERY_CLOSE)
    );
    assert_dispatched(&next);
    assert_eq!(world.provider_calls(), 2);
    assert_eq!(world.totals().await, (0, 2 * COST));
    world
        .broker()
        .verify_receipt(&bound.request, receipt(&completed))
        .await
        .unwrap();
    assert_same_error(world.replay(&bound).await, &expected);
    assert_eq!(world.provider_calls(), 2);
}

/// T2. A request source is revoked with the real `begin_revoke`: the watermark
/// moves and the replay fails at `verify_current` (the compiler binding records
/// the old watermark). It first records the lost close, then returns the same
/// error; the next call, at the new watermark over the live source, dispatches.
#[tokio::test]
async fn source_revocation_replay_closes_the_settled_call_then_refuses_unchanged() {
    let world = World::new().await;
    let inputs = current_compiler_use(&world.store, 1).await;
    let bound = current_bound_request("source-revoked", 1, &RUNS, "software fixture", &inputs);
    let completed = settle_open(&world, &bound).await;
    let status = LifecycleStore::begin_revoke(
        &admin(),
        &world.store,
        TypedObjectRef {
            kind: "run".into(),
            id: RUNS[1].into(),
        },
        "fixture source revoked",
        30,
    )
    .await
    .unwrap();
    assert_eq!(status.state, CleanupState::Pending);
    assert_eq!(status.watermark_seq, 2);
    let expected = current_refusal(&world.store, &bound).await;
    assert_conflict(&expected, BINDING_MISMATCH);
    let settled = world.call(&bound).await;
    assert_settled_open(&settled);
    let before = world.snapshot();
    assert_same_error(world.replay(&bound).await, &expected);
    let after = world.snapshot();
    let replayed = world.call(&bound).await;
    let next = next_call(&world, status.watermark_seq, &RUNS[..1]).await;
    assert!(
        replayed.execution_closed,
        "the refused replay left the settled call open on its root \
         (execution_closed=0); the next call on that root got {next:?}"
    );
    assert_only_recovery_close(&before, &after, &settled);
    assert_dispatched(&next);
    assert_eq!(world.provider_calls(), 2);
    assert_eq!(world.totals().await, (0, 2 * COST));
    world
        .broker()
        .verify_receipt(&bound.request, receipt(&completed))
        .await
        .unwrap();
    assert_same_error(world.replay(&bound).await, &expected);
    assert_eq!(world.provider_calls(), 2);
}

/// R1 (the controller's counterexample P2 to AG-105). The same revocation, and its
/// real cleanup ran to completion while the close was still lost: the call's
/// request, transport and response artifacts are redacted, so the stored request
/// artifact cannot have the bound request's fingerprint. The accounting close does
/// not depend on whether the content is still there (§11, §7.2.1; the legacy rule
/// of AG-097): the same input digest closes the call first, then the replay
/// returns the same error and the next call on the root dispatches.
#[tokio::test]
async fn redacted_replay_closes_the_settled_call_then_refuses_unchanged() {
    let world = World::new().await;
    let inputs = current_compiler_use(&world.store, 1).await;
    let bound = current_bound_request("source-redacted", 1, &RUNS, "software fixture", &inputs);
    let completed = settle_open(&world, &bound).await;
    let status = LifecycleStore::begin_revoke(
        &admin(),
        &world.store,
        TypedObjectRef {
            kind: "run".into(),
            id: RUNS[1].into(),
        },
        "fixture source revoked",
        30,
    )
    .await
    .unwrap();
    finish_cleanup(&world, &status.job_id).await;
    let redacted = world.call(&bound).await;
    for artifact in [&redacted.request_artifact, &redacted.response_artifact] {
        assert_eq!(artifact.as_ref().unwrap().schema_version, REDACTED);
    }
    assert_eq!(redacted.state, BudgetCallState::Finalized);
    assert_eq!(redacted.response_usable, Some(false));
    assert_eq!(redacted.actual_input_digest, bound.request.cache_key_digest);
    assert_ne!(
        fingerprint(redacted.request_artifact.as_ref().unwrap()).unwrap(),
        fingerprint(
            &BudgetArtifact::from_serializable("rsia.bound_model_request.v1", &bound).unwrap()
        )
        .unwrap()
    );
    assert!(!redacted.execution_closed, "fixture execution was not open");
    let expected = current_refusal(&world.store, &bound).await;
    assert_conflict(&expected, BINDING_MISMATCH);
    let before = world.snapshot();
    assert_same_error(world.replay(&bound).await, &expected);
    let after = world.snapshot();
    let replayed = world.call(&bound).await;
    let next = next_call(&world, status.watermark_seq, &RUNS[..1]).await;
    assert!(
        replayed.execution_closed,
        "the refused replay left the redacted settled call open on its root \
         (execution_closed=0); the next call on that root got {next:?}"
    );
    assert_only_recovery_close(&before, &after, &redacted);
    assert_eq!(closed_events(&after, &bound.request.request_id), 1);
    assert_dispatched(&next);
    assert_eq!(world.provider_calls(), 2);
    assert_eq!(world.totals().await, (0, 2 * COST));
    world
        .broker()
        .verify_receipt(&bound.request, receipt(&completed))
        .await
        .unwrap();
    assert_same_error(world.replay(&bound).await, &expected);
    assert_eq!(world.provider_calls(), 2);
}

/// T3, guard (also green on the baseline). Only the same key may close: the same
/// call id with another input digest, or the same input under another bound
/// request artifact, is refused as before (`call_id_reused`, or the authority
/// error when that check refuses first) and writes nothing, so the settled call
/// stays open.
#[tokio::test]
async fn reused_call_id_with_other_input_or_binding_refuses_without_closing() {
    let world = World::new().await;
    let legacy_inputs = current_compiler_use(&world.store, 1).await;
    let legacy_other_input = current_bound_request(
        "reused",
        1,
        &RUNS,
        "another software fixture",
        &legacy_inputs,
    );
    let inputs = current_compiler_use(&world.store, 1).await;
    let original = current_bound_request("reused", 1, &RUNS, "software fixture", &inputs);
    settle_open(&world, &original).await;
    let settled = world.call(&original).await;
    // (a) Another input digest under the same call id; the authority is unchanged.
    let other_input =
        current_bound_request("reused", 1, &RUNS, "another software fixture", &inputs);
    assert_eq!(other_input.request.request_id, original.request.request_id);
    assert_ne!(
        other_input.request.cache_key_digest,
        original.request.cache_key_digest
    );
    let before = world.snapshot();
    assert_call_id_reused(world.replay_current(&other_input).await.unwrap());
    assert_eq!(world.snapshot(), before, "a reused call id wrote");
    // (b) The same input digest under another bound request artifact: the binding
    // that is current after an Admin moved the profile pointer.
    repair_move_pointer_epoch(&world.store, 1).await;
    let current = current_compiler_use(&world.store, 1).await;
    let other_binding = CurrentBoundModelRequest::new(original.request.clone(), current).unwrap();
    assert_eq!(
        other_binding.request.cache_key_digest,
        original.request.cache_key_digest
    );
    let moved = world.snapshot();
    assert_call_id_reused(world.replay_current(&other_binding).await.unwrap());
    // (c) Another input digest under the stale binding: the authority check refuses
    // first, and nothing was closed before it.
    let expected = current_refusal(&world.store, &other_input).await;
    assert_conflict(&expected, AUTHORITY_CHANGED);
    assert_same_error(world.replay_current(&other_input).await, &expected);
    // Keep the original stale v1 negative wire in addition to Current health.
    let legacy_expected = current_refusal(&world.store, &legacy_other_input).await;
    assert_conflict(&legacy_expected, AUTHORITY_CHANGED);
    assert_same_error(world.replay(&legacy_other_input).await, &legacy_expected);
    assert_eq!(world.snapshot(), moved, "a reused call id wrote");
    assert_eq!(world.call(&original).await, settled);
    assert_eq!(closed_events(&moved, &original.request.request_id), 0);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
}

/// T4. A refused call replayed again and again records its close once: the
/// second and third replays write nothing and return the same error.
#[tokio::test]
async fn repeated_refused_replays_record_exactly_one_close() {
    let world = World::new().await;
    let inputs = current_compiler_use(&world.store, 1).await;
    let bound = current_bound_request("repeated", 1, &RUNS, "software fixture", &inputs);
    settle_open(&world, &bound).await;
    repair_move_pointer_epoch(&world.store, 1).await;
    let expected = current_refusal(&world.store, &bound).await;
    assert_conflict(&expected, AUTHORITY_CHANGED);
    let settled = world.call(&bound).await;
    let before = world.snapshot();
    assert_same_error(world.replay(&bound).await, &expected);
    let once = world.snapshot();
    for _ in 0..2 {
        assert_same_error(world.replay(&bound).await, &expected);
        assert_eq!(world.snapshot(), once, "a repeated replay wrote again");
    }
    assert_only_recovery_close(&before, &once, &settled);
    assert_eq!(closed_events(&once, &bound.request.request_id), 1);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
}

/// T4, live form (also green on the baseline). The authority is unchanged: the
/// replay returns the persisted response unchanged, without a new dispatch, and
/// the settled call is closed exactly once however often it is replayed.
#[tokio::test]
async fn live_replay_returns_the_persisted_response_and_closes_once() {
    let world = World::new().await;
    let inputs = current_compiler_use(&world.store, 1).await;
    let bound = current_bound_request("live", 1, &RUNS, "software fixture", &inputs);
    let completed = settle_open(&world, &bound).await;
    let settled = world.call(&bound).await;
    let before = world.snapshot();
    assert_same_response(&world.replay_current(&bound).await.unwrap(), &completed);
    let once = world.snapshot();
    assert_same_response(&world.replay_current(&bound).await.unwrap(), &completed);
    assert_eq!(world.snapshot(), once, "a repeated replay wrote again");
    assert_only_recovery_close(&before, &once, &settled);
    assert_eq!(closed_events(&once, &bound.request.request_id), 1);
    world
        .broker()
        .verify_receipt(&bound.request, receipt(&completed))
        .await
        .unwrap();
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
}

#[tokio::test]
async fn historical_v1_closes_its_paid_record_but_cannot_authorize_new_dispatch() {
    let world = World::new().await;
    let inputs = compiler_use(&world.store, 1).await;
    let bound = bound_request("legacy-paid-reader", 1, &RUNS, "software fixture", &inputs);
    let completed = settle_open(&world, &bound).await;
    let call = world.call(&bound).await;
    assert_eq!(
        call.request_artifact.as_ref().unwrap().schema_version,
        "rsia.bound_model_request.v1"
    );
    let before = world.snapshot();
    assert_conflict(
        &world.replay(&bound).await.unwrap_err(),
        "legacy compiler use is not Current input",
    );
    let once = world.snapshot();
    assert_only_recovery_close(&before, &once, &call);
    assert_eq!(closed_events(&once, &bound.request.request_id), 1);
    world
        .broker()
        .verify_receipt(&bound.request, receipt(&completed))
        .await
        .unwrap();
    let new_legacy = bound_request("legacy-new-use", 1, &RUNS, "software fixture", &inputs);
    assert_conflict(
        &world.replay(&new_legacy).await.unwrap_err(),
        "legacy compiler use is not Current input",
    );
    assert_eq!(world.snapshot(), once);
    repair_move_pointer_epoch(&world.store, 1).await;
    let moved = world.snapshot();
    let expected = current_refusal(&world.store, &bound).await;
    assert_conflict(&expected, AUTHORITY_CHANGED);
    assert_same_error(world.replay(&bound).await, &expected);
    assert_eq!(world.snapshot(), moved);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
}

#[tokio::test]
async fn current_model_preimage_refusal_preserves_dispatched_accounting() {
    let world = World::new().await;
    let inputs = current_compiler_use(&world.store, 1).await;
    let bound = current_bound_request("source-model-preimage", 1, &RUNS, "software fixture", &inputs);
    let completed = settle_open(&world, &bound).await;
    let settled = world.call(&bound).await;
    assert_settled_open(&settled);
    let fixture = trusted_compiler_inputs::inputs(NAMESPACE, PROFILE, parent_skill(), Strategy::default(), no_capabilities(), &hash(b"environment"));
    let mut session = world.store.session().await.unwrap();
    let mut provider: Value = session.need(&admin(), "artifact", &fixture.provider.id).await.unwrap();
    provider["provider_model"] = json!("another-legal-model");
    session.put(&admin(), "artifact", &fixture.provider.id, admin().actor(), &provider).await.unwrap();
    session.commit().await.unwrap();
    let expected = current_refusal(&world.store, &bound).await;
    assert!(matches!(expected, Error::Forbidden));
    let before = world.snapshot();
    assert_same_error(world.replay_current(&bound).await, &expected);
    let after = world.snapshot();
    assert_only_recovery_close(&before, &after, &settled);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
    let closed = world.call(&bound).await;
    assert_eq!(closed.actual_cost_micros, Some(COST));
    assert_eq!(closed.response_artifact, settled.response_artifact);
    assert_eq!(closed.usage_record_id, settled.usage_record_id);
    assert_eq!(closed_events(&after, &bound.request.request_id), 1);
    assert_same_error(world.replay_current(&bound).await, &expected);
    assert_eq!(world.snapshot(), after);
    world.broker().verify_receipt(&bound.request, receipt(&completed)).await.unwrap();
}
