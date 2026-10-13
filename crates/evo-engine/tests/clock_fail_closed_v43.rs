use async_trait::async_trait;
use evo_core::evidence::Purpose;
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
};
use evo_core::skill_edit::EvidenceRef;
use evo_core::{Context, Error, Result, Role, hash};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::model::{ModelExecutionProvenance, ModelPort};
use evo_storage::{Store, budget::RootBudgetAuthorization};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
struct CountingFixture(Arc<AtomicUsize>);
#[async_trait]
impl ModelTransport for CountingFixture {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }
    async fn execute(&self, _: &ModelRequest, _: &str) -> Result<TransportCompletion> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(Error::Internal)
    }
}
async fn refuses(clock: i64) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let admin = Context::new("clock-ns", "clock-broker", Role::Admin).unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "clock-root".into(),
                billing_scope: "clock-scope".into(),
                allowed_namespaces: vec!["clock-ns".into()],
                currency: "USD".into(),
                pricing_version: "fixture-v1".into(),
                payment_subject: "fixture-payer".into(),
                authorization_receipt_digest: hash(b"fixture-only"),
                per_call_cap_micros: 20,
                total_limit_micros: 100,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let mut s = store.session().await.unwrap();
    s.put(&admin, "run", "clock-source", admin.actor(), &json!({"id":"clock-source", "schema_version":"rsia.optimization.source.v1","body":"fixture"})).await.unwrap();
    s.commit().await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let request = ModelRequest::build(
        ModelRequestContext {
            request_id: "clock-request".into(),
            namespace: "clock-ns".into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "clock-episode".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: hash(b"parent"),
            bundle_digest: hash(b"bundle"),
            source_closure: vec![EvidenceRef {
                id: "clock-source".into(),
                digest: hash(b"clock-source"),
            }],
            model_digest: hash(b"model"),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: hash(b"sampling"),
            revoke_watermark: 0,
            max_suggestions: 4,
        },
        vec![ModelInputPart {
            role: ModelInputRole::User,
            label: "task".into(),
            content: "fixture only".into(),
        }],
    )
    .unwrap();
    let broker = PersistentModelBroker::with_clock(
        store.clone(),
        CountingFixture(calls.clone()),
        BrokerConfig {
            billing_scope: "clock-scope".into(),
            actor: "clock-broker".into(),
            currency: "USD".into(),
            pricing_version: "fixture-v1".into(),
            max_cost_micros: 20,
            lease_seconds: 60,
        },
        Arc::new(move || clock),
    )
    .unwrap();
    let error = broker.dispatch(request).await.unwrap_err();
    assert!(matches!(error, Error::ClockRegressed));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let root = store
        .root_budget(&admin, "clock-scope")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((root.spent_micros, root.reserved_micros), (0, 0));
    assert!(
        store
            .budget_call(&admin, "clock-scope", "clock-request")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.verify_audit(&admin).await.unwrap(), 0);
    let mut s = store.session().await.unwrap();
    assert_eq!(s.namespace_object_count(&admin, "run").await.unwrap(), 1);
    assert_eq!(
        s.namespace_object_count(&admin, "artifact").await.unwrap(),
        0
    );
    s.commit().await.unwrap();
    store.close().await;
}
#[tokio::test]
async fn negative_one_refuses_before_reservation_and_fixture_dispatch() {
    refuses(-1).await;
}
#[tokio::test]
async fn minimum_refuses_before_reservation_and_fixture_dispatch() {
    refuses(i64::MIN).await;
}
