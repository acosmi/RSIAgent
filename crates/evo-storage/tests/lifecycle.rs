use evo_core::{Context, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{BackupManifest, CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::json;
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
    std::fs::create_dir(destination).unwrap();
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
