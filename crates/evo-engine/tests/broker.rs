use async_trait::async_trait;
use evo_core::evidence::Purpose;
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
};
use evo_core::skill_edit::EvidenceRef;
use evo_core::{Context, Error, Result, Role, hash};
use evo_engine::broker::{
    BrokerConfig, DisabledModelTransport, ModelTransport, PersistentModelBroker,
    TransportCompletion,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelPort, ModelRejectionKind, ModelResponse, RejectedDispatch,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

#[derive(Clone)]
struct FixtureTransport {
    calls: Arc<AtomicUsize>,
    fail: bool,
    wrong_pricing: bool,
    wrong_model: bool,
    advance_clock_to: Option<Arc<AtomicI64>>,
}

#[async_trait]
impl ModelTransport for FixtureTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(clock) = &self.advance_clock_to {
            clock.store(100, Ordering::SeqCst);
        }
        if self.fail {
            return Err(Error::Internal);
        }
        let mut completion = completion(request, self.wrong_pricing);
        if self.wrong_model {
            completion.actual_model_digest = hash(b"different-model");
        }
        Ok(completion)
    }
}

#[derive(Clone)]
struct GroupStoppingTransport {
    store: Store,
    evaluator: Context,
    calls: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct InvalidOutputTransport {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ModelTransport for InvalidOutputTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut completion = completion(request, false);
        completion.output.clear();
        Ok(completion)
    }
}

#[async_trait]
impl ModelTransport for GroupStoppingTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.store
            .stop_dispatch_group(
                &self.evaluator,
                "scope-1",
                &request.episode_id,
                "early_stop",
                15,
            )
            .await?;
        Ok(completion(request, false))
    }
}

fn completion(request: &ModelRequest, wrong_pricing: bool) -> TransportCompletion {
    TransportCompletion {
        response_id: format!("response-{}", request.request_id),
        provider_request_id: format!("provider-{}", request.request_id),
        usage_record_id: format!("usage-{}", request.request_id),
        actual_model_digest: request.model_digest.clone(),
        output: "fixture output".into(),
        actual_cost_micros: 10,
        currency: "USD".into(),
        pricing_version: if wrong_pricing {
            "wrong-price".into()
        } else {
            "price-v1".into()
        },
    }
}

fn request(id: &str, stage: ModelStage) -> ModelRequest {
    request_in_namespace(id, stage, "ns-a")
}

fn request_in_namespace(id: &str, stage: ModelStage, namespace: &str) -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: id.into(),
            namespace: namespace.into(),
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

fn config() -> BrokerConfig {
    BrokerConfig {
        billing_scope: "scope-1".into(),
        actor: "trusted-broker".into(),
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        max_cost_micros: 20,
        lease_seconds: 60,
    }
}

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    (dir, store)
}

async fn authorize(store: &Store) {
    let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "scope-1".into(),
                allowed_namespaces: vec!["ns-a".into()],
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
}

fn fixed_clock() -> Arc<dyn Fn() -> i64 + Send + Sync> {
    Arc::new(|| 10)
}

fn refusal_broker(
    store: &Store,
    calls: &Arc<AtomicUsize>,
) -> PersistentModelBroker<FixtureTransport> {
    PersistentModelBroker::with_clock(
        store.clone(),
        FixtureTransport {
            calls: calls.clone(),
            fail: false,
            wrong_pricing: false,
            wrong_model: false,
            advance_clock_to: None,
        },
        config(),
        fixed_clock(),
    )
    .unwrap()
}

fn assert_predispatch_refusal(response: ModelResponse, expected: ModelRejectionKind, code: &str) {
    match response {
        ModelResponse::Rejected {
            kind,
            reason,
            dispatch,
            ..
        } => {
            assert_eq!(kind, expected);
            assert_eq!(reason, format!("broker_pre_dispatch_v1.{code}"));
            assert_eq!(dispatch, RejectedDispatch::NotDispatched);
        }
        other => panic!("expected a known pre-dispatch refusal, got {other:?}"),
    }
}

// Stage A: these regressions exercise only the APIs available at the task baseline.
#[tokio::test]
async fn ag054_public_stopped_group_is_cancelled_before_dispatch() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
    store
        .stop_dispatch_group(&admin, "scope-1", "episode-1", "operator stop", 2)
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = refusal_broker(&store, &calls);
    assert_predispatch_refusal(
        broker
            .dispatch(request("group-refused", ModelStage::Merge))
            .await
            .unwrap(),
        ModelRejectionKind::CancelledBeforeDispatch,
        "group_stopped",
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        0
    );
}

#[tokio::test]
async fn ag054_public_concurrency_refusal_releases_only_its_reservation() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let worker = Context::new("ns-a", "trusted-broker", Role::Worker).unwrap();
    let active = store
        .reserve_budget_call(
            &worker,
            &BudgetCallReservation {
                billing_scope: "scope-1".into(),
                call_id: "active".into(),
                dispatch_group_id: "active-group".into(),
                stage: BudgetStage::Reflection,
                actual_input_digest: hash(b"active"),
                request_artifact: None,
                max_cost_micros: 7,
                lease_token: "active-lease".into(),
                lease_until: 100,
                now: 2,
            },
        )
        .await
        .unwrap();
    store
        .begin_budget_dispatch(
            &worker,
            &BudgetCallFence {
                billing_scope: active.billing_scope,
                call_id: active.call_id,
                actual_input_digest: active.actual_input_digest,
                lease_token: active.lease_token,
                lease_epoch: active.lease_epoch,
                now: 3,
            },
        )
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = refusal_broker(&store, &calls);
    assert_predispatch_refusal(
        broker
            .dispatch(request("concurrency-refused", ModelStage::Merge))
            .await
            .unwrap(),
        ModelRejectionKind::BudgetUnavailable,
        "root_concurrency_limit",
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let call = store
        .budget_call(&worker, "scope-1", "concurrency-refused")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, BudgetCallState::Released);
    assert_eq!(
        call.terminal_reason.as_deref(),
        Some("broker_pre_dispatch_v1.root_concurrency_limit")
    );
    assert_eq!(
        store
            .root_budget(&worker, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        7
    );
}

#[tokio::test]
async fn ag054_public_reused_call_is_invalid_and_preserves_finalized_facts() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = refusal_broker(&store, &calls);
    let first = request("reused", ModelStage::Merge);
    broker.dispatch(first).await.unwrap();
    let worker = Context::new("ns-a", "trusted-broker", Role::Worker).unwrap();
    let before = store
        .budget_call(&worker, "scope-1", "reused")
        .await
        .unwrap()
        .unwrap();
    assert_predispatch_refusal(
        broker
            .dispatch(request("reused", ModelStage::Rank))
            .await
            .unwrap(),
        ModelRejectionKind::InvalidRequest,
        "call_id_reused",
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .budget_call(&worker, "scope-1", "reused")
            .await
            .unwrap()
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn disabled_transport_never_reserves_or_dispatches() {
    let (_dir, store) = store().await;
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        DisabledModelTransport,
        config(),
        fixed_clock(),
    )
    .unwrap();
    let response = broker
        .dispatch(request("request-1", ModelStage::ReflectFailure))
        .await
        .unwrap();
    assert!(matches!(
        response,
        ModelResponse::Rejected {
            kind: ModelRejectionKind::Unauthorized,
            dispatch: RejectedDispatch::NotDispatched,
            ..
        }
    ));
    let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
    assert!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn budget_binding_is_read_only_and_namespace_scoped() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        DisabledModelTransport,
        config(),
        fixed_clock(),
    )
    .unwrap();
    let binding = broker.budget_binding("ns-a").await.unwrap();
    assert_eq!(binding.billing_scope, "scope-1");
    assert_eq!(binding.root_budget_id, "root-1");
    assert!(broker.budget_binding("other-namespace").await.is_err());
    assert!(
        store
            .budget_call(
                &Context::new("ns-a", "admin", Role::Admin).unwrap(),
                "scope-1",
                "uncreated-call"
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn successful_fixture_is_billed_once_and_receipt_matches_persistent_facts() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        FixtureTransport {
            calls: calls.clone(),
            fail: false,
            wrong_pricing: false,
            wrong_model: false,
            advance_clock_to: None,
        },
        config(),
        fixed_clock(),
    )
    .unwrap();
    let request = request("request-1", ModelStage::ReflectSuccess);
    let response = broker.dispatch(request.clone()).await.unwrap();
    let receipt = match response {
        ModelResponse::Completed {
            execution_receipt,
            output,
            ..
        } => {
            assert_eq!(output, "fixture output");
            execution_receipt
        }
        other => panic!("unexpected response: {other:?}"),
    };
    broker.verify_receipt(&request, &receipt).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let worker = Context::new("ns-a", "trusted-broker", Role::Host).unwrap();
    let call = store
        .budget_call(&worker, "scope-1", "request-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, BudgetCallState::Finalized);
    assert!(call.execution_closed);
    assert_eq!(call.actual_cost_micros, Some(10));

    let mut forged = receipt.clone();
    forged.provenance = ModelExecutionProvenance::ExternalProvider;
    assert!(broker.verify_receipt(&request, &forged).await.is_err());

    let repeated = broker.dispatch(request).await.unwrap();
    assert!(matches!(repeated, ModelResponse::Completed { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        10
    );
}

#[tokio::test]
async fn transport_failure_becomes_uncertain_and_retry_does_not_call_again() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        FixtureTransport {
            calls: calls.clone(),
            fail: true,
            wrong_pricing: false,
            wrong_model: false,
            advance_clock_to: None,
        },
        config(),
        fixed_clock(),
    )
    .unwrap();
    let request = request("request-1", ModelStage::Merge);
    assert!(matches!(
        broker.dispatch(request.clone()).await.unwrap(),
        ModelResponse::Uncertain { .. }
    ));
    assert!(matches!(
        broker.dispatch(request).await.unwrap(),
        ModelResponse::Uncertain { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let worker = Context::new("ns-a", "trusted-broker", Role::Worker).unwrap();
    let call = store
        .budget_call(&worker, "scope-1", "request-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, BudgetCallState::Uncertain);
    assert_eq!(call.reserved_micros, 20);
}

#[tokio::test]
async fn invalid_charge_identity_freezes_dispatched_call_as_uncertain() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        FixtureTransport {
            calls: Arc::new(AtomicUsize::new(0)),
            fail: false,
            wrong_pricing: true,
            wrong_model: false,
            advance_clock_to: None,
        },
        config(),
        fixed_clock(),
    )
    .unwrap();
    assert!(matches!(
        broker
            .dispatch(request("request-1", ModelStage::Rank))
            .await
            .unwrap(),
        ModelResponse::Rejected {
            kind: ModelRejectionKind::CancelledAfterDispatch,
            dispatch: RejectedDispatch::Dispatched { .. },
            ..
        }
    ));
    let worker = Context::new("ns-a", "trusted-broker", Role::Worker).unwrap();
    let call = store
        .budget_call(&worker, "scope-1", "request-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, BudgetCallState::Uncertain);
    assert_eq!(call.actual_cost_micros, Some(10));
    assert!(call.response_artifact.is_some());
    assert_eq!(
        call.execution_provenance,
        Some(evo_storage::budget::BudgetExecutionProvenance::Fixture)
    );
}

#[tokio::test]
async fn wrong_actual_model_is_billed_but_persistently_rejected() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        FixtureTransport {
            calls: calls.clone(),
            fail: false,
            wrong_pricing: false,
            wrong_model: true,
            advance_clock_to: None,
        },
        config(),
        fixed_clock(),
    )
    .unwrap();
    let request = request("request-wrong-model", ModelStage::Rank);
    assert!(matches!(
        broker.dispatch(request.clone()).await.unwrap(),
        ModelResponse::Rejected {
            kind: ModelRejectionKind::ProviderRejected,
            dispatch: RejectedDispatch::Dispatched { .. },
            ..
        }
    ));
    let worker = Context::new("ns-a", "trusted-broker", Role::Worker).unwrap();
    let call = store
        .budget_call(&worker, "scope-1", "request-wrong-model")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.actual_cost_micros, Some(10));
    assert_eq!(
        call.response_block_reason.as_deref(),
        Some("actual_model_digest_mismatch")
    );
    assert_eq!(call.response_usable, Some(false));
    let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        10
    );
    assert!(matches!(
        broker.dispatch(request).await.unwrap(),
        ModelResponse::Rejected {
            kind: ModelRejectionKind::ProviderRejected,
            ..
        }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let after_retry = store
        .budget_call(&worker, "scope-1", "request-wrong-model")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after_retry.response_block_reason.as_deref(),
        Some("actual_model_digest_mismatch")
    );
}

#[tokio::test]
async fn invalid_completion_with_valid_usage_is_charged_and_persisted_as_blocked() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        InvalidOutputTransport {
            calls: calls.clone(),
        },
        config(),
        fixed_clock(),
    )
    .unwrap();
    let request = request("request-invalid-output", ModelStage::ReflectFailure);
    assert!(matches!(
        broker.dispatch(request.clone()).await.unwrap(),
        ModelResponse::Rejected {
            kind: ModelRejectionKind::ProviderRejected,
            dispatch: RejectedDispatch::Dispatched { .. },
            ..
        }
    ));
    let worker = Context::new("ns-a", "trusted-broker", Role::Worker).unwrap();
    let call = store
        .budget_call(&worker, "scope-1", "request-invalid-output")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.actual_cost_micros, Some(10));
    assert_eq!(
        call.response_block_reason.as_deref(),
        Some("invalid_transport_completion")
    );
    assert!(call.transport_artifact.is_some());
    assert_eq!(
        store
            .root_budget(
                &Context::new("ns-a", "admin", Role::Admin).unwrap(),
                "scope-1",
            )
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        10
    );
    assert!(matches!(
        broker.dispatch(request).await.unwrap(),
        ModelResponse::Rejected {
            kind: ModelRejectionKind::ProviderRejected,
            ..
        }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn response_after_lease_expiry_is_billed_persisted_and_never_returned_as_completed() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let clock = Arc::new(AtomicI64::new(10));
    let read_clock: Arc<dyn Fn() -> i64 + Send + Sync> = {
        let clock = clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst))
    };
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        FixtureTransport {
            calls: calls.clone(),
            fail: false,
            wrong_pricing: false,
            wrong_model: false,
            advance_clock_to: Some(clock),
        },
        BrokerConfig {
            lease_seconds: 20,
            ..config()
        },
        read_clock.clone(),
    )
    .unwrap();
    let request = request("request-late", ModelStage::ReflectFailure);
    assert!(matches!(
        broker.dispatch(request.clone()).await.unwrap(),
        ModelResponse::Rejected {
            kind: ModelRejectionKind::CancelledAfterDispatch,
            dispatch: RejectedDispatch::Dispatched { .. },
            ..
        }
    ));
    let worker = Context::new("ns-a", "trusted-broker", Role::Worker).unwrap();
    let call = store
        .budget_call(&worker, "scope-1", "request-late")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.actual_cost_micros, Some(10));
    assert_eq!(call.response_usable, Some(false));
    assert_eq!(call.response_block_reason.as_deref(), Some("lease_expired"));
    assert!(call.execution_closed);

    let disabled = PersistentModelBroker::with_clock(
        store.clone(),
        DisabledModelTransport,
        BrokerConfig {
            lease_seconds: 20,
            ..config()
        },
        read_clock,
    )
    .unwrap();
    assert!(matches!(
        disabled.dispatch(request).await.unwrap(),
        ModelResponse::Rejected {
            kind: ModelRejectionKind::CancelledAfterDispatch,
            ..
        }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn group_stop_during_provider_await_bills_result_but_blocks_completed_response() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        GroupStoppingTransport {
            store: store.clone(),
            evaluator: Context::new("ns-a", "evaluator", Role::Evaluator).unwrap(),
            calls: calls.clone(),
        },
        config(),
        fixed_clock(),
    )
    .unwrap();
    let request = request("request-stopped", ModelStage::Merge);
    assert!(matches!(
        broker.dispatch(request.clone()).await.unwrap(),
        ModelResponse::Rejected {
            kind: ModelRejectionKind::CancelledAfterDispatch,
            dispatch: RejectedDispatch::Dispatched { .. },
            ..
        }
    ));
    let worker = Context::new("ns-a", "trusted-broker", Role::Worker).unwrap();
    let call = store
        .budget_call(&worker, "scope-1", "request-stopped")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.actual_cost_micros, Some(10));
    assert_eq!(call.response_usable, Some(false));
    assert_eq!(
        call.response_block_reason.as_deref(),
        Some("dispatch_group_stopped")
    );
    assert!(call.execution_closed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn restart_after_atomic_settlement_returns_persisted_response_without_transport_replay() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let request = request("request-recovery", ModelStage::ReflectSuccess);
    let worker = Context::new("ns-a", "trusted-broker", Role::Host).unwrap();
    let reserved = store
        .reserve_budget_call(
            &worker,
            &BudgetCallReservation {
                billing_scope: "scope-1".into(),
                call_id: request.request_id.clone(),
                dispatch_group_id: request.episode_id.clone(),
                stage: BudgetStage::Reflection,
                actual_input_digest: request.cache_key_digest.clone(),
                request_artifact: Some(
                    BudgetArtifact::from_serializable("rsia.model_request.artifact.v1", &request)
                        .unwrap(),
                ),
                max_cost_micros: 20,
                lease_token: "lease-recovery".into(),
                lease_until: 70,
                now: 10,
            },
        )
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: "scope-1".into(),
        call_id: request.request_id.clone(),
        actual_input_digest: request.cache_key_digest.clone(),
        lease_token: reserved.lease_token.clone(),
        lease_epoch: reserved.lease_epoch,
        now: 10,
    };
    let dispatched = store
        .begin_budget_dispatch(&worker, &fence)
        .await
        .unwrap()
        .call;
    let completion = completion(&request, false);
    let receipt = evo_engine::model::ModelExecutionReceipt {
        call_id: request.request_id.clone(),
        dispatch_id: dispatched.dispatch_id.clone().unwrap(),
        root_budget_id: "root-1".into(),
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
    let blocked = ModelResponse::Rejected {
        request_id: request.request_id.clone(),
        kind: ModelRejectionKind::CancelledAfterDispatch,
        reason: "blocked after dispatch".into(),
        dispatch: RejectedDispatch::Dispatched { receipt },
    };
    let settlement = store
        .settle_model_budget_call(
            &worker,
            &fence,
            &UsageCharge {
                amount_micros: 10,
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
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
    assert!(!settlement.call.execution_closed);

    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        DisabledModelTransport,
        config(),
        fixed_clock(),
    )
    .unwrap();
    assert!(matches!(
        broker.dispatch(request).await.unwrap(),
        ModelResponse::Completed { .. }
    ));
    let recovered = store
        .budget_call(&worker, "scope-1", "request-recovery")
        .await
        .unwrap()
        .unwrap();
    assert!(recovered.execution_closed);
}

async fn reserve_model_request(
    store: &Store,
    request: &ModelRequest,
) -> evo_storage::budget::BudgetCallRecord {
    let host = Context::new("ns-a", "trusted-broker", Role::Host).unwrap();
    store
        .reserve_budget_call_with_sources(
            &host,
            &BudgetCallReservation {
                billing_scope: "scope-1".into(),
                call_id: request.request_id.clone(),
                dispatch_group_id: request.episode_id.clone(),
                stage: BudgetStage::Merge,
                actual_input_digest: request.cache_key_digest.clone(),
                request_artifact: Some(
                    BudgetArtifact::from_serializable("rsia.model_request.artifact.v1", request)
                        .unwrap(),
                ),
                max_cost_micros: 20,
                lease_token: "pending-lease".into(),
                lease_until: 100,
                now: 2,
            },
            &request
                .source_closure
                .iter()
                .map(|source| source.id.clone())
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap()
}

fn call_fence(call: &evo_storage::budget::BudgetCallRecord, now: i64) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    }
}

#[tokio::test]
async fn ag054_begin_root_group_and_revoked_source_release_and_replay_after_restart() {
    for mode in ["root_stopped", "group_stopped", "source_revoked"] {
        let (dir, store) = store().await;
        authorize(&store).await;
        let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
        let req = request("pending", ModelStage::Merge);
        reserve_model_request(&store, &req).await;
        match mode {
            "root_stopped" => {
                store
                    .stop_root_budget(&admin, "scope-1", "operator stop", 3)
                    .await
                    .unwrap();
            }
            "group_stopped" => {
                store
                    .stop_dispatch_group(&admin, "scope-1", "episode-1", "operator stop", 3)
                    .await
                    .unwrap();
            }
            "source_revoked" => {
                // Corrupt tombstone fails closed, after reservation and before dispatch.
                let mut session = store.session().await.unwrap();
                session
                    .put(
                        &admin,
                        "tombstone",
                        "evidence-1",
                        admin.actor(),
                        &serde_json::json!({}),
                    )
                    .await
                    .unwrap();
                session.commit().await.unwrap();
            }
            _ => unreachable!(),
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let broker = refusal_broker(&store, &calls);
        let first = broker.dispatch(req.clone()).await.unwrap();
        let kind = if mode == "source_revoked" {
            ModelRejectionKind::Unauthorized
        } else {
            ModelRejectionKind::CancelledBeforeDispatch
        };
        assert_predispatch_refusal(first.clone(), kind, mode);
        let persisted = store
            .budget_call(&admin, "scope-1", "pending")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.state, BudgetCallState::Released);
        assert_eq!(persisted.dispatch_id, None);
        assert_eq!(
            persisted.terminal_reason,
            Some(format!("broker_pre_dispatch_v1.{mode}"))
        );
        assert_eq!(
            store
                .root_budget(&admin, "scope-1")
                .await
                .unwrap()
                .unwrap()
                .reserved_micros,
            0
        );
        assert_eq!(
            serde_json::to_value(broker.dispatch(req.clone()).await.unwrap()).unwrap(),
            serde_json::to_value(&first).unwrap()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        store.close().await;
        let reopened = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
        let disabled = PersistentModelBroker::with_clock(
            reopened,
            DisabledModelTransport,
            config(),
            fixed_clock(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(disabled.dispatch(req).await.unwrap()).unwrap(),
            serde_json::to_value(first).unwrap()
        );
    }
}

#[tokio::test]
async fn ag054_reused_reserved_and_uncertain_calls_keep_all_original_facts() {
    for uncertain in [false, true] {
        let (_dir, store) = store().await;
        authorize(&store).await;
        let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
        let req = request("same", ModelStage::Merge);
        let call = reserve_model_request(&store, &req).await;
        if uncertain {
            store
                .begin_budget_dispatch(&admin, &call_fence(&call, 3))
                .await
                .unwrap();
            store
                .mark_budget_call_uncertain(&admin, &call_fence(&call, 4), "provider unavailable")
                .await
                .unwrap();
        }
        let before = store
            .budget_call(&admin, "scope-1", "same")
            .await
            .unwrap()
            .unwrap();
        let root = store.root_budget(&admin, "scope-1").await.unwrap().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let broker = refusal_broker(&store, &calls);
        assert_predispatch_refusal(
            broker
                .dispatch(request("same", ModelStage::Rank))
                .await
                .unwrap(),
            ModelRejectionKind::InvalidRequest,
            "call_id_reused",
        );
        assert_eq!(
            store
                .budget_call(&admin, "scope-1", "same")
                .await
                .unwrap()
                .unwrap(),
            before
        );
        assert_eq!(
            store.root_budget(&admin, "scope-1").await.unwrap().unwrap(),
            root
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn ag054_corrupt_existing_source_ref_is_err_and_keeps_reservation() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let req = request("corrupt", ModelStage::Merge);
    let call = reserve_model_request(&store, &req).await;
    let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
    let digest = evo_core::fingerprint(&(
        "rsia.budget_call_ref.v1",
        &call.namespace,
        &call.billing_scope,
        &call.call_id,
    ))
    .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            &format!("budget-ref-{}", &digest[..32]),
            admin.actor(),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = refusal_broker(&store, &calls);
    assert!(matches!(broker.dispatch(req).await, Err(Error::Internal)));
    assert_eq!(
        store
            .budget_call(&admin, "scope-1", "corrupt")
            .await
            .unwrap()
            .unwrap(),
        call
    );
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        20
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ag054_configuration_refusal_replays_its_persisted_code() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        FixtureTransport {
            calls: calls.clone(),
            fail: false,
            wrong_pricing: false,
            wrong_model: false,
            advance_clock_to: None,
        },
        BrokerConfig {
            currency: "EUR".into(),
            ..config()
        },
        fixed_clock(),
    )
    .unwrap();
    let req = request("configuration", ModelStage::Merge);
    let first = broker.dispatch(req.clone()).await.unwrap();
    assert_predispatch_refusal(
        first.clone(),
        ModelRejectionKind::Unauthorized,
        "configuration_mismatch",
    );
    assert_eq!(
        serde_json::to_value(broker.dispatch(req).await.unwrap()).unwrap(),
        serde_json::to_value(first).unwrap()
    );
    let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
    assert_eq!(
        store
            .root_budget(&admin, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        0
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ag054_only_released_known_versioned_reasons_are_decoded() {
    for (cancelled, reason) in [
        (false, "root_budget_concurrency_limit"),
        (false, "operator arbitrary reason"),
        (false, "broker_pre_dispatch_v1.future_code"),
        (false, "development_pre_dispatch_v1.root_concurrency_limit"),
        (true, "broker_pre_dispatch_v1.source_revoked"),
    ] {
        let (_dir, store) = store().await;
        authorize(&store).await;
        let req = request("terminal", ModelStage::Merge);
        let call = reserve_model_request(&store, &req).await;
        let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
        if cancelled {
            store
                .cancel_budget_call(&admin, &call_fence(&call, 3), reason)
                .await
                .unwrap();
        } else {
            store
                .release_undispatched_budget_call(&admin, &call_fence(&call, 3), reason)
                .await
                .unwrap();
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let broker = refusal_broker(&store, &calls);
        match broker.dispatch(req).await.unwrap() {
            ModelResponse::Rejected {
                kind,
                reason,
                dispatch,
                ..
            } => {
                assert_eq!(kind, ModelRejectionKind::CancelledBeforeDispatch);
                assert_eq!(reason, "call ended before dispatch");
                assert_eq!(dispatch, RejectedDispatch::NotDispatched);
            }
            other => panic!("expected legacy fallback, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn ag054_namespace_refusals_do_not_expose_or_modify_other_calls() {
    let (_dir, store) = store().await;
    let admin = Context::new("ns-a", "admin", Role::Admin).unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                allowed_namespaces: vec!["ns-a".into(), "ns-b".into()],
                root_budget_id: "root-1".into(),
                billing_scope: "scope-1".into(),
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
    let original = reserve_model_request(&store, &request("private", ModelStage::Merge)).await;
    let root = store.root_budget(&admin, "scope-1").await.unwrap().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = refusal_broker(&store, &calls);
    for (id, namespace, code) in [
        ("private", "ns-b", "unauthorized"),
        ("different", "ns-b", "group_namespace_mismatch"),
        ("different", "ns-c", "unauthorized"),
    ] {
        assert_predispatch_refusal(
            broker
                .dispatch(request_in_namespace(id, ModelStage::Merge, namespace))
                .await
                .unwrap(),
            ModelRejectionKind::Unauthorized,
            code,
        );
    }
    assert_eq!(
        store
            .budget_call(&admin, "scope-1", "private")
            .await
            .unwrap()
            .unwrap(),
        original
    );
    assert_eq!(
        store.root_budget(&admin, "scope-1").await.unwrap().unwrap(),
        root
    );
    assert!(
        store
            .budget_call(&admin, "scope-1", "different")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ag054_public_reserve_resource_refusals_are_budget_unavailable() {
    let (_dir, store) = store().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = refusal_broker(&store, &calls);
    assert_predispatch_refusal(
        broker
            .dispatch(request("missing-root", ModelStage::Merge))
            .await
            .unwrap(),
        ModelRejectionKind::BudgetUnavailable,
        "root_authorization_missing",
    );
    authorize(&store).await;
    let cap_broker = PersistentModelBroker::with_clock(
        store.clone(),
        FixtureTransport {
            calls: calls.clone(),
            fail: false,
            wrong_pricing: false,
            wrong_model: false,
            advance_clock_to: None,
        },
        BrokerConfig {
            max_cost_micros: 21,
            ..config()
        },
        fixed_clock(),
    )
    .unwrap();
    assert_predispatch_refusal(
        cap_broker
            .dispatch(request("over-cap", ModelStage::Merge))
            .await
            .unwrap(),
        ModelRejectionKind::BudgetUnavailable,
        "per_call_cap_exceeded",
    );
    let host = Context::new("ns-a", "trusted-broker", Role::Host).unwrap();
    for index in 0..5 {
        store
            .reserve_budget_call(
                &host,
                &BudgetCallReservation {
                    billing_scope: "scope-1".into(),
                    call_id: format!("held-{index}"),
                    dispatch_group_id: format!("held-group-{index}"),
                    stage: BudgetStage::Reflection,
                    actual_input_digest: hash(b"held"),
                    request_artifact: None,
                    max_cost_micros: 20,
                    lease_token: format!("lease-{index}"),
                    lease_until: 100,
                    now: 2,
                },
            )
            .await
            .unwrap();
    }
    assert_predispatch_refusal(
        broker
            .dispatch(request("root-full", ModelStage::Merge))
            .await
            .unwrap(),
        ModelRejectionKind::BudgetUnavailable,
        "root_budget_exhausted",
    );
    assert_eq!(
        store
            .root_budget(&host, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        100
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ag054_r1_legal_source_closure_change_is_a_reused_effective_request() {
    let (_dir, store) = store().await;
    authorize(&store).await;
    let original_request = request("source-change", ModelStage::Merge);
    let call = reserve_model_request(&store, &original_request).await;
    // Keep a legal nonempty closure, rebuilding all request digests through the
    // public constructor. Empty closures are invalid at ModelRequest::build.
    let changed = ModelRequest::build(
        ModelRequestContext {
            request_id: original_request.request_id.clone(),
            namespace: original_request.namespace.clone(),
            purpose: original_request.purpose,
            stage: original_request.stage,
            episode_id: original_request.episode_id.clone(),
            step: original_request.step,
            attempt: original_request.attempt,
            parent_skill_digest: original_request.parent_skill_digest.clone(),
            bundle_digest: original_request.bundle_digest.clone(),
            source_closure: vec![EvidenceRef {
                id: "different-source".into(),
                digest: hash(b"different source"),
            }],
            model_digest: original_request.model_digest.clone(),
            tools_digest: original_request.tools_digest.clone(),
            rules_digest: original_request.rules_digest.clone(),
            sampling_digest: original_request.sampling_digest.clone(),
            revoke_watermark: original_request.revoke_watermark,
            max_suggestions: original_request.max_suggestions,
        },
        original_request.input.clone(),
    )
    .unwrap();
    assert_ne!(changed.cache_key_digest, original_request.cache_key_digest);
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = refusal_broker(&store, &calls);
    assert_predispatch_refusal(
        broker.dispatch(changed).await.unwrap(),
        ModelRejectionKind::InvalidRequest,
        "call_id_reused",
    );
    let host = Context::new("ns-a", "trusted-broker", Role::Host).unwrap();
    assert_eq!(
        store
            .budget_call(&host, "scope-1", &call.call_id)
            .await
            .unwrap()
            .unwrap(),
        call
    );
    assert_eq!(
        store
            .root_budget(&host, "scope-1")
            .await
            .unwrap()
            .unwrap()
            .reserved_micros,
        20
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
