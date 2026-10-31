use evo_core::evidence::Purpose;
use evo_core::replay::{
    HistoricalUsage, PrefixCoverageV1, REPLAY_MANIFEST_SCHEMA, REPLAY_REPORT_SCHEMA,
    REPLAY_WORLD_SCHEMA, ReplayActionSpecV1, ReplayCoverage, ReplayPoolManifestV1, ReplayReportV2,
    ReplaySimulationProfile, ReplaySourceRef, ReplayTerminal, ReplayTransitionOutcome,
    ReplayTransitionV2, ReplayWorldManifestV2, ReplayWorldV2, StoredReplayReportV1, WorldPartition,
};
use evo_core::replay::{
    REPLAY_REPORT_SCHEMA_V3, RationalValue, ReplayPolicyStopExclusions, ReplayReportV3,
    ReplayRootContractState, StoredReplayReportV2,
};
use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
use evo_core::{Context, Role, fingerprint, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{BackupManifest, CleanupState, LifecycleStore, TypedObjectRef};
use evo_storage::replay::{
    put_current_replay_report, put_replay_report, register_replay_pool, seal_replay_world,
};
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::process::Command;

fn ctx(actor: &str, role: Role) -> Context {
    Context::new("n", actor, role).unwrap()
}

async fn database() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    (dir, store)
}

async fn put_source(store: &Store, id: &str, marker: &str) {
    let host = ctx("host", Role::Host);
    let mut session = store.session().await.unwrap();
    session
        .put(
            &host,
            "run",
            id,
            host.actor(),
            &json!({"id":id,"schema_version":"rsia.optimization.source.v1","body":marker}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

fn e16_import_envelope(
    schema: &str,
    id: &str,
    source_refs: serde_json::Value,
    payload: serde_json::Value,
) -> serde_json::Value {
    json!({
        "schema_version":schema,
        "id":id,
        "namespace":"n",
        "owner_actor":"admin",
        "request_key":format!("request-{id}"),
        "input_digest":hash(format!("input-{id}").as_bytes()),
        "created_at":1,
        "updated_at":1,
        "source_refs":source_refs,
        "revoke_watermark":0,
        "payload":payload,
    })
}

fn lifecycle_replay_world(id: &str, cluster: &str, partition: WorldPartition) -> ReplayWorldV2 {
    let transition = ReplayTransitionV2 {
        record_id: "replay-record".into(),
        record_seq: 1,
        generation_signature: hash(b"replay-generation"),
        parent_context_signature: hash(b"replay-parent-context"),
        action_kind: ActionKindV1::Widen { root_slot: 1 },
        next_context_signature: hash(b"replay-next"),
        outcome: ReplayTransitionOutcome::Observed {
            status: ObservedStatus::Valid {
                quality_micros: 600_000,
            },
        },
        actual_usage: HistoricalUsage {
            input_tokens: 2,
            output_tokens: 3,
            cost_micros: Some(7),
            latency_millis: Some(11),
        },
        source_ids: vec![format!("replay-observation-{id}"), "replay-source-2".into()],
        observation_source_id: format!("replay-observation-{id}"),
    };
    let mut world = ReplayWorldV2 {
        schema_version: REPLAY_WORLD_SCHEMA.into(),
        manifest: ReplayWorldManifestV2 {
            schema_version: REPLAY_MANIFEST_SCHEMA.into(),
            world_id: id.into(),
            cluster_id: cluster.into(),
            partition,
            purpose: Purpose::Development,
            generation_signature: hash(b"replay-generation"),
            world_context_signature: hash(format!("workspace-{id}").as_bytes()),
            baseline_context_signature: hash(b"replay-parent-context"),
            approved_parent_digest: hash(b"replay-parent"),
            model_digest: hash(b"replay-model"),
            tools_digest: hash(b"replay-tools"),
            scorer_digest: hash(b"replay-scorer"),
            guidance_digest: hash(b"replay-guidance"),
            repair_template_digest: hash(b"replay-repair"),
            input_order_digest: hash(format!("order-{id}").as_bytes()),
            initial_baseline_quality_micros: 100_000,
            baseline_observation_source_id: format!("replay-baseline-{id}"),
            source_closure: vec![
                ReplaySourceRef {
                    source_id: format!("replay-observation-{id}"),
                    content_digest: String::new(),
                },
                ReplaySourceRef {
                    source_id: "replay-source-2".into(),
                    content_digest: hash(b"REPLAY-SOURCE-SECRET"),
                },
                ReplaySourceRef {
                    source_id: format!("replay-baseline-{id}"),
                    content_digest: hash(b"pending-replay-baseline"),
                },
            ],
            revoke_watermark: 1,
            prefix_coverage: vec![PrefixCoverageV1 {
                context_signature: hash(b"replay-next"),
                exhausted: true,
            }],
            action_catalog: vec![ReplayActionSpecV1 {
                record_seq: 1,
                generation_signature: hash(b"replay-generation"),
                parent_context_signature: hash(b"replay-parent-context"),
                branch_seq: 1,
                target_depth: 1,
                action_kind: ActionKindV1::Widen { root_slot: 1 },
                estimated_cost_upper_micros: Some(7),
                writes_shared_workspace: false,
            }],
        },
        transitions: vec![transition],
        sealed_digest: None,
    };
    world.manifest.source_closure[2].content_digest =
        evo_core::replay::replay_baseline_observation_digest(&world.manifest).unwrap();
    world.manifest.source_closure[0].content_digest =
        evo_core::replay::replay_observation_digest(&world.manifest, &world.transitions[0])
            .unwrap();
    world.seal().unwrap();
    world
}

fn lifecycle_replay_report(
    world: &ReplayWorldV2,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    secret: &str,
) -> ReplayReportV2 {
    ReplayReportV2 {
        schema_version: REPLAY_REPORT_SCHEMA.into(),
        world_id: world.manifest.world_id.clone(),
        world_digest: world.sealed_digest.clone().unwrap(),
        cluster_id: world.manifest.cluster_id.clone(),
        partition: world.manifest.partition,
        policy_digest: fingerprint(policy).unwrap(),
        profile_digest: fingerprint(profile).unwrap(),
        objective_version: profile.objective.version().into(),
        batches: vec![],
        probe_best_quality_micros: vec![600_000],
        round_best_quality_micros: vec![600_000],
        final_quality_micros: 600_000,
        probes: 1,
        simulated_rounds: 1,
        parallel_penalty: None,
        no_probe: false,
        attainment_auc_micros: Some(600_000),
        q_auc_sim_micros: Some(600_000),
        score_v2_micros: Some(500_000),
        historical_usage: HistoricalUsage {
            input_tokens: 2,
            output_tokens: 3,
            cost_micros: Some(7),
            latency_millis: Some(11),
        },
        replay_cpu_nanos: 13,
        coverage: ReplayCoverage {
            attempted_actions: 1,
            observed_actions: 1,
            censored_actions: 0,
            out_of_support_actions: 0,
            complete_support: true,
        },
        terminal: ReplayTerminal::PolicyStop,
        terminal_reason: secret.into(),
        development_only: true,
        revealed_prefix: None,
    }
}

// Unscored current wire fixture; not an engine semantic verifier.
fn lifecycle_current_replay_report(
    world: &ReplayWorldV2,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    secret: &str,
) -> ReplayReportV3 {
    ReplayReportV3 {
        schema_version: REPLAY_REPORT_SCHEMA_V3.into(),
        world_id: world.manifest.world_id.clone(),
        world_digest: world.sealed_digest.clone().unwrap(),
        cluster_id: world.manifest.cluster_id.clone(),
        partition: world.manifest.partition,
        policy_digest: fingerprint(policy).unwrap(),
        profile_digest: fingerprint(profile).unwrap(),
        objective_version: profile.objective.version().into(),
        batches: vec![],
        probe_best_quality_micros: vec![600_000],
        round_best_quality_micros: vec![600_000],
        final_quality_micros: 600_000,
        probes: 1,
        simulated_rounds: 1,
        parallel_penalty: Some(RationalValue {
            numerator: 1,
            denominator: 1,
        }),
        no_probe: false,
        attainment_auc_micros: None,
        q_auc_sim_micros: None,
        score_v2_micros: None,
        historical_usage: HistoricalUsage {
            input_tokens: 2,
            output_tokens: 3,
            cost_micros: Some(7),
            latency_millis: Some(11),
        },
        replay_cpu_nanos: 13,
        coverage: ReplayCoverage {
            attempted_actions: 1,
            observed_actions: 1,
            censored_actions: 0,
            out_of_support_actions: 0,
            complete_support: false,
        },
        terminal: ReplayTerminal::OutOfSupport,
        terminal_reason: secret.into(),
        policy_stop_exclusions: ReplayPolicyStopExclusions::default(),
        root_contract_state: ReplayRootContractState::Unknown,
        development_only: true,
        revealed_prefix: None,
    }
}

fn reservation(call_id: &str, marker: &str) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: "scope-1".into(),
        call_id: call_id.into(),
        dispatch_group_id: "group-1".into(),
        stage: BudgetStage::Reflection,
        actual_input_digest: hash(format!("input-{call_id}").as_bytes()),
        request_artifact: Some(
            BudgetArtifact::from_serializable(
                "rsia.model_request.artifact.v1",
                &json!({
                    "request_id":call_id,
                    "source_closure":[{"id":"run-1","digest":hash(b"source")}],
                    "input":[{"content":marker}],
                }),
            )
            .unwrap(),
        ),
        max_cost_micros: 20,
        lease_token: format!("lease-{call_id}"),
        lease_until: 100,
        now: 1,
    }
}

async fn authorize_budget(store: &Store) {
    store
        .authorize_root_budget(
            &ctx("admin", Role::Admin),
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "scope-1".into(),
                allowed_namespaces: vec!["n".into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "payer".into(),
                authorization_receipt_digest: hash(b"authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn revoke_blocks_immediately_resumes_after_restart_and_preserves_accounting() {
    let (dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let host = ctx("broker", Role::Host);
    let worker = ctx("worker", Role::Worker);
    let marker = "UNIQUE-SOURCE-SECRET";
    put_source(&store, "run-1", marker).await;
    let mut graph = store.session().await.unwrap();
    graph
        .put(
            &worker,
            "artifact",
            "artifact-a",
            worker.actor(),
            &json!({"id":"artifact-a","schema_version":"rsia.optimization.stage_fact.v1","payload":marker,"kind":"terminal_rejected","state":"failed"}),
        )
        .await
        .unwrap();
    graph
        .put(
            &worker,
            "artifact",
            "artifact-b",
            worker.actor(),
            &json!({"id":"artifact-b","schema_version":"rsia.optimization.stage_fact.v1","payload":marker,"kind":"response_observed"}),
        )
        .await
        .unwrap();
    graph
        .put(
            &worker,
            "evaluation",
            "evaluation-1",
            worker.actor(),
            &json!({"id":"evaluation-1","score":7,"state":"failed"}),
        )
        .await
        .unwrap();
    graph
        .put_edge(&worker, "artifact", "artifact-a", "run", "run-1")
        .await
        .unwrap();
    graph
        .put_edge(&worker, "artifact", "artifact-b", "artifact", "artifact-a")
        .await
        .unwrap();
    graph
        .put_edge(&worker, "artifact", "artifact-a", "artifact", "artifact-b")
        .await
        .unwrap();
    graph
        .put_edge(
            &worker,
            "evaluation",
            "evaluation-1",
            "artifact",
            "artifact-a",
        )
        .await
        .unwrap();
    graph.commit().await.unwrap();

    authorize_budget(&store).await;
    let call = store
        .reserve_budget_call(&host, &reservation("call-1", marker))
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now: 2,
    };
    let dispatched = store
        .begin_budget_dispatch(&host, &fence)
        .await
        .unwrap()
        .call;
    let response = BudgetArtifact::from_serializable(
        "test.response.v1",
        &json!({"status":"completed","output":marker}),
    )
    .unwrap();
    store
        .settle_model_budget_call(
            &host,
            &fence,
            &UsageCharge {
                amount_micros: 5,
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                provider_request_id: "provider-1".into(),
                usage_record_id: "usage-1".into(),
                output_digest: hash(marker.as_bytes()),
            },
            &ModelCallSettlementEvidence {
                provenance: BudgetExecutionProvenance::Fixture,
                actual_model_digest: Some(hash(b"model")),
                transport_artifact: BudgetArtifact::from_serializable(
                    "test.transport.v1",
                    &json!({"raw":marker}),
                )
                .unwrap(),
                usable_response: Some(response.clone()),
                blocked_response: BudgetArtifact::from_serializable(
                    "test.response.v1",
                    &json!({"status":"blocked"}),
                )
                .unwrap(),
                forced_block_reason: None,
            },
        )
        .await
        .unwrap();
    store
        .close_budget_call_execution(
            &host,
            "scope-1",
            "call-1",
            dispatched.dispatch_id.as_deref().unwrap(),
            "complete",
            3,
        )
        .await
        .unwrap();

    let status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "run-1".into(),
        },
        "user_delete",
        4,
    )
    .await
    .unwrap();
    let mut immediate = store.session().await.unwrap();
    assert!(
        immediate
            .get::<serde_json::Value>(&admin, "tombstone", "run-1")
            .await
            .unwrap()
            .is_some()
    );
    immediate.commit().await.unwrap();
    LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 1, 5)
        .await
        .unwrap();
    store.close().await;

    let reopened = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let mut current = LifecycleStore::cleanup_status(&admin, &reopened, &status.job_id)
        .await
        .unwrap();
    for now in 6..100 {
        if current.state == CleanupState::Complete {
            break;
        }
        current = LifecycleStore::cleanup_step(&admin, &reopened, &status.job_id, 8, now)
            .await
            .unwrap();
    }
    assert_eq!(current.state, CleanupState::Complete);
    let mut session = reopened.session().await.unwrap();
    assert!(
        session
            .get::<serde_json::Value>(&admin, "run", "run-1")
            .await
            .unwrap()
            .is_none()
    );
    let artifact: serde_json::Value = session
        .need(&admin, "artifact", "artifact-a")
        .await
        .unwrap();
    assert_eq!(artifact["schema_version"], "rsia.redacted.v1");
    assert!(!artifact.to_string().contains(marker));
    let evaluation: serde_json::Value = session
        .need(&admin, "evaluation", "evaluation-1")
        .await
        .unwrap();
    assert_eq!(evaluation["score"], 7);
    session.commit().await.unwrap();
    let call = reopened
        .budget_call(&host, "scope-1", "call-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, BudgetCallState::Finalized);
    assert_eq!(call.actual_cost_micros, Some(5));
    assert_eq!(call.usage_record_id.as_deref(), Some("usage-1"));
    for artifact in [
        call.request_artifact,
        call.transport_artifact,
        call.response_artifact,
    ]
    .into_iter()
    .flatten()
    {
        assert_eq!(artifact.schema_version, "rsia.redacted.v1");
        assert!(!artifact.body.contains(marker));
    }
    let root = reopened
        .root_budget(&admin, "scope-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root.spent_micros, 5);
    assert_eq!(root.reserved_micros, 0);
}

#[tokio::test]
async fn cleanup_recognizes_real_e06_kinds_and_redacts_nested_e05_budget_bodies() {
    let (_dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let marker = "SOURCE2-PRIVATE-MARKER";
    put_source(&store, "source-2", marker).await;
    let request_artifact = BudgetArtifact::from_serializable(
        "rsia.model_request.artifact.v1",
        &json!({"input":[{"content":marker}]}),
    )
    .unwrap();
    let transport_artifact = BudgetArtifact::from_serializable(
        "rsia.model_transport.artifact.v1",
        &json!({"raw_provider_body":marker}),
    )
    .unwrap();
    let response_artifact = BudgetArtifact::from_serializable(
        "rsia.model_response.artifact.v1",
        &json!({"output":marker}),
    )
    .unwrap();
    let call = evo_storage::budget::BudgetCallRecord {
        billing_scope: "scope-e05".into(),
        call_id: "call-e05".into(),
        dispatch_group_id: "ticket-e05".into(),
        namespace: "n".into(),
        stage: BudgetStage::FormalEvaluation,
        actual_input_digest: hash(b"input"),
        request_artifact: Some(request_artifact.clone()),
        reserved_micros: 11,
        state: BudgetCallState::Finalized,
        lease_token: "lease-secret-not-retained".into(),
        lease_epoch: 2,
        lease_until: 50,
        dispatch_id: Some("dispatch-e05".into()),
        provider_request_id: Some("provider-e05".into()),
        usage_record_id: Some("usage-e05".into()),
        output_digest: Some(hash(b"output")),
        actual_cost_micros: Some(7),
        actual_currency: Some("USD".into()),
        actual_pricing_version: Some("pricing-v1".into()),
        execution_provenance: Some(BudgetExecutionProvenance::Fixture),
        actual_model_digest: Some(hash(b"model")),
        transport_artifact: Some(transport_artifact.clone()),
        response_artifact: Some(response_artifact.clone()),
        response_usable: Some(true),
        response_block_reason: None,
        execution_closed: true,
        execution_closed_at: Some(9),
        execution_close_reason: Some("complete".into()),
        terminal_reason: Some("finalized".into()),
        created_at: 1,
        dispatched_at: Some(2),
        finalized_at: Some(8),
    };
    let resource_id = "evaluation-resource-e05";
    let candidate_id = "candidate-e06";
    let snapshot_id = "run-snapshot-e07";
    let application_id = "host-application-e07";
    let execution_id = "host-execution-e07";
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            candidate_id,
            admin.actor(),
            &json!({
                "id":candidate_id,
                "schema_version":"rsia.release_candidate.v1",
                "bundle":{"schema_version":"rsia.resolved_bundle.v2","profile_id":"profile","parent_digest":hash(b"parent"),"baseline_digest":hash(b"baseline"),"skill":{"content":marker,"applicability":"a","counterexample":"c","required_capabilities":[],"dependencies":[]},"improver":{"temperature_micros":0,"max_suggestions":1,"ranking":"stable"},"origins":[],"digest":hash(b"bundle")},
                "bundle_digest":hash(b"bundle"),"environment_digest":hash(b"environment"),
                "profile_id":"profile","parent_digest":hash(b"parent"),"proposer_actor":"worker",
                "sources":[{"kind":"run","id":"source-2","content_digest":hash(marker.as_bytes())}],"revoke_watermark":1
            }),
        )
        .await
        .unwrap();
    session
        .put(
            &admin,
            "artifact",
            snapshot_id,
            admin.actor(),
            &json!({
                "id":snapshot_id,"schema_version":"rsia.run_application.v1","run_id":"e07","profile_id":"profile",
                "pointer_epoch":1,"release_id":"release-e06","bundle_digest":hash(b"bundle"),"environment_digest":hash(b"environment"),
                "system_snapshot":{"schema_version":"rsia.system_snapshot.v2","profile_id":"profile","host_id":"host","host_version":"1","model_id":"model","tools":["tool"],"mandatory_context_digest":hash(b"context")},
                "host_surface_id":"surface","host_surface_digest":hash(b"surface"),"host_capabilities_digest":hash(b"caps"),
                "projection":{"bundle_digest":hash(b"bundle"),"instructions":[marker],"tools":["tool"]},
                "request_material":{"mandatory_context_digest":hash(b"context"),"task_input_digest":hash(b"task"),"instructions":[marker],"tools":["tool"]},
                "request_digest":hash(b"request"),"capability_level":"full","evolution_enabled":true
            }),
        )
        .await
        .unwrap();
    session
        .put(
            &admin,
            "receipt",
            application_id,
            admin.actor(),
            &json!({
                "id":application_id,"schema_version":"rsia.host_application.v1","run_id":"e07","release_id":"release-e06",
                "actual_request_digest":hash(b"request"),"execution_receipt_id":execution_id,
                "receipt":{"offered":["skill"],"attached":["skill"],"used":["skill"],"verified_benefit":[],"bundle_digest":hash(b"bundle"),"request_digest":hash(b"request"),"capability_level":"full","truncated":false,"attested_by":"host"}
            }),
        )
        .await
        .unwrap();
    session
        .put(
            &admin,
            "artifact",
            execution_id,
            admin.actor(),
            &json!({"id":execution_id,"schema_version":"rsia.host_execution_receipt.v1","run_id":"e07","request_digest":hash(b"request"),"environment_digest":hash(b"environment"),"host_surface_digest":hash(b"surface"),"output_digest":hash(marker.as_bytes()),"used_ids":["skill"]}),
        )
        .await
        .unwrap();
    session
        .put(
            &admin,
            "artifact",
            resource_id,
            admin.actor(),
            &json!({
                "schema_version":"rsia.typed_artifact_envelope.v1","id":resource_id,
                "record_kind":"evaluation_resource_evidence_v1",
                "payload":{"schema_version":"rsia.evaluation_resource_evidence.v1","ticket_id":"ticket-e05","billing_scope":"scope-e05","cost_scope":"ticket_execution_only","calls":[call]}
            }),
        )
        .await
        .unwrap();
    for (kind, id) in [
        ("artifact", candidate_id),
        ("artifact", snapshot_id),
        ("receipt", application_id),
        ("artifact", execution_id),
        ("artifact", resource_id),
    ] {
        session
            .put_edge(&admin, kind, id, "run", "source-2")
            .await
            .unwrap();
    }
    session.commit().await.unwrap();

    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "source-2".into(),
        },
        "erase private source body",
        10,
    )
    .await
    .unwrap();
    for now in 11..100 {
        if status.state == CleanupState::Complete {
            break;
        }
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 32, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete);
    let mut session = store.session().await.unwrap();
    for (kind, id) in [
        ("artifact", candidate_id),
        ("artifact", snapshot_id),
        ("receipt", application_id),
        ("artifact", execution_id),
    ] {
        let value: serde_json::Value = session.need(&admin, kind, id).await.unwrap();
        assert_eq!(value["schema_version"], "rsia.redacted.v1");
        assert!(!value.to_string().contains(marker));
    }
    let resource: serde_json::Value = session.need(&admin, "artifact", resource_id).await.unwrap();
    let rendered = resource.to_string();
    assert_eq!(resource["schema_version"], "rsia.redacted.v1");
    assert_eq!(resource["metadata"]["calls"][0]["state"], "finalized");
    assert_eq!(resource["metadata"]["calls"][0]["actual_cost_micros"], 7);
    assert_eq!(
        resource["metadata"]["calls"][0]["usage_record_id"],
        "usage-e05"
    );
    assert_eq!(
        resource["metadata"]["calls"][0]["request_artifact"]["digest"],
        request_artifact.digest
    );
    assert!(resource["metadata"]["formal_closure_invalidated"] == true);
    assert!(!rendered.contains(marker));
    assert!(!rendered.contains("lease-secret-not-retained"));
    session.commit().await.unwrap();
}

#[tokio::test]
async fn cleanup_redacts_replay_pool_and_report_but_keeps_irreversible_usage() {
    let (_dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let train = lifecycle_replay_world("cleanup-train", "cleanup-c1", WorldPartition::Train);
    let select = lifecycle_replay_world("cleanup-select", "cleanup-c2", WorldPartition::Select);
    let train_observation_body =
        evo_core::replay::replay_observation_bytes(&train.manifest, &train.transitions[0]).unwrap();
    let select_observation_body =
        evo_core::replay::replay_observation_bytes(&select.manifest, &select.transitions[0])
            .unwrap();
    let second_body = b"REPLAY-SOURCE-SECRET".to_vec();
    let train_baseline_body =
        evo_core::replay::replay_baseline_observation_bytes(&train.manifest).unwrap();
    let select_baseline_body =
        evo_core::replay::replay_baseline_observation_bytes(&select.manifest).unwrap();
    let mut session = store.session().await.unwrap();
    for (id, body) in [
        (
            train.transitions[0].observation_source_id.clone(),
            train_observation_body,
        ),
        (
            select.transitions[0].observation_source_id.clone(),
            select_observation_body,
        ),
        ("replay-source-2".to_string(), second_body),
        (
            train.manifest.baseline_observation_source_id.clone(),
            train_baseline_body,
        ),
        (
            select.manifest.baseline_observation_source_id.clone(),
            select_baseline_body,
        ),
    ] {
        session
            .put(
                &admin,
                "run",
                &id,
                admin.actor(),
                &json!({
                    "schema_version":"rsia.optimization.source.v1",
                    "record":{"id":id,"body":body,"parent_family":format!("family-{id}"),"task_origin":"trusted_run","execution_attestation":"trusted_host","purpose":"development"},
                    "trace":{"run_id":id,"parent_family":format!("family-{id}"),"source_digest":hash(&body),"purpose":"development","outcome":"success","diagnosis":null,"excerpt":String::from_utf8(body.clone()).unwrap(),"seed":1},
                    "excerpt_start":0,"excerpt_end":body.len()
                }),
            )
            .await
            .unwrap();
    }
    session
        .bump_watermark(&admin, &hash(b"replay-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    seal_replay_world(&admin, &store, &train).await.unwrap();
    seal_replay_world(&admin, &store, &select).await.unwrap();
    let pool: ReplayPoolManifestV1 = register_replay_pool(
        &admin,
        &store,
        &["cleanup-train".into(), "cleanup-select".into()],
    )
    .await
    .unwrap();
    let policy = ElasticPolicyV1::default();
    let profile = ReplaySimulationProfile::default_v2(1, pool.pool_digest.clone());
    let rejected_legacy = StoredReplayReportV1::build(
        "n",
        WorldPartition::Train,
        &pool,
        policy.clone(),
        profile.clone(),
        ExplorationCapsV1::online(),
        vec![lifecycle_replay_report(
            &train,
            &policy,
            &profile,
            "REPLAY-SOURCE-SECRET",
        )],
    )
    .unwrap();
    assert!(matches!(
        put_replay_report(&admin, &store, &rejected_legacy).await,
        Err(evo_core::Error::Invalid(reason)) if reason == "legacy_replay_new_write_forbidden"
    ));
    let report = StoredReplayReportV2::build(
        "n",
        WorldPartition::Train,
        &pool,
        policy.clone(),
        profile.clone(),
        ExplorationCapsV1::online(),
        vec![lifecycle_current_replay_report(
            &train,
            &policy,
            &profile,
            "REPLAY-SOURCE-SECRET",
        )],
    )
    .unwrap();
    put_current_replay_report(&admin, &store, &report)
        .await
        .unwrap();
    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "replay-source-2".into(),
        },
        "erase replay source",
        20,
    )
    .await
    .unwrap();
    for now in 21..100 {
        if status.state == CleanupState::Complete {
            break;
        }
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 32, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete);
    let mut session = store.session().await.unwrap();
    let pool_value: serde_json::Value = session
        .need(
            &admin,
            "artifact",
            &format!("replay-pool-{}", pool.pool_digest),
        )
        .await
        .unwrap();
    let report_value: serde_json::Value = session
        .need(&admin, "artifact", &report.report_id)
        .await
        .unwrap();
    assert_eq!(pool_value["schema_version"], "rsia.redacted.v1");
    assert_eq!(report_value["schema_version"], "rsia.redacted.v1");
    assert_eq!(
        report_value["metadata"]["irreversible_usage"][0]["historical_usage"]["cost_micros"],
        7
    );
    assert_eq!(
        report_value["metadata"]["irreversible_usage"][0]["probes"],
        1
    );
    assert!(!report_value.to_string().contains("REPLAY-SOURCE-SECRET"));
    session.commit().await.unwrap();
}

#[tokio::test]
async fn more_than_ten_thousand_edges_are_keyset_paginated_without_truncation() {
    let (_dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let worker = ctx("worker", Role::Worker);
    put_source(&store, "run-many", "source").await;
    let mut session = store.session().await.unwrap();
    for index in 0..10_001 {
        session
            .put_edge(
                &worker,
                "artifact",
                &format!("node-{index:05}"),
                "run",
                "run-many",
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "run-many".into(),
        },
        "large_graph",
        1,
    )
    .await
    .unwrap();
    for now in 2..250 {
        if status.state == CleanupState::Complete {
            break;
        }
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 256, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete);
    assert!(status.processed_nodes >= 10_003);
}

#[tokio::test]
async fn unknown_content_scope_fails_job_instead_of_claiming_complete() {
    let (_dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let worker = ctx("worker", Role::Worker);
    put_source(&store, "run-root", "root").await;
    put_source(&store, "run-derived", "derived").await;
    let mut session = store.session().await.unwrap();
    session
        .put_edge(&worker, "run", "run-derived", "run", "run-root")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "run-root".into(),
        },
        "unknown_scope",
        1,
    )
    .await
    .unwrap();
    for now in 2..20 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 8, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Failed);
    assert!(status.last_error.unwrap().contains("blocked_unknown_scope"));
}

async fn make_backup() -> (tempfile::TempDir, Store, std::path::PathBuf, String) {
    let (dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    put_source(&store, "run-2", "post-backup-source").await;
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", "a", admin.actor(), &json!({"id":"a"}))
        .await
        .unwrap();
    session
        .bump_watermark(&admin, &hash(b"initial-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let blob = store.put_blob(&admin, b"blob-data").await.unwrap();
    let backup = dir.path().join("backup");
    store.backup(&backup).await.unwrap();
    (dir, store, backup, blob)
}

fn exact_delta(backup: &std::path::Path) -> std::path::PathBuf {
    let manifest_bytes = std::fs::read(backup.join("backup-manifest.json")).unwrap();
    let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes).unwrap();
    let namespaces = manifest
        .watermarks
        .iter()
        .map(|watermark| {
            json!({
                "namespace":watermark.namespace,
                "base_seq":watermark.seq,
                "base_digest":watermark.digest,
                "events":[],
                "latest_seq":watermark.seq,
                "latest_digest":watermark.digest,
            })
        })
        .collect::<Vec<_>>();
    let delta = backup.parent().unwrap().join("delta.json");
    std::fs::write(
        &delta,
        serde_json::to_vec(&json!({
            "schema_version":"rsia.revoke_delta.v1",
            "base_manifest_sha256":hash(&manifest_bytes),
            "namespaces":namespaces,
        }))
        .unwrap(),
    )
    .unwrap();
    delta
}

fn restore(backup: &std::path::Path, dest: &std::path::Path, delta: &std::path::Path) -> i32 {
    restore_anchor(
        backup,
        dest,
        delta,
        Some(&backup.parent().unwrap().join("rsia.sqlite3")),
    )
}

#[tokio::test]
async fn restore_replays_strict_e16_import_source_revocation_from_current_anchor() {
    let (dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, &hash(b"e16-base-watermark"))
        .await
        .unwrap();
    let source = e16_import_envelope(
        "rsia.e16.import_source.v1",
        "restore-import-source",
        json!([]),
        json!({"raw_blob_digest":null,"state":"missing"}),
    );
    session
        .put(
            &admin,
            "artifact",
            "restore-import-source",
            admin.actor(),
            &source,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let backup = dir.path().join("e16-backup");
    store.backup(&backup).await.unwrap();
    let manifest_bytes = std::fs::read(backup.join("backup-manifest.json")).unwrap();
    let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes).unwrap();
    let base = manifest.watermarks.first().unwrap();

    let revoked = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: "restore-import-source".into(),
        },
        "post backup import revoke",
        9,
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    let tombstone: evo_storage::lifecycle::RevokeTombstone = session
        .need(&admin, "tombstone", "restore-import-source")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let delta = dir.path().join("e16-delta.json");
    std::fs::write(
        &delta,
        serde_json::to_vec(&json!({
            "schema_version":"rsia.revoke_delta.v1",
            "base_manifest_sha256":hash(&manifest_bytes),
            "namespaces":[{
                "namespace":base.namespace,
                "base_seq":base.seq,
                "base_digest":base.digest,
                "events":[{
                    "seq":base.seq + 1,
                    "previous_digest":base.digest,
                    "digest":revoked.watermark_digest,
                    "source_kind":"artifact",
                    "source_id":"restore-import-source",
                    "tombstone_digest":hash(&serde_json::to_vec(&tombstone).unwrap()),
                    "reason":"post backup import revoke",
                    "created_at":9,
                }],
                "latest_seq":base.seq + 1,
                "latest_digest":revoked.watermark_digest,
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    let restored = dir.path().join("e16-restored");
    assert_eq!(restore(&backup, &restored, &delta), 0);
    let restored_store = Store::open(&restored.join("rsia.sqlite3")).await.unwrap();
    let mut restored_session = restored_store.session().await.unwrap();
    let restored_tombstone: evo_storage::lifecycle::RevokeTombstone = restored_session
        .need(&admin, "tombstone", "restore-import-source")
        .await
        .unwrap();
    assert_eq!(restored_tombstone.source_kind, "artifact");
    restored_session.commit().await.unwrap();
    let status = LifecycleStore::cleanup_status(&admin, &restored_store, &revoked.job_id)
        .await
        .unwrap();
    assert_eq!(status.state, CleanupState::Pending);
}

#[tokio::test]
async fn backup_manifest_and_restore_reject_incomplete_old_or_overwrite_cases() {
    let (dir, store, backup, blob) = make_backup().await;
    let manifest: BackupManifest =
        serde_json::from_slice(&std::fs::read(backup.join("backup-manifest.json")).unwrap())
            .unwrap();
    assert!(manifest.complete);
    assert_eq!(manifest.schema_version, "rsia.backup.v2");
    assert!(manifest.blobs.iter().any(|entry| entry.sha256 == blob));
    let delta = exact_delta(&backup);
    let restored = dir.path().join("restored");
    assert_eq!(restore(&backup, &restored, &delta), 0);
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(restored.join("restore.json")).unwrap()).unwrap();
    assert_eq!(receipt["receipt_scope"], "local_only_not_for_export");
    assert!(
        receipt["trusted_anchor"]["scope"]
            .as_str()
            .unwrap()
            .contains("stale copy")
    );
    assert!(receipt["trusted_anchor"]["verified_at"].is_string());
    assert!(receipt["trusted_anchor"]["watermarks"]["n"]["seq"].is_number());
    let opened = Store::open(&restored.join("rsia.sqlite3")).await.unwrap();
    assert_eq!(opened.integrity().await.unwrap(), "ok");

    let manifest_bytes = std::fs::read(backup.join("backup-manifest.json")).unwrap();
    let watermark = manifest.watermarks.first().unwrap();
    let revoked = LifecycleStore::begin_revoke(
        &ctx("admin", Role::Admin),
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "run-2".into(),
        },
        "post_backup_delete",
        9,
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    let tombstone: evo_storage::lifecycle::RevokeTombstone = session
        .need(&ctx("admin", Role::Admin), "tombstone", "run-2")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let tombstone_digest = hash(&serde_json::to_vec(&tombstone).unwrap());
    let proof = revoked.watermark_digest;
    let newer_delta = dir.path().join("newer-delta.json");
    std::fs::write(
        &newer_delta,
        serde_json::to_vec(&json!({
            "schema_version":"rsia.revoke_delta.v1",
            "base_manifest_sha256":hash(&manifest_bytes),
            "namespaces":[{
                "namespace":watermark.namespace,
                "base_seq":watermark.seq,
                "base_digest":watermark.digest,
                "events":[{
                    "seq":watermark.seq + 1,
                    "previous_digest":watermark.digest,
                    "digest":proof,
                    "source_kind":"run",
                    "source_id":"run-2",
                    "tombstone_digest":tombstone_digest,
                    "reason":"post_backup_delete",
                    "created_at":9,
                }],
                "latest_seq":watermark.seq + 1,
                "latest_digest":proof,
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    let stale_dest = dir.path().join("stale-delta");
    assert_eq!(restore(&backup, &stale_dest, &delta), 1);
    assert!(!stale_dest.exists());
    let mut invented: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&newer_delta).unwrap()).unwrap();
    invented["namespaces"][0]["events"][0]["source_id"] = json!("invented-source");
    let invented_path = dir.path().join("invented-delta.json");
    std::fs::write(&invented_path, serde_json::to_vec(&invented).unwrap()).unwrap();
    assert_eq!(
        restore(&backup, &dir.path().join("invented-dest"), &invented_path),
        1
    );
    let replayed = dir.path().join("replayed");
    assert_eq!(restore(&backup, &replayed, &newer_delta), 0);
    let replayed_store = Store::open(&replayed.join("rsia.sqlite3")).await.unwrap();
    let mut replayed_session = replayed_store.session().await.unwrap();
    assert!(
        replayed_session
            .get::<serde_json::Value>(&ctx("admin", Role::Admin), "tombstone", "run-2")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        replayed_session
            .watermark(&ctx("admin", Role::Admin))
            .await
            .unwrap()
            .unwrap()
            .0,
        watermark.seq + 1
    );
    replayed_session.commit().await.unwrap();
    let admin = ctx("admin", Role::Admin);
    let restored_job = LifecycleStore::cleanup_status(&admin, &replayed_store, &revoked.job_id)
        .await
        .unwrap();
    assert_eq!(restored_job.state, CleanupState::Pending);
    let finished = LifecycleStore::cleanup_step(&admin, &replayed_store, &revoked.job_id, 16, 10)
        .await
        .unwrap();
    assert_eq!(finished.state, CleanupState::Complete);
    let mut session = replayed_store.session().await.unwrap();
    assert!(
        session
            .get::<serde_json::Value>(&admin, "run", "run-2")
            .await
            .unwrap()
            .is_none()
    );
    session.commit().await.unwrap();

    let forged_delta = dir.path().join("forged-delta.json");
    std::fs::write(
        &forged_delta,
        serde_json::to_vec(&json!({
            "schema_version":"rsia.revoke_delta.v1",
            "base_manifest_sha256":hash(&manifest_bytes),
            "namespaces":[{
                "namespace":watermark.namespace,
                "base_seq":watermark.seq,
                "base_digest":watermark.digest,
                "events":[],
                "latest_seq":watermark.seq + 1,
                "latest_digest":hash(b"forged"),
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        restore(&backup, &dir.path().join("forged-restore"), &forged_delta),
        1
    );

    let existing = dir.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    assert_eq!(restore(&backup, &existing, &delta), 2);
    assert!(store.backup(&existing).await.is_err());

    let broken = dir.path().join("broken");
    copy_dir(&backup, &broken);
    let blob_path = manifest.blobs.first().unwrap().path.clone();
    std::fs::remove_file(broken.join(blob_path)).unwrap();
    assert_eq!(
        restore(&broken, &dir.path().join("broken-restore"), &delta),
        1
    );

    let old = dir.path().join("old-backup");
    std::fs::create_dir(&old).unwrap();
    std::fs::copy(backup.join("rsia.sqlite3"), old.join("rsia.sqlite3")).unwrap();
    assert_eq!(restore(&old, &dir.path().join("old-restore"), &delta), 1);

    let unknown = dir.path().join("unknown-schema");
    copy_dir(&backup, &unknown);
    let unknown_manifest_path = unknown.join("backup-manifest.json");
    let mut unknown_manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&unknown_manifest_path).unwrap()).unwrap();
    unknown_manifest["migrations"]
        .as_array_mut()
        .unwrap()
        .push(json!({"version":999,"description":"future","checksum_hex":"00","success":true}));
    std::fs::write(
        &unknown_manifest_path,
        serde_json::to_vec_pretty(&unknown_manifest).unwrap(),
    )
    .unwrap();
    let unknown_delta = exact_delta(&unknown);
    assert_eq!(
        restore(
            &unknown,
            &dir.path().join("unknown-restore"),
            &unknown_delta
        ),
        1
    );

    let blocked_parent = dir.path().join("not-a-directory");
    std::fs::write(&blocked_parent, b"x").unwrap();
    let failed_dest = blocked_parent.join("backup");
    assert!(store.backup(&failed_dest).await.is_err());
    assert!(!failed_dest.join("backup-manifest.json").exists());
}

fn copy_dir(source: &std::path::Path, destination: &std::path::Path) {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.recursive(true).create(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn cleanup_state_json_is_strict() {
    let unknown = json!({
        "job_id":"j",
        "source":{"kind":"run","id":"r"},
        "watermark_seq":1,
        "watermark_digest":hash(b"w"),
        "state":"pending",
        "processed_nodes":0,
        "pending_nodes":1,
        "last_error":null,
        "unknown":true,
    });
    assert!(serde_json::from_value::<evo_storage::lifecycle::CleanupStatus>(unknown).is_err());
}

#[tokio::test]
async fn restore_rejects_unknown_nonfinite_and_boolean_integer_manifest_fields() {
    let (dir, _store, backup, _) = make_backup().await;
    let delta = exact_delta(&backup);
    for mutation in ["extra", "nan", "delta-bool-seq"] {
        let package = dir.path().join(format!("strict-{mutation}"));
        copy_dir(&backup, &package);
        let manifest_path = package.join("backup-manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        let bytes = match mutation {
            "extra" => {
                value["unexpected"] = json!(true);
                serde_json::to_vec(&value).unwrap()
            }
            "nan" => {
                value["created_at"] = serde_json::Value::Null;
                serde_json::to_string(&value)
                    .unwrap()
                    .replace("\"created_at\":null", "\"created_at\":NaN")
                    .into_bytes()
            }
            "delta-bool-seq" => serde_json::to_vec(&value).unwrap(),
            _ => unreachable!(),
        };
        if mutation != "delta-bool-seq" {
            std::fs::write(&manifest_path, bytes).unwrap();
        }
        let selected_delta = if mutation == "delta-bool-seq" {
            let path = dir.path().join("delta-bool-seq.json");
            let mut value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&delta).unwrap()).unwrap();
            value["namespaces"][0]["base_seq"] = json!(true);
            std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            path
        } else {
            delta.clone()
        };
        let dest = dir.path().join(format!("strict-dest-{mutation}"));
        assert_eq!(restore(&package, &dest, &selected_delta), 1, "{mutation}");
        assert!(!dest.exists());
    }
}

fn restore_anchor(
    backup: &std::path::Path,
    dest: &std::path::Path,
    delta: &std::path::Path,
    anchor: Option<&std::path::Path>,
) -> i32 {
    let backup = std::fs::canonicalize(backup).unwrap();
    let dest = std::fs::canonicalize(dest.parent().unwrap())
        .unwrap()
        .join(dest.file_name().unwrap());
    let delta = std::fs::canonicalize(delta).unwrap();
    let anchor = anchor.map(|p| std::fs::canonicalize(p).unwrap());
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/restore_backup.py");
    let mut command = Command::new("python3");
    command
        .arg(script)
        .arg("--backup")
        .arg(backup)
        .arg("--dest")
        .arg(dest)
        .arg("--revoke-delta")
        .arg(delta);
    if let Some(anchor) = anchor {
        command.arg("--trusted-revocations-db").arg(anchor);
    }
    command.status().unwrap().code().unwrap()
}

#[tokio::test]
async fn restore_rejects_unlisted_links_duplicate_paths_and_checksum_forgery() {
    let (dir, _store, backup, _blob) = make_backup().await;
    let manifest: BackupManifest =
        serde_json::from_slice(&std::fs::read(backup.join("backup-manifest.json")).unwrap())
            .unwrap();
    let blob = &manifest.blobs[0].path;
    for attack in [
        "extra",
        "hardlink",
        "leaf_symlink",
        "parent_symlink",
        "duplicate",
        "checksum",
    ] {
        let malicious = dir.path().join(format!("malicious-{attack}"));
        copy_dir(&backup, &malicious);
        let manifest_path = malicious.join("backup-manifest.json");
        match attack {
            "extra" => {
                std::fs::write(malicious.join("blobs/unlisted-secret"), b"unlisted").unwrap()
            }
            "hardlink" => {
                std::fs::hard_link(malicious.join(blob), dir.path().join("hardlink-copy")).unwrap()
            }
            "leaf_symlink" => {
                #[cfg(unix)]
                {
                    let target = dir.path().join("leaf-target");
                    std::fs::rename(malicious.join(blob), &target).unwrap();
                    std::os::unix::fs::symlink(target, malicious.join(blob)).unwrap();
                }
            }
            "parent_symlink" => {
                #[cfg(unix)]
                {
                    let source = malicious.join(blob).parent().unwrap().to_path_buf();
                    let target = dir.path().join("parent-target");
                    std::fs::rename(&source, &target).unwrap();
                    std::os::unix::fs::symlink(target, source).unwrap();
                }
            }
            "duplicate" => {
                let mut m: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
                let mut duplicate = m["blobs"][0].clone();
                duplicate["path"] = json!(format!("./{blob}"));
                m["blobs"].as_array_mut().unwrap().push(duplicate);
                std::fs::write(&manifest_path, serde_json::to_vec(&m).unwrap()).unwrap();
            }
            "checksum" => {
                let pool = sqlx::SqlitePool::connect(&format!(
                    "sqlite:{}",
                    malicious.join("rsia.sqlite3").display()
                ))
                .await
                .unwrap();
                sqlx::query("UPDATE _sqlx_migrations SET checksum=zeroblob(48) WHERE version=1")
                    .execute(&pool)
                    .await
                    .unwrap();
                pool.close().await;
                let mut m: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
                m["migrations"][0]["checksum_hex"] = json!("00".repeat(48));
                m["database"]["sha256"] = json!(hash(
                    &std::fs::read(malicious.join("rsia.sqlite3")).unwrap()
                ));
                std::fs::write(&manifest_path, serde_json::to_vec(&m).unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        let delta = exact_delta(&malicious);
        let dest = dir.path().join(format!("rejected-{attack}"));
        assert_eq!(restore(&malicious, &dest, &delta), 1, "attack {attack}");
        assert!(!dest.exists());
    }
}

#[tokio::test]
async fn restore_requires_independent_current_anchor_and_never_rewinds_query_consumption() {
    let (dir, _store, backup, _) = make_backup().await;
    let delta = exact_delta(&backup);
    let dest = dir.path().join("no-anchor");
    assert_eq!(restore_anchor(&backup, &dest, &delta, None), 1);
    assert!(!dest.exists());
    assert_eq!(
        restore_anchor(
            &backup,
            &dir.path().join("self-anchor"),
            &delta,
            Some(&backup.join("rsia.sqlite3"))
        ),
        1
    );
    // A current control plane has consumed another query after the backup.
    let query_pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("rsia.sqlite3").display()
    ))
    .await
    .unwrap();
    sqlx::query("INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','artifact','consumed-query','admin',?)")
        .bind(json!({"schema_version":"rsia.typed_artifact_envelope.v1","id":"consumed-query","record_kind":"exposure_ledger_v1","payload":spent_exposure_ledger()}).to_string())
        .execute(&query_pool).await.unwrap();
    query_pool.close().await;
    let dest = dir.path().join("query-rewind");
    assert_eq!(restore(&backup, &dest, &delta), 1);
    assert!(!dest.exists());
    // Anchor older than backup and unknown anchor migration schema are independently isolated.
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("rsia.sqlite3").display()
    ))
    .await
    .unwrap();
    sqlx::query("DELETE FROM objects WHERE id='consumed-query'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE revoke_watermark SET seq=0")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(restore(&backup, &dir.path().join("old-anchor"), &delta), 1);
    sqlx::query("UPDATE revoke_watermark SET seq=1")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE _sqlx_migrations SET checksum=zeroblob(48) WHERE version=1")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        restore(&backup, &dir.path().join("unknown-anchor"), &delta),
        1
    );
    pool.close().await;
}

#[tokio::test]
async fn cleanup_preserves_typed_spent_facts_redacts_world_and_blocks_unknown_artifacts() {
    let (_dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    put_source(&store, "source", "SECRET").await;
    let mut session = store.session().await.unwrap();
    for kind in [
        "exposure_ledger_v1",
        "alpha_claim_reservation_v1",
        "evaluation_ticket_v2",
    ] {
        let payload = if kind == "exposure_ledger_v1" {
            serde_json::to_value(spent_exposure_ledger()).unwrap()
        } else {
            json!({"spent":1,"state":"early_stopped"})
        };
        session.put(&admin,"artifact",kind,"admin",&json!({"schema_version":"rsia.typed_artifact_envelope.v1","id":kind,"record_kind":kind,"payload":payload})).await.unwrap();
        session
            .put_edge(&admin, "artifact", kind, "run", "source")
            .await
            .unwrap();
    }
    session
        .put(
            &admin,
            "artifact",
            "unknown",
            "admin",
            &json!({"schema_version":"future.unknown.v1","secret":"MUST-REMAIN-BLOCKED"}),
        )
        .await
        .unwrap();
    session
        .put_edge(&admin, "artifact", "unknown", "run", "source")
        .await
        .unwrap();
    session.put_world(&admin,"world",true,&json!({"schema_version":"rsia.replay_world.v2","manifest":{"world_id":"world","source_closure":[{"source_id":"source"}],"action_catalog":["SECRET"],"revoke_watermark":1},"sealed_digest":hash(b"world"),"transitions":[{"record_id":"t","record_seq":1,"next_context_signature":"SECRET","actual_usage":{"cost_micros":7,"input_tokens":2,"output_tokens":0},"outcome":{"outcome":"observed","status":{"status":"hard_failure"}}}]})).await.unwrap();
    session
        .put_edge(&admin, "replay_world", "world", "run", "source")
        .await
        .unwrap();
    let (_, mut draft) = session.get_world(&admin, "world").await.unwrap().unwrap();
    draft["manifest"]["world_id"] = json!("draft");
    draft["sealed_digest"] = serde_json::Value::Null;
    session
        .put_world(&admin, "draft", false, &draft)
        .await
        .unwrap();
    // Deliberately no edge: E08 discovers the already introduced draft schema by a bounded scan.
    session.commit().await.unwrap();
    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "source".into(),
        },
        "delete",
        1,
    )
    .await
    .unwrap();
    for now in 2..20 {
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 16, now)
            .await
            .unwrap();
        if status.pending_nodes == 0 {
            break;
        }
    }
    assert_eq!(status.state, CleanupState::Failed);
    let mut session = store.session().await.unwrap();
    for kind in [
        "exposure_ledger_v1",
        "alpha_claim_reservation_v1",
        "evaluation_ticket_v2",
    ] {
        let v: serde_json::Value = session.need(&admin, "artifact", kind).await.unwrap();
        if kind == "exposure_ledger_v1" {
            let mut ledger: evo_core::holdout::ExposureLedger =
                serde_json::from_value(v["payload"].clone()).unwrap();
            ledger.validate_restore().unwrap();
            assert!(ledger.is_cluster_consumed("cluster-spent"));
            assert!(ledger.release_undispatched_money("ticket-spent").is_err());
            assert_eq!(ledger.entries.len(), 1);
        } else {
            assert_eq!(v["payload"]["spent"], 1);
        }
        assert_eq!(v["record_kind"], kind);
    }
    let unknown: serde_json::Value = session.need(&admin, "artifact", "unknown").await.unwrap();
    assert_eq!(unknown["schema_version"], "future.unknown.v1");
    let (sealed, world) = session.get_world(&admin, "world").await.unwrap().unwrap();
    assert!(sealed);
    assert_eq!(world["schema_version"], "rsia.redacted.v1");
    assert!(!world.to_string().contains("SECRET"));
    assert_eq!(
        world["historical_transitions"][0]["actual_usage"]["cost_micros"],
        7
    );
    let (sealed, draft) = session.get_world(&admin, "draft").await.unwrap().unwrap();
    assert!(!sealed);
    assert_eq!(draft["schema_version"], "rsia.redacted.v1");
    assert!(!draft.to_string().contains("SECRET"));
    session.commit().await.unwrap();
}

#[tokio::test]
async fn e16_import_source_revocation_cleans_full_result_closure_and_only_its_blob() {
    let (_dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let first_blob = store.put_blob(&admin, b"SOURCE-ONE-SECRET").await.unwrap();
    let second_blob = store.put_blob(&admin, b"SOURCE-TWO-SECRET").await.unwrap();
    let source_one = e16_import_envelope(
        "rsia.e16.import_source.v1",
        "import-source-one",
        json!([]),
        json!({"raw_blob_digest":first_blob}),
    );
    let source_two = e16_import_envelope(
        "rsia.e16.import_source.v1",
        "import-source-two",
        json!([]),
        json!({"raw_blob_digest":second_blob}),
    );
    let result = e16_import_envelope(
        "rsia.e16.import_result.v1",
        "import-result",
        json!([
            {"kind":"artifact","id":"import-source-one","content_digest":hash(b"one")},
            {"kind":"artifact","id":"import-source-two","content_digest":hash(b"two")}
        ]),
        json!({"aggregate_summary":{"total_events":2},"secret":"RESULT-SECRET"}),
    );
    let mut session = store.session().await.unwrap();
    for (id, value) in [
        ("import-source-one", source_one),
        ("import-source-two", source_two),
        ("import-result", result),
    ] {
        session
            .put(&admin, "artifact", id, admin.actor(), &value)
            .await
            .unwrap();
    }
    session
        .put_edge(&admin, "artifact", "import-source-one", "blob", &first_blob)
        .await
        .unwrap();
    session
        .put_edge(
            &admin,
            "artifact",
            "import-source-two",
            "blob",
            &second_blob,
        )
        .await
        .unwrap();
    for source in ["import-source-one", "import-source-two"] {
        session
            .put_edge(&admin, "artifact", "import-result", "artifact", source)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();

    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: "import-source-two".into(),
        },
        "source two revoked",
        2,
    )
    .await
    .unwrap();
    for now in 3..30 {
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 16, now)
            .await
            .unwrap();
        if status.pending_nodes == 0 {
            break;
        }
    }
    assert_eq!(status.state, CleanupState::Complete);
    assert_eq!(
        store.read_blob(&admin, &first_blob, 1024).await.unwrap(),
        b"SOURCE-ONE-SECRET"
    );
    let second_read = store.read_blob(&admin, &second_blob, 1024).await;
    assert!(
        matches!(second_read, Err(evo_core::Error::NotFound)),
        "unexpected revoked blob read: {second_read:?}"
    );
    let mut session = store.session().await.unwrap();
    let first: serde_json::Value = session
        .need(&admin, "artifact", "import-source-one")
        .await
        .unwrap();
    assert_eq!(first["schema_version"], "rsia.e16.import_source.v1");
    let second: serde_json::Value = session
        .need(&admin, "artifact", "import-source-two")
        .await
        .unwrap();
    let result: serde_json::Value = session
        .need(&admin, "artifact", "import-result")
        .await
        .unwrap();
    assert_eq!(second["schema_version"], "rsia.redacted.v1");
    assert_eq!(result["schema_version"], "rsia.redacted.v1");
    assert!(!result.to_string().contains("RESULT-SECRET"));
    session.commit().await.unwrap();
    let reconnected = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: "import-source-two".into(),
        },
        "source two revoked",
        2,
    )
    .await
    .unwrap();
    assert_eq!(reconnected.job_id, status.job_id);
}

#[tokio::test]
async fn e16_shared_package_blob_is_deleted_only_after_every_source_closure_is_revoked() {
    let (_dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    put_source(&store, "source-a", "A").await;
    put_source(&store, "source-b", "B").await;
    let blob = store
        .put_blob(&admin, b"SHARED-PACKAGE-SECRET")
        .await
        .unwrap();
    let make_staged = |id: &str, source: &str| {
        json!({
            "schema_version":"rsia.e16.staged_asset.v1",
            "id":id,
            "namespace":"n",
            "owner_actor":"admin",
            "request_key":format!("request-{id}"),
            "input_digest":hash(format!("input-{id}").as_bytes()),
            "created_at":1,
            "updated_at":1,
            "source_refs":[{"kind":"run","id":source,"digest":hash(source.as_bytes())}],
            "revoke_watermark":0,
            "payload":{
                "publisher":"publisher",
                "asset_id":id,
                "package_kind":"skill",
                "manifest_digest":hash(b"manifest"),
                "content_blob_digest":blob,
                "content_bytes":21,
                "baseline_digest":hash(b"baseline"),
                "local_digest":hash(b"local"),
                "environment_digest":hash(b"environment"),
                "package_schema_version":"v1",
                "compiler_version":"v1",
                "state":"staged",
                "secret":"PACKAGE-SECRET"
            }
        })
    };
    let mut session = store.session().await.unwrap();
    for (id, source) in [("staged-a", "source-a"), ("staged-b", "source-b")] {
        session
            .put(
                &admin,
                "artifact",
                id,
                admin.actor(),
                &make_staged(id, source),
            )
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", id, "run", source)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();

    for (source, now, should_exist) in [("source-a", 2, true), ("source-b", 20, false)] {
        let mut status = LifecycleStore::begin_revoke(
            &admin,
            &store,
            TypedObjectRef {
                kind: "run".into(),
                id: source.into(),
            },
            "delete source",
            now,
        )
        .await
        .unwrap();
        for step in now + 1..now + 15 {
            status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 16, step)
                .await
                .unwrap();
            if status.pending_nodes == 0 {
                break;
            }
        }
        assert_eq!(status.state, CleanupState::Complete);
        let read = store.read_blob(&admin, &blob, 1024).await;
        assert_eq!(read.is_ok(), should_exist);
    }
}

async fn prepared_staged_asset(
    store: &Store,
    admin: &Context,
    source_id: &str,
    artifact_id: &str,
    bytes: &[u8],
) -> String {
    put_source(store, source_id, "authority").await;
    let source = json!({
        "id":source_id,
        "schema_version":"rsia.optimization.source.v1",
        "body":"authority"
    });
    let digest = hash(bytes);
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(admin, &hash(format!("watermark-{artifact_id}").as_bytes()))
        .await
        .unwrap();
    let envelope = json!({
        "schema_version":"rsia.e16.staged_asset.v1",
        "id":artifact_id,
        "namespace":"n",
        "owner_actor":"admin",
        "request_key":format!("request-{artifact_id}"),
        "input_digest":hash(format!("input-{artifact_id}").as_bytes()),
        "created_at":1,
        "updated_at":1,
        "source_refs":[{"kind":"run","id":source_id,"digest":fingerprint(&source).unwrap()}],
        "revoke_watermark":1,
        "payload":{
            "publisher":"publisher",
            "asset_id":artifact_id,
            "package_kind":"skill",
            "manifest_digest":hash(b"manifest"),
            "content_blob_digest":digest,
            "content_bytes":bytes.len(),
            "baseline_digest":hash(b"baseline"),
            "local_digest":hash(b"local"),
            "environment_digest":hash(b"environment"),
            "package_schema_version":"v1",
            "compiler_version":"v1",
            "state":"prepared"
        }
    });
    session
        .put(admin, "artifact", artifact_id, admin.actor(), &envelope)
        .await
        .unwrap();
    session
        .put_edge(admin, "artifact", artifact_id, "run", source_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    digest
}

#[tokio::test]
async fn cleanup_before_registered_publish_rejects_late_blob_without_orphan() {
    let (_dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let bytes = b"late package secret";
    let digest =
        prepared_staged_asset(&store, &admin, "late-source", "late-prepared-asset", bytes).await;
    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "late-source".into(),
        },
        "revoke before publish",
        2,
    )
    .await
    .unwrap();
    for now in 3..20 {
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 16, now)
            .await
            .unwrap();
        if status.pending_nodes == 0 {
            break;
        }
    }
    assert_eq!(status.state, CleanupState::Complete);
    assert!(
        store
            .publish_registered_blob(
                &admin,
                "late-prepared-asset",
                "rsia.e16.staged_asset.v1",
                "content_blob_digest",
                bytes,
                64 * 1024 * 1024,
            )
            .await
            .is_err()
    );
    assert!(matches!(
        store.read_blob(&admin, &digest, 1024).await,
        Err(evo_core::Error::NotFound)
    ));
}

#[tokio::test]
async fn registered_publish_before_cleanup_leaves_prepared_reference_for_safe_deletion() {
    let (_dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let bytes = b"published package secret";
    let digest = prepared_staged_asset(
        &store,
        &admin,
        "published-source",
        "published-prepared-asset",
        bytes,
    )
    .await;
    store
        .publish_registered_blob(
            &admin,
            "published-prepared-asset",
            "rsia.e16.staged_asset.v1",
            "content_blob_digest",
            bytes,
            64 * 1024 * 1024,
        )
        .await
        .unwrap();
    assert_eq!(store.read_blob(&admin, &digest, 1024).await.unwrap(), bytes);
    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "published-source".into(),
        },
        "revoke after publish",
        2,
    )
    .await
    .unwrap();
    for now in 3..20 {
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 16, now)
            .await
            .unwrap();
        if status.pending_nodes == 0 {
            break;
        }
    }
    assert_eq!(status.state, CleanupState::Complete);
    assert!(matches!(
        store.read_blob(&admin, &digest, 1024).await,
        Err(evo_core::Error::NotFound)
    ));
}

fn spent_exposure_ledger() -> evo_core::holdout::ExposureLedger {
    use evo_core::holdout::{
        ExposureEntry, ExposureLedger, ExposureState, MonetaryReservationState,
    };
    let mut ledger = ExposureLedger::new("family", hash(b"alpha-plan"), 1).unwrap();
    ledger.entries.push(ExposureEntry {
        ticket_id: "ticket-spent".into(),
        manifest_digest: hash(b"manifest"),
        candidate_pair_digest: hash(b"pair"),
        epoch_id: "epoch".into(),
        slice_id: "slice".into(),
        task_cluster_ids: vec!["cluster-spent".into()],
        state: ExposureState::FeedbackUsed { used_at_unix_ms: 2 },
        monetary_state: MonetaryReservationState::DispatchedCostPending,
    });
    ledger.validate_restore().unwrap();
    ledger
}

#[test]
fn restore_publication_is_atomically_no_overwrite_even_for_empty_destination() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let dest = dir.path().join("dest");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(source.join("restore.json"), b"receipt").unwrap();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/restore_backup.py");
    let code = "import importlib.util,sys; s=importlib.util.spec_from_file_location('restore',sys.argv[1]); m=importlib.util.module_from_spec(s);s.loader.exec_module(m)\ntry: m.publish_exclusive(sys.argv[2],sys.argv[3])\nexcept OSError: sys.exit(0)\nsys.exit(1)";
    assert!(
        Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg("-c")
            .arg(code)
            .arg(script)
            .arg(&source)
            .arg(&dest)
            .status()
            .unwrap()
            .success()
    );
    assert!(source.join("restore.json").exists());
    assert!(!dest.join("restore.json").exists());
}

#[tokio::test]
async fn empty_blob_directories_do_not_create_unlisted_backup_members() {
    let (dir, store, _backup, blob) = make_backup().await;
    store
        .delete_blob(&ctx("admin", Role::Admin), &blob)
        .await
        .unwrap();
    let backup = dir.path().join("empty-blobs-backup");
    store.backup(&backup).await.unwrap();
    let manifest: BackupManifest =
        serde_json::from_slice(&std::fs::read(backup.join("backup-manifest.json")).unwrap())
            .unwrap();
    assert!(manifest.blobs.is_empty());
    assert!(!backup.join("blobs").exists());
    let delta = exact_delta(&backup);
    assert_eq!(
        restore(&backup, &dir.path().join("empty-blobs-restored"), &delta),
        0
    );
}

#[cfg(unix)]
#[test]
fn restore_rejects_symlink_above_the_package_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let real = root.join("real");
    std::fs::create_dir_all(real.join("package")).unwrap();
    std::fs::write(real.join("package/member"), b"verified").unwrap();
    let alias = root.join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/restore_backup.py");
    let code = "import importlib.util,sys,pathlib; s=importlib.util.spec_from_file_location('restore',sys.argv[1]); m=importlib.util.module_from_spec(s);s.loader.exec_module(m)\ntry: m.safe_member(pathlib.Path(sys.argv[2]),'member')\nexcept m.IsolationError: sys.exit(0)\nsys.exit(1)";
    assert!(
        Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg("-c")
            .arg(code)
            .arg(script)
            .arg(alias.join("package"))
            .status()
            .unwrap()
            .success()
    );
}

// =========================================================================
// F17 Adversarial Regression (from controller review): a staged asset whose
// payload state is not live (aborted, failed, quarantined) must not keep a raw
// import blob alive once the import source is revoked, while live states
// (prepared, staged) still do.
// =========================================================================

/// Live import source `import_id` (edge -> blob) plus a staged asset
/// `staged_id` in `state` whose `content_blob_digest` is the same raw blob and
/// whose only source is run `source_id`.
async fn f17_import_and_staged_asset_sharing_blob(
    store: &Store,
    admin: &Context,
    import_id: &str,
    staged_id: &str,
    source_id: &str,
    state: &str,
) -> String {
    let bytes = format!("SHARED-RAW-{import_id}");
    let digest = store.put_blob(admin, bytes.as_bytes()).await.unwrap();
    put_source(store, source_id, "authority").await;
    let source = json!({
        "id":source_id,
        "schema_version":"rsia.optimization.source.v1",
        "body":"authority"
    });
    let import = e16_import_envelope(
        "rsia.e16.import_source.v1",
        import_id,
        json!([{"kind":"blob","id":digest,"content_digest":digest}]),
        json!({"status":"ready","raw_blob_digest":digest,"blob_published":true}),
    );
    let staged = e16_import_envelope(
        "rsia.e16.staged_asset.v1",
        staged_id,
        json!([{"kind":"run","id":source_id,"digest":fingerprint(&source).unwrap()}]),
        json!({
            "publisher":"publisher",
            "asset_id":"asset",
            "package_kind":"skill",
            "manifest_digest":hash(b"manifest"),
            "content_blob_digest":digest,
            "content_bytes":bytes.len(),
            "baseline_digest":hash(b"baseline"),
            "local_digest":hash(b"local"),
            "environment_digest":hash(b"environment"),
            "package_schema_version":"v1",
            "compiler_version":"v1",
            "state":state
        }),
    );
    let mut session = store.session().await.unwrap();
    session
        .put(admin, "artifact", import_id, admin.actor(), &import)
        .await
        .unwrap();
    session
        .put_edge(admin, "artifact", import_id, "blob", &digest)
        .await
        .unwrap();
    session
        .put(admin, "artifact", staged_id, admin.actor(), &staged)
        .await
        .unwrap();
    session
        .put_edge(admin, "artifact", staged_id, "run", source_id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    digest
}

async fn f17_revoke_import_and_check_blob(state: &str, expect_blob_kept: bool) {
    let (_dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let digest = f17_import_and_staged_asset_sharing_blob(
        &store,
        &admin,
        "f17-import",
        "f17-staged",
        "f17-run",
        state,
    )
    .await;
    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: "f17-import".into(),
        },
        "f17 import revoke",
        2,
    )
    .await
    .unwrap();
    for now in 3..40 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 16, now)
            .await
            .unwrap();
    }
    assert_eq!(
        status.state,
        CleanupState::Complete,
        "{:?}",
        status.last_error
    );
    let read = store.read_blob(&admin, &digest, 1024).await;
    assert_eq!(
        read.is_ok(),
        expect_blob_kept,
        "staged asset state {state:?}: blob read after import revoke = {read:?}"
    );
    let mut session = store.session().await.unwrap();
    let import: serde_json::Value = session
        .need(&admin, "artifact", "f17-import")
        .await
        .unwrap();
    let staged: serde_json::Value = session
        .need(&admin, "artifact", "f17-staged")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(import["schema_version"], "rsia.redacted.v1");
    assert_eq!(staged["schema_version"], "rsia.e16.staged_asset.v1");
    assert_eq!(staged["payload"]["state"], state);
}

#[tokio::test]
async fn f17_aborted_staged_asset_does_not_keep_shared_import_blob_alive() {
    f17_revoke_import_and_check_blob("aborted", false).await;
}

#[tokio::test]
async fn f17_failed_staged_asset_does_not_keep_shared_import_blob_alive() {
    f17_revoke_import_and_check_blob("failed", false).await;
}

#[tokio::test]
async fn f17_quarantined_staged_asset_does_not_keep_shared_import_blob_alive() {
    f17_revoke_import_and_check_blob("quarantined", false).await;
}

#[tokio::test]
async fn f17_staged_and_prepared_staged_assets_keep_shared_import_blob_alive() {
    f17_revoke_import_and_check_blob("staged", true).await;
    f17_revoke_import_and_check_blob("prepared", true).await;
}

#[tokio::test]
async fn seed_install_v2_registered_blobs_recover_and_share_liveness() {
    let (dir, store) = database().await;
    let admin = ctx("admin", Role::Admin);
    let bytes = b"AG158-SHARED-SEED";
    let digest = hash(bytes);
    // Software run fixtures from the existing lifecycle helper, not execution
    // or formal authority. Both new v2 producers bind their actual stored run.
    for source in ["seed-source-a", "seed-source-b"] {
        put_source(&store, source, "AG158-v2-source-software-fixture").await;
    }
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, "seed-v2-watermark")
        .await
        .unwrap();
    for (id, source) in [
        ("seed-v2-a", "seed-source-a"),
        ("seed-v2-b", "seed-source-b"),
    ] {
        let body: serde_json::Value = session.need(&admin, "run", source).await.unwrap();
        let envelope = json!({
            "schema_version":"rsia.e16.seed_install.v2", "id":id, "namespace":"n", "owner_actor":"admin", "request_key":id,
            "input_digest":hash(id.as_bytes()), "created_at":1, "updated_at":1,
            "source_refs":[{"kind":"run","id":source,"digest":fingerprint(&body).unwrap()}], "revoke_watermark":1,
            "payload":{
                "publisher":"org.rsia", "asset_id":id, "kind":"skill", "baseline_blob_digest":digest, "baseline_digest":digest,
                "local_blob_digest":digest,"local_digest":digest,"upstream_blob_digest":digest,"upstream_digest":digest,
                "environment_digest":hash(b"environment"),"status":"prepared","quarantine_reason":null,
                "local_identity":{"publisher":"org.rsia","asset_id":id,"kind":"skill"},"local_marked":true,
                "upstream_path":"upstream/seed.txt","compiler_version":"compiler-v1","catalog_version":null,"confirmed_content_digest":null
            }
        });
        session
            .put(&admin, "artifact", id, admin.actor(), &envelope)
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", id, "run", source)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    store.close().await;
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    for id in ["seed-v2-a", "seed-v2-b"] {
        for field in [
            "baseline_blob_digest",
            "local_blob_digest",
            "upstream_blob_digest",
        ] {
            assert_eq!(
                store
                    .publish_registered_blob(
                        &admin,
                        id,
                        "rsia.e16.seed_install.v2",
                        field,
                        bytes,
                        1024
                    )
                    .await
                    .unwrap(),
                digest
            );
        }
    }
    assert_eq!(store.read_blob(&admin, &digest, 1024).await.unwrap(), bytes);
    for (source, tick, survives) in [("seed-source-a", 2, true), ("seed-source-b", 30, false)] {
        let mut cleanup = LifecycleStore::begin_revoke(
            &admin,
            &store,
            TypedObjectRef {
                kind: "run".into(),
                id: source.into(),
            },
            "revoke seed source",
            tick,
        )
        .await
        .unwrap();
        for now in tick + 1..tick + 20 {
            cleanup = LifecycleStore::cleanup_step(&admin, &store, &cleanup.job_id, 16, now)
                .await
                .unwrap();
            if cleanup.pending_nodes == 0 {
                break;
            }
        }
        assert_eq!(cleanup.state, CleanupState::Complete);
        let read = store.read_blob(&admin, &digest, 1024).await;
        if survives {
            assert_eq!(read.unwrap(), bytes);
        } else {
            assert!(matches!(read, Err(evo_core::Error::NotFound)));
        }
    }
}

#[tokio::test]
async fn seed_install_v1_v2_cleanup_redacts_same_registered_content() {
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../evo-engine/tests/fixtures/ag158_seed_install_v1.json"
    ))
    .unwrap();
    let fixture = &fixtures["revocable_observation"];
    let original = fixture["stored_envelope_body"].as_str().unwrap();
    assert_eq!(
        hash(original.as_bytes()),
        fixture["stored_envelope_body_sha256"]
    );
    assert_eq!(fixture["source_is_execution_or_formal_proof"], false);
    let (dir, store) = database().await;
    let admin = Context::new(fixture["namespace"].as_str().unwrap(), "admin", Role::Admin).unwrap();
    let host = Context::new(
        admin.namespace(),
        fixture["source_owner_actor"].as_str().unwrap(),
        Role::Host,
    )
    .unwrap();
    let observed = &fixture["envelope"];
    let legacy_id = observed["id"].as_str().unwrap();
    let legacy_source = fixture["source_refs"][0]["id"].as_str().unwrap();
    assert_eq!(fixture["source_refs"][0]["kind"], "run");
    assert_eq!(fixture["dependency_edges"], json!([["run", legacy_source]]));
    let source: serde_json::Value =
        serde_json::from_str(fixture["source_body"].as_str().unwrap()).unwrap();
    assert_eq!(
        hash(fixture["source_body"].as_str().unwrap().as_bytes()),
        fixture["source_body_sha256"]
    );
    // The v2 protocol fixture has its own live software run in the observed
    // record's real namespace. Equal digests here are the same physical blobs.
    let live_source = "ag158-current-live-run";
    let live_body = json!({"id":live_source,"schema_version":"rsia.optimization.source.v1","body":"AG158-v2-software-source"});
    let mut session = store.session().await.unwrap();
    session
        .put(&host, "run", legacy_source, host.actor(), &source)
        .await
        .unwrap();
    session
        .put(&host, "run", live_source, host.actor(), &live_body)
        .await
        .unwrap();
    session
        .bump_watermark(&admin, "observed-legacy-watermark")
        .await
        .unwrap();
    session
        .put(
            &admin,
            "budget",
            "retained-cost",
            admin.actor(),
            &json!({"id":"retained-cost","settled_micros":7}),
        )
        .await
        .unwrap();
    assert_eq!(
        session
            .object_row(&admin, "run", legacy_source)
            .await
            .unwrap()
            .unwrap()
            .body,
        fixture["source_body"].as_str().unwrap()
    );
    session.commit().await.unwrap();
    // Restore the untouched old TEXT in its original namespace, without a
    // typed JSON projection or a different source/identity masquerading as it.
    let connection = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        dir.path().join("rsia.sqlite3").display()
    ))
    .await
    .unwrap();
    sqlx::query("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,'artifact',?,?,?)")
        .bind(admin.namespace())
        .bind(legacy_id)
        .bind(admin.actor())
        .bind(original)
        .execute(&connection)
        .await
        .unwrap();
    connection.close().await;
    // New, explicitly synthetic storage protocol material; it is not a
    // historical capture, a Current-verified comparison, or execution evidence.
    let current = json!({
        "schema_version":"rsia.e16.seed_install.v2", "id":"seed-v2-shared", "namespace":admin.namespace(),
        "owner_actor":admin.actor(), "request_key":"seed-v2-shared", "input_digest":hash(b"AG158-v2-protocol-fixture"),
        "created_at":1,"updated_at":1,"source_refs":[{"kind":"run","id":live_source,"digest":fingerprint(&live_body).unwrap()}],
        "revoke_watermark":1,"payload":{
            "publisher":"org.rsia","asset_id":"reasoning-seed","kind":"skill",
            "baseline_blob_digest":observed["payload"]["baseline_blob_digest"],"baseline_digest":observed["payload"]["baseline_digest"],
            "local_blob_digest":observed["payload"]["local_blob_digest"],"local_digest":observed["payload"]["local_digest"],
            "upstream_blob_digest":observed["payload"]["upstream_blob_digest"],"upstream_digest":observed["payload"]["upstream_digest"],
            "environment_digest":hash(b"environment"),"status":"prepared","quarantine_reason":null,
            "local_identity":{"publisher":"org.rsia","asset_id":"reasoning-seed","kind":"skill"},"local_marked":true,
            "upstream_path":"upstream/seed.txt","compiler_version":"compiler-v1","catalog_version":null,"confirmed_content_digest":null
        }
    });
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            "seed-v2-shared",
            admin.actor(),
            &current,
        )
        .await
        .unwrap();
    session
        .put_edge(&admin, "artifact", legacy_id, "run", legacy_source)
        .await
        .unwrap();
    session
        .put_edge(&admin, "artifact", "seed-v2-shared", "run", live_source)
        .await
        .unwrap();
    assert_eq!(
        session
            .object_row(&admin, "artifact", legacy_id)
            .await
            .unwrap()
            .unwrap()
            .body,
        original
    );
    assert_eq!(
        session
            .dependents(&admin, "run", legacy_source)
            .await
            .unwrap(),
        vec![("artifact".to_string(), legacy_id.to_string())]
    );
    session.commit().await.unwrap();
    for (input, field) in [
        ("baseline_bytes", "baseline_blob_digest"),
        ("local_bytes", "local_blob_digest"),
        ("upstream_bytes", "upstream_blob_digest"),
    ] {
        let bytes: Vec<u8> = serde_json::from_value(fixture["request"][input].clone()).unwrap();
        assert_eq!(
            store
                .publish_registered_blob(
                    &admin,
                    "seed-v2-shared",
                    "rsia.e16.seed_install.v2",
                    field,
                    &bytes,
                    1024
                )
                .await
                .unwrap(),
            observed["payload"][field].as_str().unwrap()
        );
    }
    for (source, tick, survives) in [(legacy_source, 2, true), (live_source, 30, false)] {
        let mut cleanup = LifecycleStore::begin_revoke(
            &admin,
            &store,
            TypedObjectRef {
                kind: "run".into(),
                id: source.into(),
            },
            "revoke legacy or current seed source",
            tick,
        )
        .await
        .unwrap();
        for now in tick + 1..tick + 20 {
            cleanup = LifecycleStore::cleanup_step(&admin, &store, &cleanup.job_id, 16, now)
                .await
                .unwrap();
            if cleanup.pending_nodes == 0 {
                break;
            }
        }
        assert_eq!(cleanup.state, CleanupState::Complete);
        let mut session = store.session().await.unwrap();
        let redacted: serde_json::Value =
            session.need(&admin, "artifact", legacy_id).await.unwrap();
        assert_eq!(redacted["schema_version"], "rsia.redacted.v1");
        assert_eq!(
            redacted["original_digest"],
            fixture["stored_envelope_body_sha256"]
        );
        assert_eq!(redacted["original_kind"], "seed_install");
        assert!(redacted.get("payload").is_none());
        // The frozen parent's normative redaction retains digest metadata.
        assert_eq!(
            redacted["metadata"]["payload"]["baseline_blob_digest"],
            observed["payload"]["baseline_blob_digest"]
        );
        let other: serde_json::Value = session
            .need(&admin, "artifact", "seed-v2-shared")
            .await
            .unwrap();
        if survives {
            assert_eq!(other["schema_version"], "rsia.e16.seed_install.v2");
            assert_eq!(other["payload"]["status"], "prepared");
        } else {
            assert_eq!(other["schema_version"], "rsia.redacted.v1");
            assert_eq!(other["original_schema"], "rsia.e16.seed_install.v2");
            assert_eq!(other["original_kind"], "seed_install");
            assert!(other.get("payload").is_none());
            assert_eq!(
                other["metadata"]["payload"]["baseline_blob_digest"],
                observed["payload"]["baseline_blob_digest"]
            );
        }
        let cost: serde_json::Value = session
            .need(&admin, "budget", "retained-cost")
            .await
            .unwrap();
        assert_eq!(cost["settled_micros"], 7);
        let tombstone: serde_json::Value = session.need(&admin, "tombstone", source).await.unwrap();
        assert_eq!(tombstone["source_kind"], "run");
        session.commit().await.unwrap();
        for (input, field) in [
            ("baseline_bytes", "baseline_blob_digest"),
            ("local_bytes", "local_blob_digest"),
            ("upstream_bytes", "upstream_blob_digest"),
        ] {
            let read = store
                .read_blob(&admin, observed["payload"][field].as_str().unwrap(), 1024)
                .await;
            if survives {
                let bytes: Vec<u8> =
                    serde_json::from_value(fixture["request"][input].clone()).unwrap();
                assert_eq!(read.unwrap(), bytes);
            } else {
                assert!(matches!(read, Err(evo_core::Error::NotFound)));
            }
        }
    }
    // Separate, explicitly synthetic protocol fixtures mirror the existing F17
    // import/shared-blob test: a strict import root and an independent software
    // run-backed nonlive v2 record reference the same bytes in one namespace.
    for status in ["staged", "quarantined", "unknown"] {
        let (_dir, store) = database().await;
        let admin = ctx("admin", Role::Admin);
        put_source(&store, "nonlive-run", "AG158-nonlive-software-source").await;
        let bytes = format!("nonlive-v2-{status}");
        let digest = store.put_blob(&admin, bytes.as_bytes()).await.unwrap();
        let root = format!("import-{status}");
        let survivor = format!("nonlive-{status}");
        let import = e16_import_envelope(
            "rsia.e16.import_source.v1",
            &root,
            json!([{"kind":"blob","id":digest,"content_digest":digest}]),
            json!({"status":"ready","raw_blob_digest":digest,"blob_published":true}),
        );
        let mut session = store.session().await.unwrap();
        let source: serde_json::Value = session.need(&admin, "run", "nonlive-run").await.unwrap();
        let nonlive = e16_import_envelope(
            "rsia.e16.seed_install.v2",
            &survivor,
            json!([{"kind":"run","id":"nonlive-run","digest":fingerprint(&source).unwrap()}]),
            json!({
                "publisher":"org.rsia","asset_id":survivor,"kind":"skill",
                "baseline_blob_digest":digest,"baseline_digest":digest,"local_blob_digest":digest,"local_digest":digest,
                "upstream_blob_digest":digest,"upstream_digest":digest,"environment_digest":hash(b"environment"),
                "status":status,"quarantine_reason":null,
                "local_identity":{"publisher":"org.rsia","asset_id":survivor,"kind":"skill"},"local_marked":true,
                "upstream_path":"upstream/seed.txt","compiler_version":"compiler-v1","catalog_version":null,"confirmed_content_digest":null
            }),
        );
        session
            .put(&admin, "artifact", &root, admin.actor(), &import)
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", &root, "blob", &digest)
            .await
            .unwrap();
        session
            .put(&admin, "artifact", &survivor, admin.actor(), &nonlive)
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", &survivor, "run", "nonlive-run")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let mut cleanup = LifecycleStore::begin_revoke(
            &admin,
            &store,
            TypedObjectRef {
                kind: "artifact".into(),
                id: root.clone(),
            },
            "nonlive seed does not retain content",
            40,
        )
        .await
        .unwrap();
        for tick in 41..60 {
            cleanup = LifecycleStore::cleanup_step(&admin, &store, &cleanup.job_id, 16, tick)
                .await
                .unwrap();
            if cleanup.pending_nodes == 0 {
                break;
            }
        }
        assert_eq!(cleanup.state, CleanupState::Complete);
        assert!(matches!(
            store.read_blob(&admin, &digest, 1024).await,
            Err(evo_core::Error::NotFound)
        ));
        let mut session = store.session().await.unwrap();
        let import: serde_json::Value = session.need(&admin, "artifact", &root).await.unwrap();
        let remaining: serde_json::Value =
            session.need(&admin, "artifact", &survivor).await.unwrap();
        assert_eq!(import["schema_version"], "rsia.redacted.v1");
        assert_eq!(remaining["schema_version"], "rsia.e16.seed_install.v2");
        assert_eq!(remaining["payload"]["status"], status);
        session.commit().await.unwrap();
    }
}
