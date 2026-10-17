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
    let strategy = Strategy::default();
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

#[test]
fn pure_compiler_keeps_custom_strategy_semantics_without_production_authority() {
    use evo_core::contract::{
        CompileParts, Profile, SkillPatch, compile_bundle, replay_resolved_bundle,
    };
    let skill = SkillSnapshot::empty();
    let strategy = Strategy {
        instruction: "pure historical strategy".into(),
        ..Strategy::default()
    };
    let baseline = Strategy::default();
    let caps = HostCapabilities {
        available: Default::default(),
        granted: Default::default(),
    };
    let profile = Profile {
        id: "pure".into(),
        evolution_enabled: true,
        parent_digest: hash(b"P"),
        baseline_digest: hash(b"B"),
    };
    let bundle = compile_bundle(CompileParts {
        profile: &profile,
        parent: &skill,
        baseline: &skill,
        parent_strategy: &strategy,
        baseline_strategy: &baseline,
        skill_patch: &SkillPatch::default(),
        improver_patch: &ImproverPatch::default(),
        caps: &caps,
        revoked: &Default::default(),
    })
    .unwrap();
    assert_eq!(
        fingerprint(&bundle.improver).unwrap(),
        fingerprint(&strategy).unwrap()
    );
    assert_eq!(bundle.origins.len(), 8);
    replay_resolved_bundle(
        &bundle,
        &skill,
        &skill,
        &strategy,
        &baseline,
        &caps,
        &Default::default(),
    )
    .unwrap();
}

#[tokio::test]
async fn audit_f01_production_compiler_rejects_custom_search_and_explicit_patch() {
    use evo_core::contract::{PatchOp, SkillPatch};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = trusted_compiler_inputs::inputs(
        "f01compiler",
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
    let worker = Context::new("f01compiler", "worker", Role::Worker).unwrap();
    let request = CompilerInputRequest {
        profile: f.profile.clone(),
        search_parent: SkillSnapshot::empty(),
        baseline: f.frozen.baseline_skill.clone(),
        search_parent_strategy: Strategy::default(),
        expected_parent_strategy_digest: Some(fingerprint(&f.root.strategy).unwrap()),
        baseline_strategy: f.frozen.baseline_strategy.clone(),
        capabilities: f.frozen.capabilities.clone(),
        environment_digest: f.frozen.environment_digest.clone(),
        purpose: Purpose::Development,
        revoke_watermark: 0,
    };
    let mut s = store.session().await.unwrap();
    let verified = resolve_compiler_inputs(&mut s, &worker, &request)
        .await
        .unwrap();
    verified
        .compile_search_edit(&SkillSnapshot::empty(), &ImproverPatch::default())
        .unwrap();
    for patch in [
        ImproverPatch {
            instruction: PatchOp::Set {
                value: "unapproved generator".into(),
            },
            ..Default::default()
        },
        ImproverPatch {
            max_candidates: PatchOp::Set { value: 3 },
            ..Default::default()
        },
    ] {
        assert!(matches!(
            verified.compile(&SkillPatch::default(), &patch),
            Err(evo_core::Error::Forbidden)
        ));
        assert!(matches!(
            verified.compile_search_edit(&SkillSnapshot::empty(), &patch),
            Err(evo_core::Error::Forbidden)
        ));
    }
    for change in 0..3 {
        let mut altered = request.clone();
        match change {
            0 => altered
                .search_parent_strategy
                .instruction
                .push_str(" unapproved"),
            1 => altered.search_parent_strategy.max_candidates = 3,
            _ => altered.search_parent_strategy.require_counterexample = false,
        }
        assert!(matches!(
            resolve_compiler_inputs(&mut s, &worker, &altered).await,
            Err(evo_core::Error::Forbidden)
        ));
    }
    drop(s);
    store.close().await;
}

#[tokio::test]
async fn audit_f01_profile_cannot_register_nonbuiltin_baseline_strategy() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = trusted_compiler_inputs::inputs(
        "f01baseline",
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
    let admin = Context::new("f01baseline", "fixture-admin", Role::Admin).unwrap();
    let mut changed = f.frozen.clone();
    changed.id.push_str("-changed");
    changed.version = 2;
    changed
        .baseline_strategy
        .instruction
        .push_str(" unapproved");
    let id = changed.id.clone();
    assert!(matches!(
        FormalRegistry::freeze_profile(&admin, &store, changed).await,
        Err(evo_core::Error::Forbidden)
    ));
    let mut s = store.session().await.unwrap();
    assert!(
        s.get::<serde_json::Value>(&admin, "artifact", &id)
            .await
            .unwrap()
            .is_none()
    );
    drop(s);
    store.close().await;
}
