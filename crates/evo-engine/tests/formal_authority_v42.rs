//! Formal authority uses explicit control registrations and actual private
//! evidence. These software tests do not claim an external paid experiment.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, SkillSnapshot};
use evo_core::{Context, Error, Result, Role, Strategy, hash};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::formal_authority::{
    FormalAuthorityIssuer, FormalRegistry, RegisteredExternalTransport,
};
use evo_engine::model::{FormalTaskRequest, ModelExecutionProvenance};
use evo_engine::streaming_evaluator::{BrokerTaskView, ExecutionSide};
use evo_storage::Store;

fn fixture() -> trusted_compiler_inputs::FixtureInputs {
    trusted_compiler_inputs::inputs(
        "n",
        "profile",
        SkillSnapshot::empty(),
        Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        &hash(b"env"),
    )
}
#[tokio::test]
async fn explicit_empty_root_has_no_ordinary_p_or_provider_execution_credit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = fixture();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    assert!(matches!(
        FormalRegistry::register_host(&worker, &store, f.host.clone()).await,
        Err(Error::Forbidden)
    ));
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    assert!(
        RegisteredExternalTransport::connect(&host, &store, &f.provider.id)
            .await
            .is_err()
    );
    assert!(
        FormalAuthorityIssuer::revalidate(&worker, &store, "missing-report", "missing-candidate")
            .await
            .is_err()
    );
    let mut s = store.session().await.unwrap();
    assert!(
        s.get::<serde_json::Value>(&worker, "artifact", "formal-authority-missing-report")
            .await
            .unwrap()
            .is_none()
    );
    s.commit().await.unwrap();
    store.close().await;
}
#[tokio::test]
async fn empty_root_rejects_nonempty_skill_nonfixed_strategy_and_wrong_authorizer() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = fixture();
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let admin = Context::new("n", "fixture-admin", Role::Admin).unwrap();
    for mutation in 0..3 {
        let mut root = f.root.clone();
        root.id = format!("invalid-root-{mutation}");
        match mutation {
            0 => root.skill.content = "caller supplied root".into(),
            1 => root.strategy.instruction = "caller supplied strategy".into(),
            _ => root.authorized_by = "someone-else".into(),
        }
        assert!(
            FormalRegistry::authorize_empty_root(&admin, &store, root.clone())
                .await
                .is_err()
        );
        let mut s = store.session().await.unwrap();
        assert!(
            s.get::<serde_json::Value>(&admin, "artifact", &root.id)
                .await
                .unwrap()
                .is_none()
        );
        s.commit().await.unwrap();
    }
    store.close().await;
}
struct LabeledTransport;
#[async_trait]
impl ModelTransport for LabeledTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::ExternalProvider)
    }
    async fn execute(
        &self,
        _: &evo_core::optimization::ModelRequest,
        _: &str,
    ) -> Result<TransportCompletion> {
        panic!("untrusted labeled transport must never execute")
    }
}
fn labeled_request() -> FormalTaskRequest {
    FormalTaskRequest {
        task: BrokerTaskView {
            ticket_id: "ticket".into(),
            target_id: "target".into(),
            side: ExecutionSide::Candidate,
            input_artifact_id: "input".into(),
            input_digest: hash(b"input"),
            bundle_digest: hash(b"bundle"),
            environment_digest: hash(b"env"),
            request_digest: hash(b"request"),
        },
        candidate_id: "candidate".into(),
        profile_registration_id: "profile".into(),
        compiler_binding_digest: hash(b"binding"),
        model_digest: hash(b"model"),
        mandatory_context: "mandatory".into(),
        projection: evo_core::contract::RunProjection {
            bundle_digest: hash(b"bundle"),
            instructions: vec![],
            tools: vec![],
        },
        task_input_utf8: "input".into(),
    }
}
#[tokio::test]
async fn external_provider_label_cannot_reserve_dispatch_or_mint_formal_capture() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let broker = PersistentModelBroker::new(
        store.clone(),
        LabeledTransport,
        BrokerConfig {
            billing_scope: "billing".into(),
            actor: "host".into(),
            currency: "USD".into(),
            pricing_version: "v1".into(),
            max_cost_micros: 1,
            lease_seconds: 60,
        },
    )
    .unwrap();
    let request = labeled_request();
    let call_id = format!(
        "formal-call-{}",
        &hash(request.task.request_digest.as_bytes())[..32]
    );
    assert!(matches!(
        broker.dispatch_formal_task("n", request).await,
        Err(Error::Forbidden)
    ));
    let ctx = Context::new("n", "host", Role::Host).unwrap();
    assert!(
        store
            .budget_call(&ctx, "billing", &call_id)
            .await
            .unwrap()
            .is_none()
    );
    let mut s = store.session().await.unwrap();
    assert!(
        s.get::<serde_json::Value>(&ctx, "artifact", "candidate")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        s.get::<serde_json::Value>(&ctx, "artifact", "formal-cost-ticket")
            .await
            .unwrap()
            .is_none()
    );
    s.commit().await.unwrap();
    store.close().await;
}
#[tokio::test]
async fn unknown_or_missing_issued_evidence_never_becomes_a_restored_approval() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = fixture();
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let candidate = trusted_compiler_inputs::software_candidate("n", &hash(b"env"), "candidate");
    let mut s = store.session().await.unwrap();
    s.put(&worker,"artifact","formal-authority-report","evaluator",&serde_json::json!({"id":"formal-authority-report","schema_version":"rsia.issued_formal_authority.v99","ordinary_p_eligible":true})).await.unwrap();
    assert!(
        evo_engine::formal_authority::read_typed_closure(
            &mut s,
            &worker,
            &[("artifact".into(), "formal-authority-report".into())]
        )
        .await
        .is_err()
    );
    assert!(
        s.get::<serde_json::Value>(&worker, "release", &candidate.digest)
            .await
            .unwrap()
            .is_none()
    );
    s.commit().await.unwrap();
    store.close().await;
}
#[tokio::test]
async fn cost_recording_refuses_missing_whole_root_measurements_without_source_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = fixture();
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    let measurements = evo_core::evaluation::CompleteEpisodeCostMeasurements {
        episode_id: "episode".into(),
        cpu_nanoseconds: None,
        storage_byte_seconds: Some(0),
        retained_history_bytes: Some(0),
        human_nanoseconds: Some(0),
        gray_release_nanoseconds: Some(0),
        measurement_sources: vec!["meter".into()],
    };
    assert!(
        FormalAuthorityIssuer::record_episode_cost(
            &host,
            &store,
            "ticket",
            &f.frozen.id,
            measurements,
            vec![]
        )
        .await
        .is_err()
    );
    let mut s = store.session().await.unwrap();
    assert!(
        s.get::<serde_json::Value>(&host, "artifact", "formal-cost-ticket")
            .await
            .unwrap()
            .is_none()
    );
    s.commit().await.unwrap();
    store.close().await;
}

#[test]
fn four_principal_descriptor_rejects_uid_credential_alias_and_config_is_not_observation() {
    use evo_engine::formal_authority::{
        PRINCIPALS_SCHEMA, PrincipalAuthorityService, RegisteredPrincipal, RegisteredPrincipalSet,
    };
    let f = fixture();
    let mut host = f.host;
    let principal = |uid, actor: &str, secret: &str| RegisteredPrincipal {
        uid,
        actor: actor.into(),
        principal: hash(secret.as_bytes()),
        credential_digest: hash(secret.as_bytes()),
    };
    let set = RegisteredPrincipalSet {
        id: format!("formal-principals-{}", host.id),
        schema_version: PRINCIPALS_SCHEMA.into(),
        namespace: host.namespace.clone(),
        host_registration_id: host.id.clone(),
        version: 1,
        host: principal(
            host.host_uid,
            "host",
            "host-independent-private-key-32-bytes",
        ),
        grader: principal(
            host.grader_uid,
            "grader",
            "grader-independent-private-key-32-bytes",
        ),
        administrator: principal(
            host.administrator_uid,
            "admin",
            "admin-independent-private-key-32-bytes",
        ),
        worker: principal(10004, "worker", "worker-independent-private-key-32-bytes"),
    };
    host.host_principal = set.host.principal.clone();
    host.grader_principal = set.grader.principal.clone();
    host.administrator_principal = set.administrator.principal.clone();
    host.worker_principal = set.worker.principal.clone();
    host.credential_binding_digest = set.binding_digest().unwrap();
    set.validate(&host).unwrap();
    // A well-shaped four-UID configuration has no authority capability. Its
    // absent private workspace/code/credential observations refuse construction.
    assert!(PrincipalAuthorityService::new(host.clone(), set.clone()).is_err());
    for mutation in 0..4 {
        let mut bad = set.clone();
        match mutation {
            0 => bad.worker.uid = bad.administrator.uid,
            1 => bad.worker.credential_digest = bad.administrator.credential_digest.clone(),
            2 => bad.worker.actor = bad.grader.actor.clone(),
            _ => bad.worker.principal = bad.host.principal.clone(),
        }
        let mut current = host.clone();
        current.worker_principal = bad.worker.principal.clone();
        current.credential_binding_digest = bad.binding_digest().unwrap();
        assert!(bad.validate(&current).is_err());
    }
}
#[tokio::test]
async fn copied_public_principal_body_or_admin_alias_does_not_authenticate_formal_transport() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = fixture();
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let worker = Context::new("n", "fixture-admin", Role::Worker).unwrap();
    let id = format!("formal-principals-{}", f.host.id);
    let mut s = store.session().await.unwrap();
    s.put(&worker,"artifact",&id,worker.actor(),&serde_json::json!({"id":id,"schema_version":"rsia.formal_principal_set_observation.v1","namespace":"n","grader_uid":f.host.grader_uid,"ordinary_p_eligible":true})).await.unwrap();
    s.commit().await.unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    assert!(
        RegisteredExternalTransport::connect(&host, &store, &f.provider.id)
            .await
            .is_err()
    );
    let mut s = store.session().await.unwrap();
    assert!(
        s.get::<serde_json::Value>(&worker, "artifact", "formal-authority-ticket")
            .await
            .unwrap()
            .is_none()
    );
    s.commit().await.unwrap();
    store.close().await;
}
