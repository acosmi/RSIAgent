use async_trait::async_trait;
use evo_core::evidence::Purpose;
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
};
use evo_core::skill_edit::EvidenceRef;
use evo_core::{Context, Result, Role, hash};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_engine::model::{ModelExecutionProvenance, ModelPort, ModelRejectionKind, ModelResponse};
use evo_storage::Store;
use evo_storage::budget::RootBudgetAuthorization;
use evo_storage::lifecycle::CleanupState;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct FixtureTransport {
    calls: Arc<AtomicUsize>,
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
        Ok(TransportCompletion {
            response_id: "response-1".into(),
            provider_request_id: "provider-1".into(),
            usage_record_id: "usage-1".into(),
            actual_model_digest: request.model_digest.clone(),
            output: "SECRET-MODEL-OUTPUT".into(),
            actual_cost_micros: 7,
            currency: "USD".into(),
            pricing_version: "pricing-v1".into(),
        })
    }
}

fn request() -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: "request-1".into(),
            namespace: "n".into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "episode-1".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: hash(b"parent"),
            bundle_digest: hash(b"bundle"),
            source_closure: vec![EvidenceRef {
                id: "run-1".into(),
                digest: hash(b"source"),
            }],
            model_digest: hash(b"model"),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: hash(b"sampling"),
            revoke_watermark: 0,
            max_suggestions: 4,
        },
        vec![ModelInputPart {
            role: ModelInputRole::Evidence,
            label: "source".into(),
            content: "SECRET-MODEL-INPUT".into(),
        }],
    )
    .unwrap()
}

#[tokio::test]
async fn source_revoke_redacts_broker_content_and_reconnect_never_calls_transport() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("e08.sqlite3")).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &host,
            "run",
            "run-1",
            host.actor(),
            &serde_json::json!({"id":"run-1","body":"SECRET-SOURCE"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "scope-1".into(),
                allowed_namespaces: vec!["n".into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "payer".into(),
                authorization_receipt_digest: hash(b"authorization"),
                per_call_cap_micros: 20,
                total_limit_micros: 100,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        FixtureTransport {
            calls: calls.clone(),
        },
        BrokerConfig {
            billing_scope: "scope-1".into(),
            actor: "broker".into(),
            currency: "USD".into(),
            pricing_version: "pricing-v1".into(),
            max_cost_micros: 20,
            lease_seconds: 60,
        },
        Arc::new(|| 10),
    )
    .unwrap();
    let request = request();
    assert!(matches!(
        broker.dispatch(request.clone()).await.unwrap(),
        ModelResponse::Completed { .. }
    ));
    let mut status =
        LifecycleCoordinator::revoke_source(&admin, &store, "run-1", "privacy_delete", 20)
            .await
            .unwrap();
    for now in 21..80 {
        if status.state == CleanupState::Complete {
            break;
        }
        status = LifecycleCoordinator::continue_cleanup(&admin, &store, &status.job_id, 8, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete);
    assert!(matches!(
        broker.dispatch(request).await.unwrap(),
        ModelResponse::Rejected {
            kind: ModelRejectionKind::CancelledAfterDispatch,
            ..
        }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let call = store
        .budget_call(&worker, "scope-1", "request-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.actual_cost_micros, Some(7));
    assert_eq!(call.usage_record_id.as_deref(), Some("usage-1"));
    assert_eq!(
        call.response_block_reason.as_deref(),
        Some("source_revoked")
    );
}
