//! AG121: six redacted-request boundary controls through the existing broker.
//! World, compiler inputs and public settlement/cleanup setup are copied from
//! broker_bound_recovery_close_v42; its original tests stay in their own target.
//! SETUP/SNAPSHOT failures are invalid controls. Only the named TARGET assertion
//! can establish a Parent semantic failure. No real provider or physical zero-SQL
//! claim follows from these software fixtures or logical database snapshots.

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
/// `resolve_compiler_inputs` when the source revoke watermark moved.
const BINDING_MISMATCH: &str = "full P/B profile/environment/version/capability binding mismatch";
const CALL_ID_REUSED: &str = "broker_pre_dispatch_v1.call_id_reused";
/// The schema the revocation cleanup leaves in place of a redacted artifact.
const REDACTED: &str = "rsia.redacted.v1";

fn admin() -> Context {
    Context::new(NAMESPACE, "fixture-admin", Role::Admin)
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
}

fn worker() -> Context {
    Context::new(NAMESPACE, "worker", Role::Worker).expect("AG121_INVALID_SETUP_OR_OBSERVATION")
}

fn host() -> Context {
    Context::new(NAMESPACE, "host", Role::Host).expect("AG121_INVALID_SETUP_OR_OBSERVATION")
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
    let dir = tempfile::tempdir().expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let path = dir.path().join("bound-recovery-close.sqlite3");
    let store = Store::open(&path)
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
    .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let mut session = store
        .session()
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    session
        .bump_watermark(&host, "e09-initial")
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    session
        .commit()
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let mut s = store
        .session()
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let verified = Box::pin(resolve_compiler_inputs(&mut s, &worker(), &request))
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    s.commit()
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
            parent_skill_digest: skill_snapshot_digest(&parent_skill())
                .expect("AG121_INVALID_SETUP_OR_OBSERVATION"),
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
    .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
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
    .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
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
    let mut s = store
        .session()
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    s.put(&admin, "pointer", id, admin.actor(), &body)
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let operation = format!(
        "formal.admin.content.v1.{}",
        fingerprint(&(admin.namespace(), "pointer", id))
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
    );
    let entity = fingerprint(&(
        admin.namespace(),
        "pointer",
        id,
        evo_engine::formal_authority::content_digest(&body)
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION"),
    ))
    .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    s.audit(&admin, &operation, &entity)
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    s.commit()
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
    }

    /// The real `dispatch_bound` of a fresh broker over the same store.
    async fn replay(&self, bound: &BoundModelRequest) -> Result<ModelResponse> {
        let broker = self.broker();
        broker.dispatch_bound(bound.clone()).await
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
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
    }

    /// The root's (reserved, spent) micros.
    async fn totals(&self) -> (i64, i64) {
        let root = self
            .store
            .root_budget(&admin(), SCOPE)
            .await
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
        (root.reserved_micros, root.spent_micros)
    }

    fn provider_calls(&self) -> usize {
        self.transport.calls.load(Ordering::SeqCst)
    }
}
/// Real bound reservation -> begin dispatch -> fixture transport -> atomic
/// settlement, persisted as `dispatch_bound` and `execute_committed_model` persist
/// them, and no close: what a crash between the settlement commit and the close
/// commit leaves (shape of `broker_cache_revoke_v42::settle_open` with the manual
/// bound reservation of `step_terminal_classes_v42`). No execution flag is written
/// by SQL. Returns the persisted usable response.
async fn settle_open(world: &World, bound: &BoundModelRequest) -> ModelResponse {
    let ctx = host();
    let request = &bound.request;
    let reservation = BudgetCallReservation {
        billing_scope: SCOPE.into(),
        call_id: request.request_id.clone(),
        dispatch_group_id: request.episode_id.clone(),
        stage: BudgetStage::Reflection,
        actual_input_digest: request.cache_key_digest.clone(),
        request_artifact: Some(
            BudgetArtifact::from_serializable("rsia.bound_model_request.v1", bound)
                .expect("AG121_INVALID_SETUP_OR_OBSERVATION"),
        ),
        max_cost_micros: 20,
        lease_token: fingerprint(&("host", SCOPE, bound))
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION"),
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
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert!(dispatched.new_dispatch);
    let dispatch_id = dispatched
        .call
        .dispatch_id
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let completion = world
        .transport
        .execute(request, &dispatch_id)
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
    completed
        .validate_against(request)
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
                .expect("AG121_INVALID_SETUP_OR_OBSERVATION"),
                usable_response: Some(
                    BudgetArtifact::from_serializable(
                        "rsia.model_response.artifact.v1",
                        &completed,
                    )
                    .expect("AG121_INVALID_SETUP_OR_OBSERVATION"),
                ),
                blocked_response: BudgetArtifact::from_serializable(
                    "rsia.model_response.artifact.v1",
                    &blocked,
                )
                .expect("AG121_INVALID_SETUP_OR_OBSERVATION"),
                forced_block_reason: None,
            },
        )
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert!(settlement.response_usable);
    assert_settled_open(&settlement.call);
    assert_eq!(world.totals().await, (0, COST));
    completed
}

/// The error the bound request's own authority check returns now, read in a
/// Session that is rolled back.
async fn current_refusal(store: &Store, bound: &BoundModelRequest) -> Error {
    let mut s = store
        .session()
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let error = match Box::pin(bound.verify_current(&mut s, &host())).await {
        Err(error) => error,
        Ok(()) => panic!("AG121_INVALID_SETUP: expected a current authority refusal"),
    };
    drop(s);
    error
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
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
        call.response_artifact
            .as_ref()
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
            .schema_version,
        "rsia.model_response.artifact.v1"
    );
    assert_eq!(
        call.request_artifact
            .as_ref()
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
            .schema_version,
        "rsia.bound_model_request.v1"
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
fn column(table: &Value, name: &str) -> usize {
    table["columns"]
        .as_array()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        .iter()
        .position(|column| column == name)
        .unwrap_or_else(|| panic!("snapshot column {name} is missing"))
}

// AG121 fixed boundary controls. Prepared only; compile/semantic results are not claimed.
fn ag121_fixed_text_cell(text: &str) -> Value {
    let hex = text
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("");
    json!(["text", hex])
}

fn ag121_fixed_int_cell(value: i64) -> Value {
    json!(["integer", value.to_string()])
}

fn ag121_fixed_decode_text(cell: &Value) -> String {
    assert_eq!(cell[0], "text");
    let encoded = cell[1]
        .as_str()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert_eq!(encoded.len() % 2, 0);
    let bytes = (0..encoded.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&encoded[i..i + 2], 16).expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        })
        .collect::<Vec<_>>();
    String::from_utf8(bytes).expect("AG121_INVALID_SETUP_OR_OBSERVATION")
}

// Read-only logical snapshot: all tables, storage types, raw text/blob bytes and REAL bits.
// Equality is not a claim that no SQL statement ran or that WAL/filesystem bytes did not change.
fn ag121_fixed_read_lossless_snapshot(path: &Path) -> std::result::Result<Value, String> {
    let script = r#"import json,sqlite3,struct,sys
from pathlib import Path
con=sqlite3.connect(Path(sys.argv[1]).resolve().as_uri()+'?mode=ro',uri=True)
con.text_factory=bytes
con.execute('PRAGMA query_only=ON')
con.execute('BEGIN')
def quoted(name):
    return '"'+name.replace('"','""')+'"'
def cell(kind,value):
    kind=kind.decode('ascii')
    if kind=='null': return [kind,None]
    if kind=='integer': return [kind,str(value)]
    if kind=='real': return [kind,struct.pack('>d',value).hex()]
    if kind in ('text','blob'): return [kind,bytes(value).hex()]
    raise AssertionError(kind)
result={}
for (raw_name,) in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    name=raw_name.decode('utf-8')
    columns=[row[1].decode('utf-8') for row in con.execute('PRAGMA table_info('+quoted(name)+')')]
    expressions=[]
    for column in columns:
        expressions.extend(['typeof('+quoted(column)+')',quoted(column)])
    rows=[]
    for raw in con.execute('SELECT '+','.join(expressions)+' FROM '+quoted(name)):
        rows.append([cell(raw[i],raw[i+1]) for i in range(0,len(raw),2)])
    result[name]={'columns':columns,'rows':sorted(rows,key=lambda r:json.dumps(r,separators=(',',':')))}
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
        .map_err(|error| format!("snapshot process failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "snapshot exit {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let snapshot: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("snapshot is not one complete JSON value: {error}"))?;
    let tables = snapshot.as_object().ok_or("snapshot is not a table map")?;
    for (name, table) in tables {
        let columns = table["columns"]
            .as_array()
            .ok_or_else(|| format!("snapshot {name} has no columns"))?;
        let rows = table["rows"]
            .as_array()
            .ok_or_else(|| format!("snapshot {name} has no rows"))?;
        if !columns.iter().all(Value::is_string)
            || !rows.iter().all(|row| {
                row.as_array()
                    .is_some_and(|cells| cells.len() == columns.len())
            })
        {
            return Err(format!("snapshot {name} has an invalid row shape"));
        }
    }
    Ok(snapshot)
}

fn ag121_fixed_lossless_snapshot(path: &Path) -> Value {
    let snapshot = ag121_fixed_read_lossless_snapshot(path)
        .unwrap_or_else(|error| panic!("AG121_INVALID_SNAPSHOT: {error}"));
    for name in [
        "objects",
        "dependencies",
        "audit",
        "root_budget_calls",
        "root_budget_events",
        "revoke_cleanup_jobs",
        "sqlite_sequence",
    ] {
        assert!(
            snapshot.get(name).is_some(),
            "AG121_INVALID_SNAPSHOT: omitted {name}"
        );
    }
    snapshot
}

fn ag121_phase(case: &str, phase: &str) {
    eprintln!("AG121_CONTROL case={case} phase={phase}");
}

#[test]
fn ag121_lossless_snapshot_preserves_null_nul_blob_real_bits_and_duplicate_rows() {
    ag121_phase("snapshot-health", "SETUP");
    let directory = tempfile::tempdir().expect("AG121_INVALID_SETUP: snapshot tempdir");
    let path = directory.path().join("snapshot-health.sqlite3");
    // A separate scratch database with no product or billing tables. This fixture
    // writes only its own sample; the snapshot process below opens it read-only.
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(
            r#"import sqlite3,sys
con=sqlite3.connect(sys.argv[1])
con.execute('CREATE TABLE probe (n, txt, bytes, fraction REAL, counter INTEGER)')
row=(None,'a\0雪',b'\x00\xff',1.5,9007199254740993)
con.executemany('INSERT INTO probe VALUES (?,?,?,?,?)',[row,row])
con.commit()
con.close()
"#,
        )
        .arg(&path)
        .output()
        .expect("AG121_INVALID_SETUP: snapshot sample process");
    assert!(
        output.status.success(),
        "AG121_INVALID_SETUP: snapshot sample {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ag121_phase("snapshot-health", "SNAPSHOT");
    let snapshot = ag121_fixed_read_lossless_snapshot(&path)
        .unwrap_or_else(|error| panic!("AG121_INVALID_SNAPSHOT: {error}"));
    let row = json!([
        ["null", null],
        ["text", "6100e99baa"],
        ["blob", "00ff"],
        ["real", "3ff8000000000000"],
        ["integer", "9007199254740993"]
    ]);
    ag121_phase("snapshot-health", "TARGET");
    assert_eq!(
        snapshot,
        json!({"probe":{"columns":["n","txt","bytes","fraction","counter"],
        "rows":[row.clone(),row]}})
    );
    ag121_phase("snapshot-health", "PASS");
}

fn ag121_fixed_sort_rows(rows: &mut [Value]) {
    rows.sort_by_cached_key(|row| {
        serde_json::to_string(row).expect("AG121_INVALID_SETUP_OR_OBSERVATION")
    });
}

fn ag121_fixed_close_events(snapshot: &Value, call_id: &str) -> usize {
    let events = &snapshot["root_budget_events"];
    let call = column(events, "call_id");
    let kind = column(events, "event_kind");
    events["rows"]
        .as_array()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        .iter()
        .filter(|row| {
            row[call] == ag121_fixed_text_cell(call_id)
                && row[kind] == ag121_fixed_text_cell("execution_closed")
        })
        .count()
}

fn ag121_fixed_assert_only_close(before: &Value, after: &Value, call: &BudgetCallRecord) {
    let mut expected = before.clone();
    let calls = &mut expected["root_budget_calls"];
    let call_id = column(calls, "call_id");
    let closed = column(calls, "execution_closed");
    let closed_at = column(calls, "execution_closed_at");
    let reason = column(calls, "execution_close_reason");
    let row = calls["rows"]
        .as_array_mut()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        .iter_mut()
        .find(|row| row[call_id] == ag121_fixed_text_cell(&call.call_id))
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert_eq!(row[closed], ag121_fixed_int_cell(0));
    row[closed] = ag121_fixed_int_cell(1);
    row[closed_at] = ag121_fixed_int_cell(NOW);
    row[reason] = ag121_fixed_text_cell(RECOVERY_CLOSE);

    let events = &after["root_budget_events"];
    let old = before["root_budget_events"]["rows"]
        .as_array()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let added = events["rows"]
        .as_array()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        .iter()
        .filter(|row| !old.contains(row))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(added.len(), 1, "unexpected events: {added:?}");
    let event = &added[0];
    assert_eq!(
        event[column(events, "call_id")],
        ag121_fixed_text_cell(&call.call_id)
    );
    assert_eq!(
        event[column(events, "event_kind")],
        ag121_fixed_text_cell("execution_closed")
    );
    assert_eq!(event[column(events, "event_at")], ag121_fixed_int_cell(NOW));
    assert_eq!(
        event[column(events, "billing_scope")],
        ag121_fixed_text_cell(&call.billing_scope)
    );
    let old_sequence = &before["sqlite_sequence"];
    let old_name = column(old_sequence, "name");
    let old_seq = column(old_sequence, "seq");
    let old_sequence_cell = &old_sequence["rows"]
        .as_array()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        .iter()
        .find(|row| row[old_name] == ag121_fixed_text_cell("root_budget_events"))
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")[old_seq];
    assert_eq!(old_sequence_cell[0], "integer");
    let next_sequence = old_sequence_cell[1]
        .as_str()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        .parse::<i64>()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        .checked_add(1)
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert_eq!(
        event[column(events, "seq")],
        ag121_fixed_int_cell(next_sequence),
        "recovery close event sequence must increment exactly once"
    );
    let details: Value =
        serde_json::from_str(&ag121_fixed_decode_text(&event[column(events, "details")]))
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert_eq!(
        details,
        json!({"dispatch_id":call.dispatch_id,"reason":RECOVERY_CLOSE})
    );
    let rows = expected["root_budget_events"]["rows"]
        .as_array_mut()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    rows.push(event.clone());
    ag121_fixed_sort_rows(rows);
    let sequence = &mut expected["sqlite_sequence"];
    let name = column(sequence, "name");
    let seq = column(sequence, "seq");
    let rows = sequence["rows"]
        .as_array_mut()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    rows.iter_mut()
        .find(|row| row[name] == ag121_fixed_text_cell("root_budget_events"))
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")[seq] = event[column(events, "seq")].clone();
    ag121_fixed_sort_rows(rows);
    assert_eq!(
        *after, expected,
        "more than one permitted recovery close changed"
    );
}

async fn ag121_fixed_revoke_and_clean(world: &World) {
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
    .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert_eq!(status.state, CleanupState::Pending);
    finish_cleanup(world, &status.job_id).await;
}

fn ag121_fixed_assert_real_redacted(call: &BudgetCallRecord, original: &BudgetArtifact) {
    let request = call
        .request_artifact
        .as_ref()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert_eq!(request.schema_version, REDACTED);
    assert_eq!(hash(request.body.as_bytes()), request.digest);
    let body: Value =
        serde_json::from_str(&request.body).expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let keys = body
        .as_object()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected_keys: BTreeSet<&str> = [
        "schema_version",
        "state",
        "reason",
        "original_schema",
        "original_digest",
    ]
    .into_iter()
    .collect();
    assert_eq!(keys, expected_keys);
    eprintln!(
        "AG121_FIXED_REDACTED_INPUT {}",
        serde_json::to_string(request).expect("AG121_INVALID_SETUP_OR_OBSERVATION")
    );
    assert_eq!(body["schema_version"], REDACTED);
    assert_eq!(body["state"], "source_revoked");
    assert_eq!(body["reason"], "source_revoked");
    assert_eq!(body["original_schema"], original.schema_version);
    assert_eq!(body["original_digest"], original.digest);
    assert_eq!(call.state, BudgetCallState::Finalized);
    assert_eq!(call.response_usable, Some(false));
    assert!(!call.execution_closed);
}

// Malformed history is constructed only through the already-public BudgetArtifact
// and storage reservation/dispatch/settlement API; no budget row is edited by SQL.
fn ag121_fixed_malformed_shapes(original_digest: &str) -> Vec<(&'static str, BudgetArtifact)> {
    let valid = json!({"schema_version":REDACTED,"state":"source_revoked","reason":"source_revoked",
        "original_schema":"rsia.bound_model_request.v1","original_digest":original_digest});
    let mut shapes = Vec::new();
    let mut missing = valid.clone();
    missing
        .as_object_mut()
        .expect("AG121_INVALID_SETUP: redacted object")
        .remove("original_digest");
    shapes.push(("missing-original-digest", missing));
    let mut extra = valid.clone();
    extra["unexpected"] = json!("not registered");
    shapes.push(("extra-field", extra));
    for (label, field, value) in [
        ("wrong-field-type", "original_digest", json!(7)),
        (
            "wrong-schema-version",
            "schema_version",
            json!("rsia.redacted.v2"),
        ),
        ("wrong-state", "state", json!("live")),
        ("wrong-reason", "reason", json!("unknown")),
        (
            "invalid-original-schema",
            "original_schema",
            json!("invalid schema"),
        ),
        (
            "invalid-original-digest",
            "original_digest",
            json!("A".repeat(64)),
        ),
    ] {
        let mut body = valid.clone();
        body[field] = value;
        shapes.push((label, body));
    }
    let mut artifacts = shapes
        .into_iter()
        .map(|(label, body)| {
            (
                label,
                BudgetArtifact::from_serializable(REDACTED, &body)
                    .expect("AG121_INVALID_SETUP: valid JSON redacted artifact"),
            )
        })
        .collect::<Vec<_>>();
    let duplicate = format!(
        r#"{{"schema_version":"rsia.redacted.v1","state":"source_revoked","reason":"source_revoked","original_schema":"rsia.model_request.artifact.v1","original_schema":"rsia.bound_model_request.v1","original_digest":"{original_digest}"}}"#
    );
    artifacts.push((
        "duplicate-original-schema",
        BudgetArtifact {
            schema_version: REDACTED.into(),
            digest: hash(duplicate.as_bytes()),
            body: duplicate,
        },
    ));
    let noncanonical = format!(
        " {}",
        serde_json::to_string(&valid).expect("AG121_INVALID_SETUP: canonical shape")
    );
    artifacts.push((
        "noncanonical-body",
        BudgetArtifact {
            schema_version: REDACTED.into(),
            digest: hash(noncanonical.as_bytes()),
            body: noncanonical,
        },
    ));
    artifacts
}

// Exact parent settle_open mechanism with only stored request artifact varied.
async fn ag121_fixed_settle_open_with_request(
    world: &World,
    bound: &BoundModelRequest,
    stored_request: BudgetArtifact,
) -> ModelResponse {
    ag121_session_settle_open_with_artifacts(world, bound, stored_request, false).await
}

// Public kernel reader-shape fixture, not a claim of a revocation producer run.
async fn ag121_session_settle_open_with_artifacts(
    world: &World,
    bound: &impl serde::Serialize,
    stored_request: BudgetArtifact,
    redacted_blocked_response: bool,
) -> ModelResponse {
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
        request_artifact: Some(stored_request.clone()),
        max_cost_micros: 20,
        lease_token: fingerprint(&("host", SCOPE, bound))
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION"),
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
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert!(dispatched.new_dispatch);
    let dispatch_id = dispatched
        .call
        .dispatch_id
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let completion = world
        .transport
        .execute(request, &dispatch_id)
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
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
    completed
        .validate_against(request)
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let blocked = ModelResponse::Rejected {
        request_id: request.request_id.clone(),
        kind: ModelRejectionKind::CancelledAfterDispatch,
        reason: "dispatch became ineligible after provider execution; result retained for audit"
            .into(),
        dispatch: RejectedDispatch::Dispatched { receipt },
    };
    let blocked_artifact =
        BudgetArtifact::from_serializable("rsia.model_response.artifact.v1", &blocked)
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let blocked_artifact = if redacted_blocked_response {
        ag121_session_redacted_artifact(&blocked_artifact.schema_version, &blocked_artifact.digest)
    } else {
        blocked_artifact
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
                .expect("AG121_INVALID_SETUP_OR_OBSERVATION"),
                usable_response: Some(
                    BudgetArtifact::from_serializable(
                        "rsia.model_response.artifact.v1",
                        &completed,
                    )
                    .expect("AG121_INVALID_SETUP_OR_OBSERVATION"),
                ),
                blocked_response: blocked_artifact.clone(),
                forced_block_reason: redacted_blocked_response
                    .then(|| "fixture_reader_redacted_response".into()),
            },
        )
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert_eq!(settlement.response_usable, !redacted_blocked_response);
    if redacted_blocked_response {
        assert_eq!(settlement.call.response_artifact, Some(blocked_artifact));
    }
    assert_eq!(settlement.call.state, BudgetCallState::Finalized);
    assert_eq!(settlement.call.request_artifact, Some(stored_request));
    assert_eq!(settlement.call.actual_cost_micros, Some(COST));
    assert!(!settlement.call.execution_closed);
    assert_eq!(world.totals().await, (0, COST));
    completed
}

#[tokio::test]
async fn ctrl_probe_ag121_g1_real_redacted_bound_legacy_forbidden_zero_logical_writes() {
    ag121_phase("G1", "SETUP");
    let world = World::new().await;
    let inputs = compiler_use(&world.store, 1).await;
    let bound = bound_request("ag121-g1", 1, &RUNS, "software fixture", &inputs);
    let original = BudgetArtifact::from_serializable("rsia.bound_model_request.v1", &bound)
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    settle_open(&world, &bound).await;
    ag121_fixed_revoke_and_clean(&world).await;
    let call = world.call(&bound).await;
    ag121_fixed_assert_real_redacted(&call, &original);
    ag121_phase("G1", "SNAPSHOT");
    let before = ag121_fixed_lossless_snapshot(&world.path);
    ag121_phase("G1", "TARGET");
    let result = world.broker().dispatch(bound.request.clone()).await;
    eprintln!("AG121_TARGET case=G1 result={result:?}");
    assert!(
        matches!(result, Err(Error::Forbidden)),
        "G1 legacy result: {result:?}"
    );
    assert_eq!(
        ag121_fixed_lossless_snapshot(&world.path),
        before,
        "G1 wrote after refusal"
    );
    assert_eq!(world.call(&bound).await, call);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
    ag121_phase("G1", "PASS");
}

#[tokio::test]
async fn ctrl_probe_ag121_g2_plain_bound_legacy_guard_zero_logical_writes() {
    ag121_phase("G2", "SETUP");
    let world = World::new().await;
    let inputs = compiler_use(&world.store, 1).await;
    let bound = bound_request("ag121-g2", 1, &RUNS, "software fixture", &inputs);
    settle_open(&world, &bound).await;
    let call = world.call(&bound).await;
    assert_settled_open(&call);
    ag121_phase("G2", "SNAPSHOT");
    let before = ag121_fixed_lossless_snapshot(&world.path);
    ag121_phase("G2", "TARGET");
    let result = world.broker().dispatch(bound.request.clone()).await;
    eprintln!("AG121_TARGET case=G2 result={result:?}");
    assert!(
        matches!(result, Err(Error::Forbidden)),
        "G2 legacy result: {result:?}"
    );
    assert_eq!(ag121_fixed_lossless_snapshot(&world.path), before);
    assert_eq!(world.call(&bound).await, call);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
    ag121_phase("G2", "PASS");
}

#[tokio::test]
async fn ctrl_probe_ag121_i1_same_input_other_redacted_binding_does_not_close() {
    ag121_phase("I1", "SETUP");
    let world = World::new().await;
    let inputs = compiler_use(&world.store, 1).await;
    let original = bound_request("ag121-i1", 1, &RUNS, "software fixture", &inputs);
    let artifact = BudgetArtifact::from_serializable("rsia.bound_model_request.v1", &original)
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    settle_open(&world, &original).await;
    repair_move_pointer_epoch(&world.store, 1).await;
    let moved_inputs = compiler_use(&world.store, 1).await;
    let other = BoundModelRequest::new(original.request.clone(), moved_inputs)
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert_eq!(other.request.request_id, original.request.request_id);
    assert_eq!(
        other.request.cache_key_digest,
        original.request.cache_key_digest
    );
    let other_artifact = BudgetArtifact::from_serializable("rsia.bound_model_request.v1", &other)
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert_ne!(other_artifact.digest, artifact.digest);
    ag121_fixed_revoke_and_clean(&world).await;
    let call = world.call(&original).await;
    ag121_fixed_assert_real_redacted(&call, &artifact);
    let expected = current_refusal(&world.store, &other).await;
    assert_conflict(&expected, BINDING_MISMATCH);
    ag121_phase("I1", "SNAPSHOT");
    let before = ag121_fixed_lossless_snapshot(&world.path);
    ag121_phase("I1", "TARGET");
    let result = world.replay(&other).await;
    eprintln!("AG121_TARGET case=I1 result={result:?}");
    assert_same_error(result, &expected);
    assert_eq!(
        ag121_fixed_lossless_snapshot(&world.path),
        before,
        "I1 wrote a recovery close"
    );
    assert_eq!(world.call(&original).await, call);
    assert_eq!(
        ag121_fixed_close_events(&before, &original.request.request_id),
        0
    );
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
    ag121_phase("I1", "PASS");
}

#[tokio::test]
async fn ctrl_probe_ag121_i2_same_redacted_identity_closes_exactly_once_preserves_paid_facts() {
    ag121_phase("I2", "SETUP");
    let world = World::new().await;
    let inputs = compiler_use(&world.store, 1).await;
    let bound = bound_request("ag121-i2", 1, &RUNS, "software fixture", &inputs);
    let artifact = BudgetArtifact::from_serializable("rsia.bound_model_request.v1", &bound)
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let completed = settle_open(&world, &bound).await;
    ag121_fixed_revoke_and_clean(&world).await;
    let call = world.call(&bound).await;
    ag121_fixed_assert_real_redacted(&call, &artifact);
    let expected = current_refusal(&world.store, &bound).await;
    assert_conflict(&expected, BINDING_MISMATCH);
    ag121_phase("I2", "SNAPSHOT");
    let before = ag121_fixed_lossless_snapshot(&world.path);
    ag121_phase("I2", "TARGET");
    let result = world.replay(&bound).await;
    eprintln!("AG121_TARGET case=I2 result={result:?}");
    assert_same_error(result, &expected);
    let once = ag121_fixed_lossless_snapshot(&world.path);
    ag121_fixed_assert_only_close(&before, &once, &call);
    assert_eq!(
        ag121_fixed_close_events(&once, &bound.request.request_id),
        1
    );
    let closed = world.call(&bound).await;
    assert!(closed.execution_closed);
    assert_eq!(
        closed.execution_close_reason.as_deref(),
        Some(RECOVERY_CLOSE)
    );
    assert_eq!(closed.actual_cost_micros, call.actual_cost_micros);
    assert_eq!(closed.request_artifact, call.request_artifact);
    assert_eq!(closed.transport_artifact, call.transport_artifact);
    assert_eq!(closed.response_artifact, call.response_artifact);
    assert_eq!(closed.provider_request_id, call.provider_request_id);
    assert_eq!(closed.usage_record_id, call.usage_record_id);
    assert_eq!(closed.output_digest, call.output_digest);
    world
        .broker()
        .verify_receipt(&bound.request, receipt(&completed))
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    assert_same_error(world.replay(&bound).await, &expected);
    assert_eq!(ag121_fixed_lossless_snapshot(&world.path), once);
    assert_eq!(
        ag121_fixed_close_events(&once, &bound.request.request_id),
        1
    );
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
    ag121_phase("I2", "PASS");
}

#[tokio::test]
async fn ctrl_probe_ag121_g3_valid_json_digest_malformed_redacted_legacy_forbidden() {
    for case in 0..ag121_fixed_malformed_shapes(&hash(b"shape-count")).len() {
        ag121_phase("G3", "SETUP");
        let world = World::new().await;
        let inputs = compiler_use(&world.store, 1).await;
        let bound = bound_request(
            &format!("ag121-g3-{case}"),
            1,
            &RUNS,
            "software fixture",
            &inputs,
        );
        let original = BudgetArtifact::from_serializable("rsia.bound_model_request.v1", &bound)
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
        let shapes = ag121_fixed_malformed_shapes(&original.digest);
        let (label, malformed) = &shapes[case];
        eprintln!(
            "AG121_FIXED_MALFORMED_INPUT label={} {}",
            label,
            serde_json::to_string(&malformed).expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        );
        assert_eq!(hash(malformed.body.as_bytes()), malformed.digest);
        assert!(serde_json::from_str::<Value>(&malformed.body).is_ok());
        ag121_fixed_settle_open_with_request(&world, &bound, malformed.clone()).await;
        let call = world.call(&bound).await;
        assert_eq!(call.request_artifact, Some(malformed.clone()));
        assert_eq!(call.state, BudgetCallState::Finalized);
        assert!(!call.execution_closed);
        ag121_phase("G3", "SNAPSHOT");
        let before = ag121_fixed_lossless_snapshot(&world.path);
        ag121_phase("G3", "TARGET");
        let result = world.broker().dispatch(bound.request.clone()).await;
        eprintln!("AG121_TARGET case=G3 result={result:?}");
        assert!(
            matches!(result, Err(Error::Forbidden)),
            "G3 {label}: {result:?}"
        );
        assert_eq!(
            ag121_fixed_lossless_snapshot(&world.path),
            before,
            "G3 {label} wrote"
        );
        assert_eq!(world.call(&bound).await, call);
        assert_eq!(world.provider_calls(), 1);
        assert_eq!(world.totals().await, (0, COST));
        ag121_phase("G3", "PASS");
    }
}

#[tokio::test]
async fn ctrl_probe_ag121_i3_valid_json_digest_malformed_redacted_bound_does_not_close() {
    for case in 0..ag121_fixed_malformed_shapes(&hash(b"shape-count")).len() {
        ag121_phase("I3", "SETUP");
        let world = World::new().await;
        let inputs = compiler_use(&world.store, 1).await;
        let bound = bound_request(
            &format!("ag121-i3-{case}"),
            1,
            &RUNS,
            "software fixture",
            &inputs,
        );
        let original = BudgetArtifact::from_serializable("rsia.bound_model_request.v1", &bound)
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
        let shapes = ag121_fixed_malformed_shapes(&original.digest);
        let (label, malformed) = &shapes[case];
        eprintln!(
            "AG121_FIXED_MALFORMED_INPUT label={} {}",
            label,
            serde_json::to_string(&malformed).expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        );
        assert_eq!(hash(malformed.body.as_bytes()), malformed.digest);
        assert!(serde_json::from_str::<Value>(&malformed.body).is_ok());
        ag121_fixed_settle_open_with_request(&world, &bound, malformed.clone()).await;
        let call = world.call(&bound).await;
        assert_eq!(call.request_artifact, Some(malformed.clone()));
        ag121_phase("I3", "SNAPSHOT");
        let before = ag121_fixed_lossless_snapshot(&world.path);
        ag121_phase("I3", "TARGET");
        let result = world.replay(&bound).await;
        eprintln!("AG121_TARGET case=I3 label={label} result={result:?}");
        match result {
            Ok(response) => assert_call_id_reused(response),
            Err(error) => {
                panic!("AG121_TARGET_FAILURE: I3 {label} expected CallIdReused, got {error:?}")
            }
        }
        assert_eq!(
            ag121_fixed_lossless_snapshot(&world.path),
            before,
            "I3 {label} closed/reused"
        );
        assert_eq!(world.call(&bound).await, call);
        assert_eq!(
            ag121_fixed_close_events(&before, &bound.request.request_id),
            0
        );
        assert_eq!(world.provider_calls(), 1);
        assert_eq!(world.totals().await, (0, COST));
        ag121_phase("I3", "PASS");
    }
}

fn ag121_session_redacted_artifact(original_schema: &str, original_digest: &str) -> BudgetArtifact {
    BudgetArtifact::from_serializable(
        REDACTED,
        &json!({
            "schema_version": REDACTED,
            "state": "source_revoked",
            "reason": "source_revoked",
            "original_schema": original_schema,
            "original_digest": original_digest,
        }),
    )
    .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
}

async fn ag121_session_identity_control(case: &str, redaction: Option<(&str, bool)>) {
    ag121_phase(case, "SETUP");
    let world = World::new().await;
    let inputs = current_compiler_use(&world.store, 1).await;
    let bound = current_bound_request(case, 1, &RUNS, "software fixture", &inputs);
    let original = BudgetArtifact::from_serializable(bound.schema_version.clone(), &bound)
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let same_identity = redaction
        .is_none_or(|(schema, other_digest)| schema == original.schema_version && !other_digest);
    let stored_request = match redaction {
        None => original.clone(),
        Some((schema, other_digest)) => {
            let digest = if other_digest {
                let digest = hash(b"a different registered request artifact");
                assert_ne!(digest, original.digest);
                digest
            } else {
                original.digest.clone()
            };
            ag121_session_redacted_artifact(schema, &digest)
        }
    };
    // The original G3/I3 helper keeps its request-only semantics. This new fixture
    // persists a redacted blocked response so a successful identity read returns
    // a receipt, never output, and never recovers content from the request redaction.
    let completed =
        ag121_session_settle_open_with_artifacts(&world, &bound, stored_request.clone(), true)
            .await;
    let call = world.call(&bound).await;
    assert_eq!(call.request_artifact, Some(stored_request));
    assert_eq!(call.response_usable, Some(false));
    assert_eq!(
        call.response_artifact
            .as_ref()
            .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
            .schema_version,
        REDACTED
    );
    assert!(!call.execution_closed);
    // Healthy setup establishes that I2's revoked-current early refusal cannot
    // supply this control's result. The actual broker repeats all these gates.
    let mut session = world
        .store
        .session()
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    Box::pin(bound.verify_current(&mut session, &host()))
        .await
        .expect("AG121_INVALID_SETUP: healthy current binding");
    let source_ids = bound
        .request
        .source_closure
        .iter()
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    evo_engine::evidence::validate_stored_sources(
        &mut session,
        &host(),
        &source_ids,
        bound.request.revoke_watermark,
    )
    .await
    .expect("AG121_INVALID_SETUP: healthy complete sources");
    let root = session
        .root_budget(&host(), SCOPE)
        .await
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION")
        .expect("AG121_INVALID_SETUP: healthy root budget");
    assert_eq!(root.root_budget_id, ROOT);
    drop(session);
    ag121_phase(case, "SNAPSHOT");
    let before = ag121_fixed_lossless_snapshot(&world.path);
    ag121_phase(case, "TARGET");
    let result = world.replay_current(&bound).await;
    eprintln!("AG121_TARGET case={case} result={result:?}");
    let response = match result {
        Ok(response) => response,
        Err(error) => panic!("AG121_TARGET_FAILURE: {case} got {error:?}"),
    };
    let once = ag121_fixed_lossless_snapshot(&world.path);
    if same_identity {
        assert!(
            matches!(
                &response,
                ModelResponse::Rejected {
                    kind: ModelRejectionKind::CancelledAfterDispatch,
                    dispatch: RejectedDispatch::Dispatched { .. },
                    ..
                }
            ),
            "{case}: same identity was refused or content returned: {response:?}"
        );
        assert_eq!(receipt(&response), receipt(&completed));
        ag121_fixed_assert_only_close(&before, &once, &call);
        assert_eq!(
            ag121_fixed_close_events(&once, &bound.request.request_id),
            1
        );
        let mut expected_call = call.clone();
        expected_call.execution_closed = true;
        expected_call.execution_closed_at = Some(NOW);
        expected_call.execution_close_reason = Some(RECOVERY_CLOSE.into());
        assert_eq!(world.call(&bound).await, expected_call);
    } else {
        assert_call_id_reused(response);
        assert_eq!(once, before, "{case}: mismatched identity closed/reused");
        assert_eq!(world.call(&bound).await, call);
        assert_eq!(
            ag121_fixed_close_events(&once, &bound.request.request_id),
            0
        );
    }
    // Receipt syntax and paid ledger identity remain healthy in every case.
    // A mismatched request intentionally leaves execution open, so its receipt
    // cannot pass the production consumption gate until a qualified close.
    bound
        .request
        .validate()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let original_receipt = receipt(&completed).clone();
    original_receipt
        .validate()
        .expect("AG121_INVALID_SETUP_OR_OBSERVATION");
    let observed_call = world.call(&bound).await;
    assert_eq!(root.root_budget_id, original_receipt.root_budget_id);
    assert_eq!(observed_call.billing_scope, SCOPE);
    assert_eq!(observed_call.namespace, bound.request.namespace);
    assert_eq!(observed_call.call_id, bound.request.request_id);
    assert_eq!(observed_call.call_id, original_receipt.call_id);
    assert_eq!(
        observed_call.actual_input_digest,
        bound.request.cache_key_digest
    );
    assert_eq!(observed_call.state, BudgetCallState::Finalized);
    assert_eq!(
        observed_call.dispatch_id.as_deref(),
        Some(original_receipt.dispatch_id.as_str())
    );
    assert_eq!(
        observed_call.provider_request_id.as_deref(),
        Some(original_receipt.provider_request_id.as_str())
    );
    assert_eq!(
        observed_call.usage_record_id.as_deref(),
        Some(original_receipt.usage_record_id.as_str())
    );
    let ModelResponse::Completed {
        output_digest: original_output_digest,
        ..
    } = &completed
    else {
        panic!("AG121_INVALID_SETUP: original fixture response is not completed");
    };
    assert_eq!(
        observed_call.output_digest.as_deref(),
        Some(original_output_digest.as_str())
    );
    assert_eq!(
        original_receipt.provenance,
        ModelExecutionProvenance::Fixture
    );
    assert_eq!(
        observed_call.execution_provenance,
        Some(BudgetExecutionProvenance::Fixture)
    );
    assert!(observed_call.response_artifact.is_some());
    assert_eq!(observed_call.response_artifact, call.response_artifact);
    assert_eq!(observed_call.actual_cost_micros, Some(COST));
    assert_eq!(observed_call.actual_cost_micros, call.actual_cost_micros);
    assert_eq!(observed_call.actual_currency, call.actual_currency);
    assert_eq!(
        observed_call.actual_pricing_version,
        call.actual_pricing_version
    );
    assert_eq!(
        observed_call.actual_currency.as_deref(),
        Some(root.currency.as_str())
    );
    assert_eq!(
        observed_call.actual_pricing_version.as_deref(),
        Some(root.pricing_version.as_str())
    );
    assert_eq!(observed_call.execution_closed, same_identity);
    if !same_identity {
        assert_eq!(observed_call, call);
        assert!(!observed_call.execution_closed);
        assert_eq!(observed_call.execution_closed_at, None);
        assert_eq!(observed_call.execution_close_reason, None);
    }
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
    let qualification = world
        .broker()
        .verify_receipt(&bound.request, &original_receipt)
        .await;
    eprintln!("AG121_RECEIPT_QUALIFICATION case={case} result={qualification:?}");
    match (same_identity, qualification) {
        (true, Ok(())) => {}
        (false, Err(Error::Conflict(reason))) => assert_eq!(
            reason, "model execution receipt does not match persistent broker facts",
            "{case}: unexpected receipt refusal reason"
        ),
        (_, other) => {
            panic!("AG121_TARGET_FAILURE: {case} unexpected receipt qualification {other:?}")
        }
    }
    assert_eq!(
        ag121_fixed_lossless_snapshot(&world.path),
        once,
        "{case}: receipt verification changed logical state"
    );
    assert_eq!(world.call(&bound).await, observed_call);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
    let replay = world.replay_current(&bound).await;
    eprintln!("AG121_TARGET case={case} replay={replay:?}");
    match replay {
        Ok(response) if same_identity => {
            assert!(
                matches!(
                    &response,
                    ModelResponse::Rejected {
                        kind: ModelRejectionKind::CancelledAfterDispatch,
                        dispatch: RejectedDispatch::Dispatched { .. },
                        ..
                    }
                ),
                "{case}: replay returned content or changed refusal: {response:?}"
            );
            assert_eq!(receipt(&response), receipt(&completed));
        }
        Ok(response) => assert_call_id_reused(response),
        Err(error) => panic!("AG121_TARGET_FAILURE: {case} replay got {error:?}"),
    }
    assert_eq!(ag121_fixed_lossless_snapshot(&world.path), once);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.totals().await, (0, COST));
    ag121_phase(case, "PASS");
}

#[tokio::test]
async fn ag121_session_same_redacted_identity_reaches_read_and_preserves_paid_no_output() {
    ag121_session_identity_control(
        "ag121-session-same",
        Some((evo_engine::model::CURRENT_BOUND_MODEL_SCHEMA, false)),
    )
    .await;
}

#[tokio::test]
async fn ag121_session_different_valid_original_schema_refuses_zero_logical_writes() {
    ag121_session_identity_control(
        "ag121-session-schema",
        Some(("rsia.model_request.artifact.v1", false)),
    )
    .await;
}

#[tokio::test]
async fn ag121_session_different_lowercase_original_digest_refuses_zero_logical_writes() {
    ag121_session_identity_control(
        "ag121-session-digest",
        Some((evo_engine::model::CURRENT_BOUND_MODEL_SCHEMA, true)),
    )
    .await;
}

#[tokio::test]
async fn ag121_session_plain_identity_retains_original_fingerprint_guard() {
    ag121_session_identity_control("ag121-session-plain", None).await;
}
