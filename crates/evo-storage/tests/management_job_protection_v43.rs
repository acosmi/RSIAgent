//! Management wires retain cleanup, recovery accounting and backup protection.
use evo_core::{Context, Role, hash};
use evo_storage::lifecycle::{
    CleanupState, LifecycleStore, TypedObjectRef, read_control_plane_facts,
};
use evo_storage::{ManagementJob, ManagementJobState, Store, VersionedManagementJob};
use serde_json::{Value, json};

fn job(key: &str, state: ManagementJobState, at: i64) -> ManagementJob {
    let digest = hash(format!("n\0admin\0meta.start\0{key}").as_bytes());
    ManagementJob {
        id: format!("management-job-{}", &digest[..24]),
        schema_version: "rsia.management_job.v1".into(),
        operation: "meta.start".into(),
        request_key: key.into(),
        payload_digest: hash(key.as_bytes()),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state,
        step: "claimed".into(),
        private_input_ref: format!("management-input-{}", &digest[..24]),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: Some(format!("lease-{key}")),
        lease_until: at,
        generation: 1,
        diagnostics: vec![],
        created_at: 1,
    }
}

#[tokio::test]
async fn legacy_and_current_jobs_keep_restore_and_cleanup_protection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    let old = job("legacy-protected", ManagementJobState::Running, 100);
    let current =
        VersionedManagementJob::V1(job("current-protected", ManagementJobState::Draining, 100))
            .into_unfinished_v2()
            .unwrap();
    let mut s = store.session().await.unwrap();
    s.bump_watermark(&admin, &hash(b"initial")).await.unwrap();
    s.put(&host,"run","source",host.actor(),&json!({"id":"source","schema_version":"rsia.optimization.source.v1","body":"owned source"})).await.unwrap();
    s.put(&admin, "job", &old.id, admin.actor(), &old)
        .await
        .unwrap();
    s.put(&admin, "job", &current.id, admin.actor(), &current)
        .await
        .unwrap();
    for (id, input) in [
        (&old.id, &old.private_input_ref),
        (&current.id, &current.private_input_ref),
    ] {
        s.put(&admin,"artifact",input,admin.actor(),&json!({"id":input,"schema_version":"rsia.management_private_input.v1","operation":"meta.start","payload_digest":hash(b"source"),"payload":{}})).await.unwrap();
        s.put_edge(&admin, "job", id, "artifact", input)
            .await
            .unwrap();
        s.put_edge(&admin, "artifact", input, "run", "source")
            .await
            .unwrap();
    }
    s.commit().await.unwrap();
    let before = read_control_plane_facts(&path).await.unwrap();
    let started = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "source".into(),
        },
        "test source revocation",
        2,
    )
    .await
    .unwrap();
    let mut done = started;
    for _ in 0..100 {
        done = LifecycleStore::cleanup_step(&admin, &store, &done.job_id, 2, 3)
            .await
            .unwrap();
        if done.state == CleanupState::Complete {
            break;
        }
    }
    assert_eq!(done.state, CleanupState::Complete);
    let after = read_control_plane_facts(&path).await.unwrap();
    for id in [&old.id, &current.id] {
        let key = ("n".into(), "job".into(), id.to_owned());
        assert_eq!(
            after.protected_objects.get(&key),
            before.protected_objects.get(&key)
        );
    }
    let mut s = store.session().await.unwrap();
    for input in [&old.private_input_ref, &current.private_input_ref] {
        let v: Value = s.need(&admin, "artifact", input).await.unwrap();
        assert_eq!(v["schema_version"], "rsia.redacted.v1");
    }
    s.commit().await.unwrap();
    let backup = dir.path().join("backup");
    store.backup(&backup).await.unwrap();
    store.close().await;
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(backup.join("backup-manifest.json")).unwrap())
            .unwrap();
    let raw = std::fs::read(backup.join("backup-manifest.json")).unwrap();
    let entries=manifest["watermarks"].as_array().unwrap().iter().map(|m|json!({"namespace":m["namespace"],"base_seq":m["seq"],"base_digest":m["digest"],"events":[],"latest_seq":m["seq"],"latest_digest":m["digest"]})).collect::<Vec<_>>();
    let delta = dir.path().join("delta.json");
    std::fs::write(&delta,serde_json::to_vec(&json!({"schema_version":"rsia.revoke_delta.v1","base_manifest_sha256":hash(&raw),"namespaces":entries})).unwrap()).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let dest = dir.path().join("restored");
    let output = std::process::Command::new("python3")
        .arg("-B")
        .arg(root.join("scripts/restore_backup.py"))
        .arg("--backup")
        .arg(&backup)
        .arg("--dest")
        .arg(&dest)
        .arg("--revoke-delta")
        .arg(&delta)
        .arg("--trusted-revocations-db")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        read_control_plane_facts(&dest.join("rsia.sqlite3"))
            .await
            .unwrap(),
        after
    );
}

#[tokio::test]
async fn running_and_draining_capacity_preserves_existing_limits() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("capacity.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let mut s = store.session().await.unwrap();
    for i in 0..10 {
        let value = job(
            &format!("active-{i}"),
            if i % 2 == 0 {
                ManagementJobState::Running
            } else {
                ManagementJobState::Draining
            },
            100,
        );
        if i % 2 == 0 {
            s.put(&admin, "job", &value.id, admin.actor(), &value)
                .await
                .unwrap();
        } else {
            let current = VersionedManagementJob::V1(value)
                .into_unfinished_v2()
                .unwrap();
            s.put(&admin, "job", &current.id, admin.actor(), &current)
                .await
                .unwrap();
        }
    }
    let usage = s.capacity_usage_v41(99).await.unwrap();
    assert_eq!(usage.active_leases, 10);
    assert_eq!(usage.concurrent_dispatches, 10);
    let expired = s.capacity_usage_v41(100).await.unwrap();
    assert_eq!(expired.active_leases, 0);
    assert_eq!(
        expired.concurrent_dispatches, 10,
        "expiry does not close either wire's execution"
    );
    for (i, evidence) in ["none", "generation", "token", "result"]
        .into_iter()
        .enumerate()
    {
        let mut value = job(&format!("queued-{evidence}"), ManagementJobState::Queued, 0);
        value.generation = u64::from(evidence == "generation");
        value.lease_token = (evidence == "token").then(|| "queued-original-token".into());
        value.lease_until = if evidence == "token" { 100 } else { 0 };
        value.result = (evidence == "result").then(|| {
            evo_storage::ManagementResult::EvaluationTicketBlocked {
                ticket_id: "queued-existing-ticket".into(),
            }
        });
        if i % 2 == 0 {
            s.put(&admin, "job", &value.id, admin.actor(), &value)
                .await
                .unwrap();
        } else {
            let current = VersionedManagementJob::V1(value)
                .into_unfinished_v2()
                .unwrap();
            s.put(&admin, "job", &current.id, admin.actor(), &current)
                .await
                .unwrap();
        }
    }
    let before_recovery = s.capacity_usage_v41(99).await.unwrap();
    assert_eq!(before_recovery.concurrent_dispatches, 13);
    assert_eq!(before_recovery.active_leases, 11);
    let expired = s.capacity_usage_v41(100).await.unwrap();
    assert_eq!(expired.concurrent_dispatches, 13);
    assert_eq!(expired.active_leases, 0);
    s.put(
        &admin,
        "job",
        "management-job-unknown",
        admin.actor(),
        &json!({"id":"management-job-unknown","schema_version":"rsia.management_job.v99"}),
    )
    .await
    .unwrap();
    assert!(
        s.capacity_usage_v41(100).await.is_err(),
        "unreadable history cannot count as zero"
    );
}
