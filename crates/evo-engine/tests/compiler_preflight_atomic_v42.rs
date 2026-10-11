//! Real Session preflight and rollback, including idempotent candidate reads.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::contract::{HostCapabilities, SkillSnapshot};
use evo_core::{Context, Role, Strategy, fingerprint, hash};
use evo_engine::formal_authority::{CompilerInputRequest, resolve_compiler_inputs};
use evo_engine::release_store::{ReleaseStore, StageBundleRequest, TypedSourceRef};
use evo_storage::Store;

#[tokio::test]
async fn absent_control_authority_refuses_all_set_before_candidate_or_binding_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let b = trusted_compiler_inputs::software_candidate("n", &hash(b"env"), "set everything");
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let result = ReleaseStore::stage_bundle(
        &worker,
        &store,
        StageBundleRequest {
            candidate_id: "candidate".into(),
            bundle: b.clone(),
            environment_digest: hash(b"env"),
            proposer_actor: worker.actor().into(),
            sources: vec![TypedSourceRef {
                kind: "run".into(),
                id: "source".into(),
                content_digest: hash(b"source"),
            }],
            revoke_watermark: 0,
        },
    )
    .await;
    assert!(result.is_err());
    let mut s = store.session().await.unwrap();
    for id in [
        "candidate".into(),
        format!("formal-bundle-{}", b.digest),
        format!(
            "compiler-binding-{}",
            fingerprint(&("candidate", &b.digest)).unwrap()
        ),
    ] {
        assert!(
            s.get::<serde_json::Value>(&worker, "artifact", &id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            s.dependency_edges(&worker, "artifact", &id)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(s.watermark(&worker).await.unwrap(), None);
    s.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn latest_profile_update_is_seen_inside_the_callers_existing_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = trusted_compiler_inputs::inputs(
        "n",
        "profile",
        SkillSnapshot::empty(),
        Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        &hash(b"env"),
    );
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let r = CompilerInputRequest {
        profile: f.profile.clone(),
        search_parent: SkillSnapshot::empty(),
        baseline: f.frozen.baseline_skill.clone(),
        search_parent_strategy: Strategy::default(),
        expected_parent_strategy_digest: None,
        baseline_strategy: f.frozen.baseline_strategy.clone(),
        capabilities: f.frozen.capabilities.clone(),
        environment_digest: f.frozen.environment_digest.clone(),
        purpose: evo_core::evidence::Purpose::Development,
        revoke_watermark: 0,
    };
    let mut s = store.session().await.unwrap();
    let good = resolve_compiler_inputs(&mut s, &worker, &r).await.unwrap();
    let id = format!("formal-profile-{}", f.profile.id);
    let original: serde_json::Value = s.need(&worker, "artifact", &id).await.unwrap();
    let mut substituted = original.clone();
    substituted["registration_digest"] = serde_json::json!(hash(b"other"));
    s.put(&worker, "artifact", &id, worker.actor(), &substituted)
        .await
        .unwrap();
    assert!(resolve_compiler_inputs(&mut s, &worker, &r).await.is_err());
    // Dropping the same transaction rolls back every write; no nested Session commits it.
    drop(s);
    let mut s = store.session().await.unwrap();
    let restored = resolve_compiler_inputs(&mut s, &worker, &r).await.unwrap();
    assert_eq!(
        good.binding().binding_digest,
        restored.binding().binding_digest
    );
    s.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn full_compiler_use_gate_and_effect_share_the_same_caller_transaction() {
    use evo_engine::model::CompilerInputUse;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = trusted_compiler_inputs::inputs(
        "n",
        "profile",
        SkillSnapshot::empty(),
        Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        &hash(b"env"),
    );
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let r = CompilerInputRequest {
        profile: f.profile.clone(),
        search_parent: SkillSnapshot::empty(),
        baseline: f.frozen.baseline_skill.clone(),
        search_parent_strategy: Strategy::default(),
        expected_parent_strategy_digest: None,
        baseline_strategy: f.frozen.baseline_strategy.clone(),
        capabilities: f.frozen.capabilities.clone(),
        environment_digest: f.frozen.environment_digest.clone(),
        purpose: evo_core::evidence::Purpose::Development,
        revoke_watermark: 0,
    };
    let mut s = store.session().await.unwrap();
    let inputs = resolve_compiler_inputs(&mut s, &worker, &r).await.unwrap();
    let usage = CompilerInputUse::new(r.clone(), inputs.binding().clone());
    usage.verify_current(&mut s, &worker).await.unwrap();
    s.put(
        &worker,
        "artifact",
        "same-session-effect",
        worker.actor(),
        &serde_json::json!({"schema_version":"rsia.fixture_effect.v1"}),
    )
    .await
    .unwrap();
    let id = format!("formal-profile-{}", f.profile.id);
    let original: serde_json::Value = s.need(&worker, "artifact", &id).await.unwrap();
    let mut bad = original.clone();
    bad["registration_digest"] = serde_json::json!(hash(b"substitution"));
    s.put(&worker, "artifact", &id, worker.actor(), &bad)
        .await
        .unwrap();
    assert!(usage.verify_current(&mut s, &worker).await.is_err());
    drop(s);
    let mut s = store.session().await.unwrap();
    assert!(
        s.get::<serde_json::Value>(&worker, "artifact", "same-session-effect")
            .await
            .unwrap()
            .is_none()
    );
    usage.verify_current(&mut s, &worker).await.unwrap();
    s.commit().await.unwrap();
    let admin = Context::new("n", "fixture-admin", Role::Admin).unwrap();
    let mut updated = f.frozen.clone();
    updated.id = format!("{}-next", updated.id);
    updated.version += 1;
    updated.environment_digest = hash(b"new-environment");
    evo_engine::formal_authority::FormalRegistry::freeze_profile(&admin, &store, updated)
        .await
        .unwrap();
    let mut s = store.session().await.unwrap();
    assert!(usage.verify_current(&mut s, &worker).await.is_err());
    assert_eq!(s.watermark(&worker).await.unwrap(), None);
    drop(s);
    store.close().await;
}
