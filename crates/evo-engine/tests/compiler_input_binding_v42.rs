#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::contract::{HostCapabilities, ImproverPatch, PatchOp, SkillPatch, SkillSnapshot};
use evo_core::evidence::Purpose;
use evo_core::{Context, Role, Strategy, fingerprint, hash};
use evo_engine::formal_authority::{
    CompilerInputRequest, read_typed_closure, resolve_compiler_inputs,
};
use evo_storage::Store;
use serde_json::json;

fn request(f: &trusted_compiler_inputs::FixtureInputs) -> CompilerInputRequest {
    CompilerInputRequest {
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
    }
}
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
        &hash(b"environment"),
    )
}

#[tokio::test]
async fn verified_optional_parent_strategy_assertion_persists_one_bundle_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = fixture();
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let mut without_assertion = request(&f);
    without_assertion.expected_parent_strategy_digest = None;
    let with_assertion = request(&f);
    let skill = SkillPatch {
        content: PatchOp::Set {
            value: "canonical persisted candidate".into(),
        },
        ..SkillPatch::default()
    };
    let mut session = store.session().await.unwrap();
    let first = evo_engine::compiler::compile_in_session(
        &mut session,
        &worker,
        &without_assertion,
        &skill,
        &ImproverPatch::default(),
    )
    .await
    .unwrap();
    let record_id = format!("formal-bundle-{}", first.digest);
    let original: evo_engine::compiler::AuthenticatedBundleRecord =
        session.need(&worker, "artifact", &record_id).await.unwrap();
    let second = evo_engine::compiler::compile_in_session(
        &mut session,
        &worker,
        &with_assertion,
        &skill,
        &ImproverPatch::default(),
    )
    .await
    .unwrap();
    let repeated: evo_engine::compiler::AuthenticatedBundleRecord =
        session.need(&worker, "artifact", &record_id).await.unwrap();
    assert_eq!(second.digest, first.digest);
    assert_eq!(
        fingerprint(&repeated).unwrap(),
        fingerprint(&original).unwrap()
    );
    assert_eq!(
        repeated.input.expected_parent_strategy_digest,
        Some(fingerprint(&f.root.strategy).unwrap())
    );
    let mut wrong = with_assertion;
    wrong.expected_parent_strategy_digest = Some(hash(b"different actual parent strategy"));
    assert!(
        evo_engine::compiler::persist_authenticated_bundle(&mut session, &worker, &wrong, &first,)
            .await
            .is_err()
    );
    let unchanged: evo_engine::compiler::AuthenticatedBundleRecord =
        session.need(&worker, "artifact", &record_id).await.unwrap();
    assert_eq!(
        fingerprint(&unchanged).unwrap(),
        fingerprint(&original).unwrap()
    );
    session.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn full_original_inputs_are_resolved_before_all_set_and_output_reads() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = fixture();
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let r = request(&f);
    let mut s = store.session().await.unwrap();
    let v = resolve_compiler_inputs(&mut s, &worker, &r).await.unwrap();
    let skill = SkillPatch {
        content: PatchOp::Set {
            value: "all set".into(),
        },
        applicability: PatchOp::Set {
            value: "applies".into(),
        },
        counterexample: PatchOp::Set {
            value: "counter".into(),
        },
        required_capabilities: PatchOp::Set { value: vec![] },
        dependencies: PatchOp::Set { value: vec![] },
    };
    let bundle = v.compile(&skill, &ImproverPatch::default()).unwrap();
    assert_eq!(bundle.origins.len(), 8);
    v.replay(&bundle).unwrap();
    s.commit().await.unwrap();
    for mutation in 0..7 {
        let mut wrong = r.clone();
        match mutation {
            0 => wrong.profile.parent_digest = hash(b"caller-label"),
            1 => wrong.baseline.content = "substituted baseline".into(),
            2 => wrong.baseline_strategy.instruction = "substituted baseline strategy".into(),
            3 => {
                wrong.capabilities.granted.insert("extra".into());
            }
            4 => wrong.environment_digest = hash(b"other environment"),
            5 => wrong.revoke_watermark = 1,
            _ => wrong.expected_parent_strategy_digest = Some(hash(b"substituted P strategy")),
        }
        let mut s = store.session().await.unwrap();
        assert!(
            resolve_compiler_inputs(&mut s, &worker, &wrong)
                .await
                .is_err(),
            "mutation {mutation}"
        );
    }
    store.close().await;
}

#[tokio::test]
async fn unknown_version_wrong_kind_alias_missing_and_cycle_are_refused_but_dag_is_complete() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = fixture();
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let mut s = store.session().await.unwrap();
    for (id, schema) in [
        ("unknown", "rsia.unknown.v99"),
        ("wrong-kind", "rsia.optimization.source.v1"),
        ("forged-known", "rsia.optimization.stage_fact.v1"),
    ] {
        s.put(
            &worker,
            "artifact",
            id,
            worker.actor(),
            &json!({"id":id,"schema_version":schema}),
        )
        .await
        .unwrap();
    }
    for id in ["unknown", "wrong-kind", "forged-known", "missing"] {
        assert!(
            read_typed_closure(&mut s, &worker, &[("artifact".into(), id.into())])
                .await
                .is_err(),
            "{id}"
        );
    }
    let mut ids = Vec::new();
    for episode in ["a", "b", "c"] {
        let fact = evo_engine::optimization::StageFact {
            schema_version: evo_engine::optimization::OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: "pending".into(),
            namespace: "n".into(),
            episode_id: episode.into(),
            step: 0,
            attempt: 0,
            stage: evo_engine::optimization::OptimizationJournalStage::Merge,
            kind: evo_engine::optimization::StageFactKind::StepPrepared,
            request_id: episode.into(),
            input_digest: hash(episode.as_bytes()),
            output_digest: None,
            dependencies: vec![],
            payload: json!({"request":episode}),
        }
        .seal()
        .unwrap();
        s.put(
            &worker,
            "artifact",
            &fact.artifact_id,
            worker.actor(),
            &fact,
        )
        .await
        .unwrap();
        ids.push(fact.artifact_id);
    }
    let alias: serde_json::Value = s.need(&worker, "artifact", &ids[0]).await.unwrap();
    s.put(&worker, "artifact", "alias", worker.actor(), &alias)
        .await
        .unwrap();
    assert!(
        read_typed_closure(&mut s, &worker, &[("artifact".into(), "alias".into())])
            .await
            .is_err()
    );
    for (from, to) in [(0, 1), (0, 2), (1, 2)] {
        s.put_edge(&worker, "artifact", &ids[from], "artifact", &ids[to])
            .await
            .unwrap();
    }
    let dag = read_typed_closure(&mut s, &worker, &[("artifact".into(), ids[0].clone())])
        .await
        .unwrap();
    assert_eq!(dag.nodes.len(), 3);
    assert_eq!(dag.edges.len(), 3);
    s.put_edge(&worker, "artifact", &ids[2], "artifact", &ids[0])
        .await
        .unwrap();
    assert!(
        read_typed_closure(&mut s, &worker, &[("artifact".into(), ids[0].clone())])
            .await
            .is_err()
    );
    drop(s);
    store.close().await;
}

#[tokio::test]
async fn registered_content_requires_latest_actual_admin_witness() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let f = fixture();
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let r = request(&f);
    let mut s = store.session().await.unwrap();
    let original: serde_json::Value = s.need(&worker, "artifact", &f.host.id).await.unwrap();
    let mut forged = original.clone();
    forged["code_digest"] = json!(hash(b"forged code"));
    s.put(&worker, "artifact", &f.host.id, "fixture-admin", &forged)
        .await
        .unwrap();
    // Copying the operation and entity through Worker audit cannot become Admin.
    let operation = format!(
        "formal.admin.content.v1.{}",
        fingerprint(&("n", "artifact", &f.host.id)).unwrap()
    );
    let entity = fingerprint(&(
        "n",
        "artifact",
        &f.host.id,
        evo_engine::formal_authority::content_digest(&forged).unwrap(),
    ))
    .unwrap();
    s.audit(&worker, &operation, &entity).await.unwrap();
    s.commit().await.unwrap();
    let mut s = store.session().await.unwrap();
    assert!(resolve_compiler_inputs(&mut s, &worker, &r).await.is_err());
    s.put(&worker, "artifact", &f.host.id, worker.actor(), &original)
        .await
        .unwrap();
    // Immutable unchanged authorized content can be restored; no new authority is minted.
    assert!(resolve_compiler_inputs(&mut s, &worker, &r).await.is_ok());
    s.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn typed_envelopes_refuse_positional_payloads_and_unknown_nested_versions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).await.unwrap();
    let worker = Context::new("n", "worker", Role::Worker).unwrap();
    let mut session = store.session().await.unwrap();
    for (name, payload) in [
        (
            "positional",
            json!([
                "rsia.execution_output.v1",
                "output",
                "request",
                "text",
                "digest"
            ]),
        ),
        (
            "unknown-version",
            json!({"schema_version":"rsia.execution_output.v99","id":"output","output_utf8":"text","output_digest":hash(b"text")}),
        ),
    ] {
        let id = format!(
            "e05-{}",
            fingerprint(&("evaluation_execution_output_v1", "output")).unwrap()
        );
        let body = json!({"schema_version":"rsia.typed_artifact_envelope.v1","id":id,"record_kind":"evaluation_execution_output_v1","payload":payload});
        session
            .put(&worker, "artifact", &id, "worker", &body)
            .await
            .unwrap();
        assert!(
            read_typed_closure(&mut session, &worker, &[("artifact".into(), id)])
                .await
                .is_err(),
            "{name}"
        );
    }
    let roots = vec![("artifact".into(), "missing".into()); 10_001];
    assert!(
        read_typed_closure(&mut session, &worker, &roots)
            .await
            .is_err()
    );
    drop(session);
    store.close().await;
}
