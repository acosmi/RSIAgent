//! AG-038 (E08/§11, E04) at the engine layer: a model or development call over a
//! source that has been revoked is neither reserved nor sent, and what was
//! already sent stays on the books.
//!
//! Plan §11 writes the revocation first, so new use is refused at once, and
//! §11.3 and §11.5 block reads, derivation and outbound dispatch before the
//! cleanup runs ("已dispatch的晚到结果不可恢复有效性，费用仍对账"). The budget
//! ledger used to look at no revocation: a call over a revoked run was reserved,
//! and one reserved before the revocation was dispatched after it (the cleanup
//! only redacts the ledger copy; the broker sends the request it holds in memory).
//!
//! These tests drive the production callers of the ledger, the persistent model
//! broker and the registered development runner, with a counting transport (the
//! provider is never real) and the real SQLite store.
//!
//! The Q03 cases also register a real strict E16 upstream source, compare
//! lossless whole-database snapshots on refusal, and keep late charges while
//! refusing and redacting the revoked output.

#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use async_trait::async_trait;
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
    OptimizationTrace, TraceOutcome,
};
use evo_core::skill_edit::EvidenceRef;
use evo_core::{Context, Error, Result, Role, fingerprint, hash};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::curriculum_profiles::RegisteredPureFunctionProfileV1;
use evo_engine::development::{
    DevelopmentControlV1, DevelopmentEvidenceScope, DevelopmentSide, DevelopmentTaskSpecV1,
    RegisteredDevelopmentRunner, RegisteredTargetInputV1, execution_budget_call_id,
    execution_receipt_id, load_execution_receipt, register_development_control,
    registered_runner_digest,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_engine::model::{
    ModelExecutionProvenance, ModelPort, ModelRejectionKind, ModelResponse, RejectedDispatch,
};
use evo_engine::optimization::{DevRunner, DevelopmentRunRequest};
use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallRecord, BudgetCallReservation, BudgetCallState, BudgetStage,
    RootBudgetAuthorization,
};
use evo_storage::lifecycle::{CleanupState, CleanupStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// The persistent model broker (fixtures copied from tests/broker.rs)
// ---------------------------------------------------------------------------

const BROKER_NAMESPACE: &str = "ns-a";
const BROKER_SCOPE: &str = "scope-1";
/// The run the broker tests revoke, and one that stays live.
const RUN: &str = "run-r";
const LIVE: &str = "run-live";
const MODEL_REQUEST_SCHEMA: &str = "rsia.model_request.artifact.v1";

#[derive(Clone)]
struct CountingTransport {
    calls: Arc<AtomicUsize>,
    /// Revokes this run while the provider request is in flight.
    revoke_during_call: Option<(Store, &'static str)>,
}

impl CountingTransport {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                calls: calls.clone(),
                revoke_during_call: None,
            },
            calls,
        )
    }
}

#[async_trait]
impl ModelTransport for CountingTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((store, source)) = &self.revoke_during_call {
            let admin = Context::new(BROKER_NAMESPACE, "admin", Role::Admin)?;
            LifecycleCoordinator::revoke_source(&admin, store, *source, "privacy", 12).await?;
        }
        Ok(TransportCompletion {
            response_id: format!("response-{}", request.request_id),
            provider_request_id: format!("provider-{}", request.request_id),
            usage_record_id: format!("usage-{}", request.request_id),
            actual_model_digest: request.model_digest.clone(),
            output: "fixture output".into(),
            actual_cost_micros: 10,
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
        })
    }
}

fn model_request(id: &str, sources: &[&str]) -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: id.into(),
            namespace: BROKER_NAMESPACE.into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "episode-1".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: hash(b"parent-v1"),
            bundle_digest: hash(b"bundle-v1"),
            source_closure: sources
                .iter()
                .map(|source| EvidenceRef {
                    id: (*source).into(),
                    digest: hash(source.as_bytes()),
                })
                .collect(),
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

fn broker_config() -> BrokerConfig {
    BrokerConfig {
        billing_scope: BROKER_SCOPE.into(),
        actor: "trusted-broker".into(),
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        max_cost_micros: 20,
        lease_seconds: 60,
    }
}

fn broker_admin() -> Context {
    Context::new(BROKER_NAMESPACE, "admin", Role::Admin).unwrap()
}

fn broker_host() -> Context {
    Context::new(BROKER_NAMESPACE, "trusted-broker", Role::Host).unwrap()
}

/// A store with the root authorized and the runs `RUN` and `LIVE` stored.
async fn broker_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    store
        .authorize_root_budget(
            &broker_admin(),
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: BROKER_SCOPE.into(),
                allowed_namespaces: vec![BROKER_NAMESPACE.into()],
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-1".into(),
                authorization_receipt_digest: hash(b"admin-authorization"),
                per_call_cap_micros: 20,
                total_limit_micros: 100,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let host = broker_host();
    for id in [RUN, LIVE] {
        store_trace_authority(
            &store,
            &host,
            &StoredTraceAuthority {
                schema_version: "rsia.optimization.source.v1".into(),
                record: StoredRunRecord {
                    id: id.into(),
                    body: id.as_bytes().to_vec(),
                    parent_family: format!("family-{id}"),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                trace: OptimizationTrace {
                    run_id: id.into(),
                    parent_family: format!("family-{id}"),
                    source_digest: hash(id.as_bytes()),
                    purpose: Purpose::Development,
                    outcome: TraceOutcome::Success,
                    diagnosis: None,
                    excerpt: id.into(),
                    seed: 1,
                },
                excerpt_start: 0,
                excerpt_end: id.len(),
            },
        )
        .await
        .unwrap();
    }
    (dir, store)
}

fn broker_over(
    store: &Store,
    transport: CountingTransport,
) -> PersistentModelBroker<CountingTransport> {
    PersistentModelBroker::with_clock(store.clone(), transport, broker_config(), Arc::new(|| 10))
        .unwrap()
}

async fn broker_call(store: &Store, call_id: &str) -> Option<BudgetCallRecord> {
    store
        .budget_call(&broker_host(), BROKER_SCOPE, call_id)
        .await
        .unwrap()
}

async fn micros(store: &Store, scope: &str, ctx: &Context) -> (i64, i64) {
    let root = store.root_budget(ctx, scope).await.unwrap().unwrap();
    (root.reserved_micros, root.spent_micros)
}

/// Steps the cleanup job of `status` until it is `Complete`.
async fn complete_cleanup(store: &Store, mut status: CleanupStatus) {
    for now in 21..400 {
        if status.state == CleanupState::Complete {
            return;
        }
        status =
            LifecycleCoordinator::continue_cleanup(&broker_admin(), store, &status.job_id, 8, now)
                .await
                .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
}

fn assert_rejected_before_dispatch(
    response: &ModelResponse,
    expected: ModelRejectionKind,
    what: &str,
) {
    match response {
        ModelResponse::Rejected {
            kind,
            dispatch: RejectedDispatch::NotDispatched,
            ..
        } if *kind == expected => {}
        other => panic!("{what}: expected {expected:?} before dispatch, got {other:?}"),
    }
}

/// Only `begin_revoke` has run, then a model call over the revoked run is made:
/// the broker answers `Unauthorized` before any dispatch, the transport is never
/// called, and no reservation exists.
#[tokio::test]
async fn a_model_call_over_a_revoked_source_is_rejected_and_never_reaches_the_transport() {
    let (_dir, store) = broker_store().await;
    LifecycleCoordinator::revoke_source(&broker_admin(), &store, RUN, "privacy", 5)
        .await
        .unwrap();
    let (transport, calls) = CountingTransport::new();
    let broker = broker_over(&store, transport);

    // the revoked run alone, and among live ones
    for (id, sources) in [("request-1", vec![RUN]), ("request-2", vec![LIVE, RUN])] {
        let request = model_request(id, &sources);
        for attempt in ["first", "retry"] {
            let response = broker.dispatch(request.clone()).await.unwrap();
            assert_rejected_before_dispatch(
                &response,
                ModelRejectionKind::Unauthorized,
                &format!("{id} {attempt}"),
            );
        }
        assert!(broker_call(&store, id).await.is_none(), "{id} was reserved");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the provider was called");
    assert_eq!(micros(&store, BROKER_SCOPE, &broker_admin()).await, (0, 0));

    // The control: the same broker sends a call over the live run only.
    let live = model_request("request-live", &[LIVE]);
    match broker.dispatch(live).await.unwrap() {
        ModelResponse::Completed { .. } => {}
        other => panic!("a live source must complete: {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(micros(&store, BROKER_SCOPE, &broker_admin()).await, (0, 10));
}

/// Reserved over a live run, revoked before the dispatch: the broker finds its
/// reservation, the dispatch is refused as Unauthorized/source_revoked, the reservation is released (the quota
/// comes back), and the provider is never called, now or on a retry.
#[tokio::test]
async fn a_revoke_between_the_reservation_and_the_dispatch_releases_the_quota_and_sends_nothing() {
    let (_dir, store) = broker_store().await;
    let request = model_request("request-1", &[RUN]);
    // Exactly the reservation the broker would have made for this request.
    let reserved = store
        .reserve_budget_call_with_sources(
            &broker_host(),
            &BudgetCallReservation {
                billing_scope: BROKER_SCOPE.into(),
                call_id: request.request_id.clone(),
                dispatch_group_id: request.episode_id.clone(),
                stage: BudgetStage::Reflection,
                actual_input_digest: request.cache_key_digest.clone(),
                request_artifact: Some(
                    BudgetArtifact::from_serializable(MODEL_REQUEST_SCHEMA, &request).unwrap(),
                ),
                max_cost_micros: 20,
                lease_token: "lease-reserved-before-the-revoke".into(),
                lease_until: 70,
                now: 10,
            },
            &[RUN.to_string()],
        )
        .await
        .unwrap();
    assert_eq!(reserved.state, BudgetCallState::Reserved);
    assert_eq!(micros(&store, BROKER_SCOPE, &broker_admin()).await, (20, 0));

    LifecycleCoordinator::revoke_source(&broker_admin(), &store, RUN, "privacy", 11)
        .await
        .unwrap();
    let (transport, calls) = CountingTransport::new();
    let broker = broker_over(&store, transport);
    for attempt in ["first", "retry"] {
        let response = broker.dispatch(request.clone()).await.unwrap();
        assert_rejected_before_dispatch(&response, ModelRejectionKind::Unauthorized, attempt);
        match &response {
            ModelResponse::Rejected { reason, .. } => {
                assert_eq!(reason, "broker_pre_dispatch_v1.source_revoked");
            }
            other => panic!("{attempt}: expected source refusal, got {other:?}"),
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "{attempt}: provider called"
        );
    }
    let call = broker_call(&store, "request-1").await.unwrap();
    assert_eq!(call.state, BudgetCallState::Reserved);
    assert_eq!(call.dispatch_id, None);
    assert_eq!(call.dispatched_at, None);
    assert_eq!(call.terminal_reason, None);
    assert_eq!(call.actual_cost_micros, None);
    assert_eq!(call.request_artifact, reserved.request_artifact);
    // A revocation refusal preserves the original reservation and all paid facts.
    assert_eq!(micros(&store, BROKER_SCOPE, &broker_admin()).await, (20, 0));
}

/// The revocation lands while the provider request is in flight. The call was
/// dispatched, so its real cost is booked, before and after the cleanup, while
/// the request content the ledger holds is redacted by the cleanup (§11: money and
/// history are not reversed by a deletion).
#[tokio::test]
async fn a_revoke_during_the_provider_await_still_books_the_real_cost() {
    let (_dir, store) = broker_store().await;
    let (mut transport, calls) = CountingTransport::new();
    transport.revoke_during_call = Some((store.clone(), RUN));
    let broker = broker_over(&store, transport);
    let request = model_request("request-1", &[RUN]);

    // The response of a call that was in flight when its source was revoked is not
    // judged here; the accounting is.
    broker.dispatch(request.clone()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let books = |call: &BudgetCallRecord| {
        assert_eq!(call.state, BudgetCallState::Finalized);
        assert_eq!(call.actual_cost_micros, Some(10));
        assert_eq!(call.actual_currency.as_deref(), Some("USD"));
        assert!(call.dispatch_id.is_some());
    };
    books(&broker_call(&store, "request-1").await.unwrap());
    assert_eq!(micros(&store, BROKER_SCOPE, &broker_admin()).await, (0, 10));

    // A repeat of the request neither sends again nor un-spends.
    broker.dispatch(request).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Run the revocation cleanup to completion: the spend stays.
    let status = LifecycleCoordinator::revoke_source(&broker_admin(), &store, RUN, "privacy", 20)
        .await
        .unwrap();
    complete_cleanup(&store, status).await;
    let call = broker_call(&store, "request-1").await.unwrap();
    books(&call);
    assert_eq!(
        call.request_artifact.unwrap().schema_version,
        "rsia.redacted.v1",
        "the cleanup redacts the request content"
    );
    assert_eq!(micros(&store, BROKER_SCOPE, &broker_admin()).await, (0, 10));
}

/// The tombstone outlives the cleanup: once the cleanup is `Complete` (the run is
/// deleted), a new call over the revoked run is still refused before any dispatch.
#[tokio::test]
async fn a_new_model_call_is_still_refused_after_the_cleanup_has_completed() {
    let (_dir, store) = broker_store().await;
    let status = LifecycleCoordinator::revoke_source(&broker_admin(), &store, RUN, "privacy", 5)
        .await
        .unwrap();
    complete_cleanup(&store, status).await;
    let (transport, calls) = CountingTransport::new();
    let broker = broker_over(&store, transport);

    let response = broker
        .dispatch(model_request("request-1", &[RUN]))
        .await
        .unwrap();
    assert_rejected_before_dispatch(
        &response,
        ModelRejectionKind::Unauthorized,
        "after the cleanup",
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(broker_call(&store, "request-1").await.is_none());
    assert_eq!(micros(&store, BROKER_SCOPE, &broker_admin()).await, (0, 0));
}

// ---------------------------------------------------------------------------
// The registered development runner (fixtures copied from
// tests/development_receipts.rs)
// ---------------------------------------------------------------------------

const NAMESPACE: &str = "n";
const SCOPE: &str = "dev-scope";
const CONTROL_ID: &str = "dev-control";
const EPISODE: &str = "episode-1";

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn grader_spec() -> FixedGraderSpec {
    FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    }
}

fn authority(id: &str, family: &str) -> StoredTraceAuthority {
    let body = format!("trusted-run-{id}");
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.as_bytes().to_vec(),
            parent_family: family.into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: family.into(),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.clone(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    }
}

fn tasks() -> Vec<DevelopmentTaskSpecV1> {
    vec![
        DevelopmentTaskSpecV1::registered(
            "task-a",
            "family-a",
            RegisteredTargetInputV1::ClampI64 {
                value: -2,
                min: -1,
                max: 1,
            },
        )
        .unwrap(),
        DevelopmentTaskSpecV1::registered(
            "task-b",
            "family-b",
            RegisteredTargetInputV1::ClampI64 {
                value: 5,
                min: 0,
                max: 3,
            },
        )
        .unwrap(),
    ]
}

fn control() -> DevelopmentControlV1 {
    let profile = RegisteredPureFunctionProfileV1::clamp_i64();
    let grader = grader_spec();
    let mut control = DevelopmentControlV1 {
        schema_version: DevelopmentControlV1::SCHEMA.into(),
        id: CONTROL_ID.into(),
        namespace: NAMESPACE.into(),
        billing_scope: SCOPE.into(),
        root_budget_id: "dev-root".into(),
        executor_actor: "dev-executor".into(),
        grader_actor: "dev-grader".into(),
        proposer_actor: "optimizer".into(),
        manifest_id: "dev-manifest".into(),
        manifest_digest: String::new(),
        tasks: tasks(),
        environment_digest: d("environment"),
        grader_digest: fingerprint(&grader).unwrap(),
        fixed_grader: grader,
        oracle_digest: profile.oracle_digest,
        target_digest: profile.target_digest,
        runner_digest: registered_runner_digest().unwrap(),
        rules_digest: d("rules"),
        tools_digest: d("tools"),
        source_ids: vec!["run-a".into(), "run-b".into()],
        evidence_scope: DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
        created_at_unix_seconds: 1,
    };
    control.manifest_digest = control.manifest().unwrap().digest;
    control.validate().unwrap();
    control
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    admin: Context,
    executor: Context,
    grader: Context,
    control: DevelopmentControlV1,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("development.sqlite3"))
            .await
            .unwrap();
        let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
        let host = Context::new(NAMESPACE, "host", Role::Host).unwrap();
        let executor = Context::new(NAMESPACE, "dev-executor", Role::Worker).unwrap();
        let grader = Context::new(NAMESPACE, "dev-grader", Role::Evaluator).unwrap();
        for (id, family) in [("run-a", "family-a"), ("run-b", "family-b")] {
            store_trace_authority(&store, &host, &authority(id, family))
                .await
                .unwrap();
        }
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&admin, "initial-watermark")
            .await
            .unwrap();
        session.commit().await.unwrap();
        store
            .authorize_root_budget(
                &admin,
                &RootBudgetAuthorization {
                    root_budget_id: "dev-root".into(),
                    billing_scope: SCOPE.into(),
                    allowed_namespaces: vec![NAMESPACE.into()],
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    payment_subject: "fixture-only".into(),
                    authorization_receipt_digest: d("admin-budget-receipt"),
                    per_call_cap_micros: 10,
                    total_limit_micros: 1_000,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
        let control = control();
        let candidate = crate::trusted_compiler_inputs::software_candidate_current(
            &store,
            &executor,
            &control.environment_digest,
            "candidate-bundle",
            1,
        )
        .await
        .unwrap();
        crate::trusted_compiler_inputs::persist_bundle_current(
            &store,
            &executor,
            &candidate,
            &control.environment_digest,
            1,
        )
        .await
        .unwrap();
        register_development_control(&admin, &store, control.clone())
            .await
            .unwrap();
        Self {
            _dir: dir,
            store,
            admin,
            executor,
            grader,
            control,
        }
    }

    fn runner_with_clock(
        &self,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> RegisteredDevelopmentRunner {
        RegisteredDevelopmentRunner::with_clock(
            self.store.clone(),
            self.executor.clone(),
            self.grader.clone(),
            CONTROL_ID,
            600,
            clock,
        )
        .unwrap()
    }

    fn runner(&self) -> RegisteredDevelopmentRunner {
        self.runner_with_clock(Arc::new(|| 100))
    }

    fn request(&self) -> DevelopmentRunRequest {
        DevelopmentRunRequest {
            request_id: "dev-request-1".into(),
            namespace: NAMESPACE.into(),
            purpose: Purpose::Development,
            episode_id: EPISODE.into(),
            step: 1,
            attempt: 1,
            manifest: self.control.manifest().unwrap(),
            parent_bundle_digest: d("parent-bundle"),
            candidate_bundle_digest: trusted_compiler_inputs::software_candidate_content_current(
                NAMESPACE,
                &self.control.environment_digest,
                "candidate-bundle",
            )
            .digest,
            environment_digest: self.control.environment_digest.clone(),
            grader_digest: self.control.grader_digest.clone(),
            rules_digest: self.control.rules_digest.clone(),
            tools_digest: self.control.tools_digest.clone(),
            revoke_watermark: 1,
            idempotency_key: "dev-request-1-idem".into(),
        }
    }

    async fn revoke(&self, source: &str) {
        LifecycleCoordinator::revoke_source(&self.admin, &self.store, source, "privacy", 150)
            .await
            .unwrap();
    }

    /// The budget rows of the episode.
    async fn calls(&self) -> Vec<BudgetCallRecord> {
        let mut session = self.store.session().await.unwrap();
        let calls = session
            .budget_calls_for_group(&self.admin, SCOPE, EPISODE)
            .await
            .unwrap();
        session.commit().await.unwrap();
        calls
    }

    /// No execution receipt was issued for any task and side of the request.
    async fn assert_no_execution_receipt(&self, request: &DevelopmentRunRequest) {
        for task in ["task-a", "task-b"] {
            for side in [DevelopmentSide::Parent, DevelopmentSide::Candidate] {
                let receipt_id = execution_receipt_id(&request.request_id, task, side).unwrap();
                assert!(
                    matches!(
                        load_execution_receipt(&self.store, &self.admin, &receipt_id).await,
                        Err(Error::NotFound)
                    ),
                    "an execution receipt exists for {task} {side:?}"
                );
            }
        }
    }
}

/// The runner reads the revocation when it starts (`require_live_sources`), so a
/// run over a revoked source is refused with `Forbidden` before any budget row.
/// This holds before and after this change; the two tests below close the windows
/// that check leaves open.
#[tokio::test]
async fn the_runner_refuses_a_revoked_source_and_issues_no_receipt() {
    let fixture = Fixture::new().await;
    fixture.revoke("run-a").await;
    let request = fixture.request();

    let result = fixture.runner().run(request.clone()).await;
    assert!(
        matches!(result, Err(Error::Forbidden)),
        "expected Forbidden, got {result:?}"
    );
    assert!(fixture.calls().await.is_empty(), "a budget row was written");
    assert_eq!(micros(&fixture.store, SCOPE, &fixture.admin).await, (0, 0));
    fixture.assert_no_execution_receipt(&request).await;
}

/// A runner clock that revokes `run-a` the `on_read`-th time it is read (counting
/// from 0) and always reads 100. The runner reads its clock in `execute_side` at
/// two points a revocation can land between: first for the reservation time, right
/// after the start check, and then for the dispatch fence, right after the
/// reservation. The revocation is done by the Admin on the same store, with no
/// connection held at either point (the runtime is multi-threaded so the
/// revocation can run to completion while the clock call blocks its worker).
///
/// The two read indexes are today's order of the runner's clock reads. If the
/// runner reads its clock earlier, the tests below must follow it, or the first of
/// them would stop reaching the reservation gate (it would still end in
/// `Forbidden`, from the start check).
fn revoking_clock(
    fixture: &Fixture,
    on_read: usize,
) -> (Arc<dyn Fn() -> i64 + Send + Sync>, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let handle = tokio::runtime::Handle::current();
    let store = fixture.store.clone();
    let admin = fixture.admin.clone();
    let counter = reads.clone();
    let clock = move || {
        if counter.fetch_add(1, Ordering::SeqCst) == on_read {
            tokio::task::block_in_place(|| {
                handle.block_on(async {
                    LifecycleCoordinator::revoke_source(&admin, &store, "run-a", "privacy", 150)
                        .await
                        .unwrap();
                });
            });
        }
        100
    };
    (Arc::new(clock), reads)
}

async fn assert_revoked(fixture: &Fixture, source: &str) {
    let mut session = fixture.store.session().await.unwrap();
    let tombstone = session
        .get::<serde_json::Value>(&fixture.admin, "tombstone", source)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(tombstone.is_some(), "{source} was not revoked");
}

/// The revocation lands after the start check and before the reservation. The
/// ledger refuses the reservation itself: no budget row, no quota held, no
/// receipt. (Before this change the call was reserved, dispatched and settled,
/// and only the receipt check failed it afterwards.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoke_after_the_start_check_is_refused_at_the_reservation() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let (clock, reads) = revoking_clock(&fixture, 0);

    let result = fixture.runner_with_clock(clock).run(request.clone()).await;
    assert!(
        reads.load(Ordering::SeqCst) >= 1,
        "the clock was never read"
    );
    assert_revoked(&fixture, "run-a").await;
    assert!(
        matches!(result, Err(Error::Forbidden)),
        "expected Forbidden, got {result:?}"
    );
    assert!(
        fixture.calls().await.is_empty(),
        "a call over a revoked source was reserved: {:?}",
        fixture.calls().await
    );
    assert_eq!(micros(&fixture.store, SCOPE, &fixture.admin).await, (0, 0));
    fixture.assert_no_execution_receipt(&request).await;
}

/// The revocation lands after the reservation and before the dispatch. The
/// dispatch is refused (`Cancelled`), its original 1 micro remains reserved,
/// and no receipt or release event is issued. The in-process target may
/// already have executed before begin; no budget dispatch or settlement occurs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoke_after_the_reservation_is_refused_at_the_dispatch() {
    let fixture = Fixture::new().await;
    let request = fixture.request();
    let (clock, reads) = revoking_clock(&fixture, 1);

    let result = fixture.runner_with_clock(clock).run(request.clone()).await;
    assert!(
        reads.load(Ordering::SeqCst) >= 2,
        "the clock was read too few times"
    );
    assert_revoked(&fixture, "run-a").await;
    assert!(
        matches!(result, Err(Error::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
    let calls = fixture.calls().await;
    assert_eq!(calls.len(), 1, "{calls:?}");
    let expected =
        execution_budget_call_id(&request.request_id, "task-a", DevelopmentSide::Parent).unwrap();
    assert_eq!(calls[0].call_id, expected);
    assert_eq!(calls[0].state, BudgetCallState::Reserved);
    assert_eq!(calls[0].terminal_reason.as_deref(), None);
    assert_eq!(micros(&fixture.store, SCOPE, &fixture.admin).await, (1, 0));
    assert_eq!(calls[0].dispatch_id, None, "the call was dispatched");
    assert_eq!(calls[0].actual_cost_micros, None);
    fixture.assert_no_execution_receipt(&request).await;
}

fn q03_lossless_snapshot(path: &std::path::Path) -> serde_json::Value {
    let output = std::process::Command::new("python3").arg("-c").arg(r#"
import json,sqlite3,struct,sys
c=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
def cell(v):
    if v is None: return ['null']
    if isinstance(v,int): return ['integer',str(v)]
    if isinstance(v,float): return ['real',struct.pack('>d',v).hex()]
    if isinstance(v,bytes): return ['blob',v.hex()]
    return ['text',v.encode('utf-8').hex()]
out={}
for (name,) in c.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    q='"'+name.replace('"','""')+'"'
    rows=[[cell(v) for v in r] for r in c.execute('SELECT * FROM '+q)]
    out[name]=sorted(rows,key=lambda r:json.dumps(r,separators=(',',':')))
print(json.dumps(out,separators=(',',':')))
"#).arg(path).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn q03_source_graph() -> (tempfile::TempDir, Store, String) {
    use evo_engine::import::{
        IMPORT_REGISTRATION_SCHEMA, ImportRegistrationRequest, ImportRetentionScope,
        ImportSourceSpec, PersistentImportService, SourceFormat,
    };
    let (dir, store) = broker_store().await;
    let bytes = b"{\"type\":\"user\",\"message\":{\"content\":\"local fixture\"}}\n";
    let path = dir.path().join("q03-history.jsonl");
    tokio::fs::write(&path, bytes).await.unwrap();
    let selection = PersistentImportService::new(store.clone())
        .register(
            &broker_admin(),
            ImportRegistrationRequest {
                schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                request_key: "q03-upstream".into(),
                roots: vec![dir.path().to_string_lossy().into_owned()],
                purpose: Purpose::Development,
                allow_model_excerpts: false,
                outbound_authorized: false,
                retention_scope: ImportRetentionScope::LocalPrivate,
                sources: vec![ImportSourceSpec {
                    source_id: "q03-history".into(),
                    path: path.to_string_lossy().into_owned(),
                    reader: SourceFormat::ClaudeFixture,
                    expected_digest: hash(bytes),
                }],
            },
        )
        .await
        .unwrap();
    let upstream = selection.payload.import_source_ids[0].clone();
    let mut session = store.session().await.unwrap();
    session
        .put_edge(&broker_admin(), "run", RUN, "artifact", &upstream)
        .await
        .unwrap();
    assert!(
        session
            .registered_revocable_source(
                &broker_admin(),
                evo_core::evidence::RevocableSourceKind::TrustedRun,
                RUN
            )
            .await
            .unwrap()
    );
    assert!(
        session
            .registered_revocable_source(
                &broker_admin(),
                evo_core::evidence::RevocableSourceKind::ImportSource,
                &upstream
            )
            .await
            .unwrap()
    );
    session.commit().await.unwrap();
    (dir, store, upstream)
}

fn q03_reservation(id: &str) -> BudgetCallReservation {
    let request = model_request(id, &[RUN]);
    BudgetCallReservation {
        billing_scope: BROKER_SCOPE.into(),
        call_id: id.into(),
        dispatch_group_id: request.episode_id.clone(),
        stage: BudgetStage::Reflection,
        actual_input_digest: request.cache_key_digest.clone(),
        request_artifact: Some(
            BudgetArtifact::from_serializable(MODEL_REQUEST_SCHEMA, &request).unwrap(),
        ),
        max_cost_micros: 20,
        lease_token: format!("q03-lease-{id}"),
        lease_until: 70,
        now: 10,
    }
}

fn q03_fence(call: &BudgetCallRecord) -> evo_storage::budget::BudgetCallFence {
    evo_storage::budget::BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now: 12,
    }
}

async fn q03_revoke_upstream(store: &Store, id: &str) {
    evo_storage::lifecycle::LifecycleStore::begin_revoke(
        &broker_admin(),
        store,
        evo_storage::lifecycle::TypedObjectRef {
            kind: "artifact".into(),
            id: id.into(),
        },
        "q03 fixture",
        11,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn q03_legacy_reserve_refuses_revoked_transitive_source_without_writes() {
    let (dir, store, upstream) = q03_source_graph().await;
    let healthy = store
        .reserve_budget_call_with_sources(
            &broker_host(),
            &q03_reservation("q03-health"),
            &[RUN.into()],
        )
        .await
        .unwrap();
    store
        .release_undispatched_budget_call(&broker_host(), &q03_fence(&healthy), "healthy-control")
        .await
        .unwrap();
    // Full authority is required even when no tombstone exists.
    for source in ["q03-missing-run", "q03-bare-run", "q03-claimed-run"] {
        if source != "q03-missing-run" {
            let value = if source == "q03-bare-run" {
                serde_json::json!({"id":source})
            } else {
                serde_json::json!({"id":source,"schema_version":"rsia.optimization.source.v1","task_origin":"trusted_run","execution_attestation":"trusted_host"})
            };
            let mut session = store.session().await.unwrap();
            session
                .put(
                    &broker_admin(),
                    "run",
                    source,
                    broker_admin().actor(),
                    &value,
                )
                .await
                .unwrap();
            session.commit().await.unwrap();
        }
        let mut request = q03_reservation(&format!("call-{source}"));
        let model = model_request(&request.call_id, &[source]);
        request.actual_input_digest = model.cache_key_digest.clone();
        request.request_artifact =
            Some(BudgetArtifact::from_serializable(MODEL_REQUEST_SCHEMA, &model).unwrap());
        let path = dir.path().join("rsia.sqlite3");
        let before = q03_lossless_snapshot(&path);
        assert!(matches!(
            store
                .reserve_budget_call_with_sources(&broker_host(), &request, &[source.into()])
                .await,
            Err(Error::Forbidden)
        ));
        let mut session = store.session().await.unwrap();
        assert_eq!(
            session
                .reserve_budget_call_with_sources_typed(&broker_host(), &request, &[source.into()])
                .await
                .unwrap(),
            Err(evo_storage::budget::BudgetPreDispatchRefusal::SourceRevoked)
        );
        session.commit().await.unwrap();
        assert_eq!(q03_lossless_snapshot(&path), before);
    }
    for kind in ["run", "artifact"] {
        let (fault_dir, fault_store, _) = q03_source_graph().await;
        let admin = broker_admin();
        let mut session = fault_store.session().await.unwrap();
        if kind == "artifact" {
            session
                .put(
                    &admin,
                    "artifact",
                    "ordinary-upstream",
                    admin.actor(),
                    &serde_json::json!({"id":"ordinary-upstream"}),
                )
                .await
                .unwrap();
            session
                .put_edge(&admin, "run", RUN, "artifact", "ordinary-upstream")
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
        let control = fault_store
            .reserve_budget_call_with_sources(
                &broker_host(),
                &q03_reservation("q03-upstream-health"),
                &[RUN.into()],
            )
            .await
            .unwrap();
        fault_store
            .release_undispatched_budget_call(
                &broker_host(),
                &q03_fence(&control),
                "healthy-control",
            )
            .await
            .unwrap();
        let mut session = fault_store.session().await.unwrap();
        if kind == "run" {
            session
                .put(
                    &admin,
                    "run",
                    "incomplete-upstream",
                    admin.actor(),
                    &serde_json::json!({"id":"incomplete-upstream"}),
                )
                .await
                .unwrap();
            session
                .put_edge(&admin, "run", RUN, "run", "incomplete-upstream")
                .await
                .unwrap();
        } else {
            session
                .delete(&admin, "artifact", "ordinary-upstream")
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
        let path = fault_dir.path().join("rsia.sqlite3");
        let before = q03_lossless_snapshot(&path);
        assert!(matches!(
            fault_store
                .reserve_budget_call_with_sources(
                    &broker_host(),
                    &q03_reservation("q03-incomplete-upstream"),
                    &[RUN.into()]
                )
                .await,
            Err(Error::Forbidden)
        ));
        assert_eq!(q03_lossless_snapshot(&path), before);
    }
    q03_revoke_upstream(&store, &upstream).await;
    let path = dir.path().join("rsia.sqlite3");
    let before = q03_lossless_snapshot(&path);
    let request = q03_reservation("q03-refused-reserve");
    assert!(matches!(
        store
            .reserve_budget_call_with_sources(&broker_host(), &request, &[RUN.into()])
            .await,
        Err(Error::Forbidden)
    ));
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .reserve_budget_call_with_sources_typed(&broker_host(), &request, &[RUN.into()])
            .await
            .unwrap(),
        Err(evo_storage::budget::BudgetPreDispatchRefusal::SourceRevoked)
    );
    session.commit().await.unwrap();
    assert_eq!(q03_lossless_snapshot(&path), before);
    assert_eq!(micros(&store, BROKER_SCOPE, &broker_admin()).await, (0, 0));
}

#[tokio::test]
async fn q03_legacy_dispatch_refuses_revoked_transitive_source_without_release() {
    let (dir, store, upstream) = q03_source_graph().await;
    let call = store
        .reserve_budget_call_with_sources(
            &broker_host(),
            &q03_reservation("q03-dispatch"),
            &[RUN.into()],
        )
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let original: serde_json::Value = session.need(&broker_admin(), "run", RUN).await.unwrap();
    session.commit().await.unwrap();
    for replacement in [
        None,
        Some(serde_json::json!({"id":RUN,"schema_version":"rsia.optimization.source.v1"})),
    ] {
        let mut session = store.session().await.unwrap();
        session.delete(&broker_admin(), "run", RUN).await.unwrap();
        if let Some(value) = replacement {
            session
                .put(&broker_admin(), "run", RUN, broker_admin().actor(), &value)
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
        let path = dir.path().join("rsia.sqlite3");
        let before = q03_lossless_snapshot(&path);
        assert!(matches!(
            store
                .begin_budget_dispatch(&broker_host(), &q03_fence(&call))
                .await,
            Err(Error::Cancelled)
        ));
        let mut session = store.session().await.unwrap();
        assert_eq!(
            session
                .begin_budget_dispatch_typed(&broker_host(), &q03_fence(&call))
                .await
                .unwrap(),
            Err(evo_storage::budget::BudgetPreDispatchRefusal::SourceRevoked)
        );
        session.commit().await.unwrap();
        assert_eq!(q03_lossless_snapshot(&path), before);
        assert_eq!(broker_call(&store, &call.call_id).await.unwrap(), call);
    }
    let mut session = store.session().await.unwrap();
    session
        .put(
            &broker_admin(),
            "run",
            RUN,
            broker_admin().actor(),
            &original,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    q03_revoke_upstream(&store, &upstream).await;
    let path = dir.path().join("rsia.sqlite3");
    let before = q03_lossless_snapshot(&path);
    assert!(matches!(
        store
            .begin_budget_dispatch(&broker_host(), &q03_fence(&call))
            .await,
        Err(Error::Cancelled)
    ));
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .begin_budget_dispatch_typed(&broker_host(), &q03_fence(&call))
            .await
            .unwrap(),
        Err(evo_storage::budget::BudgetPreDispatchRefusal::SourceRevoked)
    );
    session.commit().await.unwrap();
    assert_eq!(q03_lossless_snapshot(&path), before);
    assert_eq!(broker_call(&store, &call.call_id).await.unwrap(), call);
    assert_eq!(micros(&store, BROKER_SCOPE, &broker_admin()).await, (20, 0));
}

#[tokio::test]
async fn q03_existing_call_missing_ref_cannot_repair_over_revoked_closure() {
    let (dir, store, upstream) = q03_source_graph().await;
    let request = q03_reservation("q03-existing");
    let call = store
        .reserve_budget_call_with_sources(&broker_host(), &request, &[RUN.into()])
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let original: serde_json::Value = session.need(&broker_admin(), "run", RUN).await.unwrap();
    session.delete(&broker_admin(), "run", RUN).await.unwrap();
    session.commit().await.unwrap();
    let path = dir.path().join("rsia.sqlite3");
    let before = q03_lossless_snapshot(&path);
    assert_eq!(
        store
            .reserve_budget_call_with_sources(&broker_host(), &request, &[RUN.into()])
            .await
            .unwrap(),
        call
    );
    assert_eq!(q03_lossless_snapshot(&path), before);
    let digest = fingerprint(&(
        "rsia.budget_call_ref.v1",
        BROKER_NAMESPACE,
        BROKER_SCOPE,
        &call.call_id,
    ))
    .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .delete(
            &broker_admin(),
            "artifact",
            &format!("budget-ref-{}", &digest[..32]),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let before = q03_lossless_snapshot(&path);
    assert!(matches!(
        store
            .reserve_budget_call_with_sources(&broker_host(), &request, &[RUN.into()])
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(q03_lossless_snapshot(&path), before);
    let mut session = store.session().await.unwrap();
    session
        .put(
            &broker_admin(),
            "run",
            RUN,
            broker_admin().actor(),
            &original,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    q03_revoke_upstream(&store, &upstream).await;
    let path = dir.path().join("rsia.sqlite3");
    let before = q03_lossless_snapshot(&path);
    assert!(matches!(
        store
            .reserve_budget_call_with_sources(&broker_host(), &request, &[RUN.into()])
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(q03_lossless_snapshot(&path), before);
    assert_eq!(broker_call(&store, &call.call_id).await.unwrap(), call);
}

#[tokio::test]
async fn q03_late_transitive_revocation_settlement_keeps_charge_and_redacts_content() {
    use evo_storage::budget::{
        BudgetExecutionProvenance, ModelCallSettlementEvidence, UsageCharge,
    };
    let (_dir, store, upstream) = q03_source_graph().await;
    let call = store
        .reserve_budget_call_with_sources(
            &broker_host(),
            &q03_reservation("q03-paid"),
            &[RUN.into()],
        )
        .await
        .unwrap();
    let dispatched = store
        .begin_budget_dispatch(&broker_host(), &q03_fence(&call))
        .await
        .unwrap();
    assert!(dispatched.new_dispatch);
    q03_revoke_upstream(&store, &upstream).await;
    let output = "q03-late-private-output";
    let transport = BudgetArtifact::from_serializable(
        "rsia.model_transport.artifact.v1",
        &serde_json::json!({"output":output}),
    )
    .unwrap();
    let evidence = ModelCallSettlementEvidence {
        provenance: BudgetExecutionProvenance::Fixture,
        actual_model_digest: Some(hash(b"model")),
        transport_artifact: transport.clone(),
        usable_response: Some(
            BudgetArtifact::from_serializable(
                "rsia.model_response.artifact.v1",
                &serde_json::json!({"output":output}),
            )
            .unwrap(),
        ),
        blocked_response: BudgetArtifact::from_serializable(
            "rsia.model_response.artifact.v1",
            &serde_json::json!({"reason":"blocked"}),
        )
        .unwrap(),
        forced_block_reason: None,
    };
    let charge = UsageCharge {
        amount_micros: 10,
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        provider_request_id: "q03-provider".into(),
        usage_record_id: "q03-usage".into(),
        output_digest: hash(output.as_bytes()),
    };
    let settled = store
        .settle_model_budget_call(&broker_host(), &q03_fence(&call), &charge, &evidence)
        .await
        .unwrap();
    assert!(!settled.response_usable);
    assert_eq!(
        settled.response_block_reason.as_deref(),
        Some("source_revoked")
    );
    assert_eq!(settled.call.actual_cost_micros, Some(10));
    assert_eq!(settled.call.dispatch_id, dispatched.call.dispatch_id);
    assert_eq!(
        settled.call.provider_request_id.as_deref(),
        Some("q03-provider")
    );
    assert_eq!(settled.call.usage_record_id.as_deref(), Some("q03-usage"));
    assert_eq!(micros(&store, BROKER_SCOPE, &broker_admin()).await, (0, 10));
    let redacted = settled.call.transport_artifact.unwrap();
    assert_eq!(redacted.schema_version, "rsia.redacted.v1");
    let body: serde_json::Value = serde_json::from_str(&redacted.body).unwrap();
    assert_eq!(body.as_object().unwrap().len(), 5);
    assert_eq!(body["original_schema"], transport.schema_version);
    assert_eq!(body["original_digest"], transport.digest);
    assert!(!redacted.body.contains(output));
}
