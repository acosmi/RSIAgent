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
    ModelRequest::build(
        ModelRequestContext {
            request_id: id.into(),
            namespace: "ns-a".into(),
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
