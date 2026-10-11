#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::contract::{HostCapabilities, ImproverPatch, SkillSnapshot};
use evo_core::evidence::Purpose;
use evo_core::{Context, Role, Strategy, fingerprint, hash};
use evo_engine::formal_authority::{CompilerInputRequest, FormalRegistry, resolve_compiler_inputs};
use evo_storage::Store;

#[tokio::test]
async fn replay_binds_all_eight_origins_strategy_hash_and_untouched_search_parent_leaves() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = trusted_compiler_inputs::inputs(
        "n",
        "p",
        SkillSnapshot::empty(),
        Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        &hash(b"environment"),
    );
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let search = SkillSnapshot {
        content: "kept search content".into(),
        applicability: "kept applicability".into(),
        counterexample: "kept counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    let strategy = Strategy {
        instruction: "search strategy retained".into(),
        ..Strategy::default()
    };
    let request = CompilerInputRequest {
        profile: f.profile.clone(),
        search_parent: search.clone(),
        baseline: f.frozen.baseline_skill.clone(),
        search_parent_strategy: strategy.clone(),
        expected_parent_strategy_digest: Some(fingerprint(&f.root.strategy).unwrap()),
        baseline_strategy: f.frozen.baseline_strategy.clone(),
        capabilities: f.frozen.capabilities.clone(),
        environment_digest: f.frozen.environment_digest.clone(),
        purpose: Purpose::Development,
        revoke_watermark: 0,
    };
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let mut s = store.session().await.unwrap();
    let inputs = resolve_compiler_inputs(&mut s, &worker, &request)
        .await
        .unwrap();
    let mut desired = search.clone();
    desired.content.push_str(" edited");
    let bundle = inputs
        .compile_search_edit(&desired, &ImproverPatch::default())
        .unwrap();
    assert_eq!(bundle.skill.applicability, search.applicability);
    assert_eq!(bundle.skill.counterexample, search.counterexample);
    assert_eq!(bundle.improver.instruction, strategy.instruction);
    assert_eq!(bundle.origins.len(), 8);
    inputs.replay(&bundle).unwrap();
    for change in 0..5 {
        let mut forged = bundle.clone();
        match change {
            0 => {
                forged.origins.pop();
            }
            1 => {
                forged.origins[1] = forged.origins[0].clone();
            }
            2 => forged.origins[0].source_digest = hash(b"wrong source"),
            3 => forged.improver.instruction.push_str(" substituted"),
            _ => forged.digest = hash(b"wrong hash"),
        }
        assert!(inputs.replay(&forged).is_err(), "forgery {change}");
    }
    drop(s);
    store.close().await;
}

#[tokio::test]
async fn admin_profile_update_refuses_replayed_old_selection_and_old_cache_binding() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = trusted_compiler_inputs::inputs(
        "n",
        "p",
        SkillSnapshot::empty(),
        Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        &hash(b"environment"),
    );
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let admin = Context::new("n", "fixture-admin", Role::Admin).unwrap();
    let id = "formal-profile-p";
    let mut s = store.session().await.unwrap();
    let old: serde_json::Value = s.need(&worker, "artifact", id).await.unwrap();
    s.commit().await.unwrap();
    let mut new = f.frozen.clone();
    new.id.push_str("-v2");
    new.version = 2;
    new.baseline_skill.content = "new B".into();
    FormalRegistry::freeze_profile(&admin, &store, new)
        .await
        .unwrap();
    let mut s = store.session().await.unwrap();
    s.put(&worker, "artifact", id, "fixture-admin", &old)
        .await
        .unwrap();
    let root = vec![("artifact".into(), id.into())];
    assert!(
        evo_engine::formal_authority::read_typed_closure(&mut s, &worker, &root)
            .await
            .is_err()
    );
    drop(s);
    store.close().await;
}
