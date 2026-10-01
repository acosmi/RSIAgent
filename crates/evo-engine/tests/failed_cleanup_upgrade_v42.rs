//! The optional generator uses APIs available at 200c43bc and creates Failed
//! through the real lifecycle API. No binary database fixtures are committed.
//! Default tests are self-contained; only an explicit OLD_BACKUP override is
//! evidence of upgrading an actual old-program artifact.

use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{
    OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
};
use evo_core::{Context, Role, hash};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, load_stored_source, store_trace_authority,
};
use evo_storage::Store;
use evo_storage::lifecycle::{
    BackupManifest, CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_NS: &str = "upgrade-fixture";
const GENERATE: &str = "RSIA_FAILED_CLEANUP_GENERATE_BACKUP";
const OLD_BACKUP: &str = "RSIA_FAILED_CLEANUP_OLD_BACKUP";

fn ctx(namespace: &str, role: Role) -> Context {
    Context::new(namespace, format!("{role:?}"), role).unwrap()
}

fn authority(id: &str) -> StoredTraceAuthority {
    let prefix = "前缀🧠;";
    let excerpt = format!("EXCERPT-{id}-汉字");
    let body = format!("{prefix}{excerpt};BODY-{id}-private");
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.as_bytes().to_vec(),
            parent_family: format!("family-{id}"),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: format!("family-{id}"),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::TaskFailure,
            diagnosis: Some(SkillFailureDiagnosis {
                kind: SkillFailureKind::Uncertain,
                skill_id: "fixture-skill".into(),
                bundle_digest: hash(b"bundle"),
                request_digest: hash(b"request"),
                rule_id: None,
                support: vec![],
                counterexamples: vec![],
                reason: format!("DIAGNOSIS-{id}-private"),
            }),
            excerpt: excerpt.clone(),
            seed: 17,
        },
        excerpt_start: prefix.len(),
        excerpt_end: prefix.len() + excerpt.len(),
    }
}

fn snapshot(path: &Path, namespace: &str) -> Value {
    let code = r#"import sqlite3,json,sys
c=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True);c.row_factory=sqlite3.Row
q={
'objects':'SELECT kind,id,body FROM objects WHERE namespace=? ORDER BY kind,id',
'cache':'SELECT actor,operation,request_key,subject_id,payload_hash,response,redacted FROM idempotency WHERE namespace=? ORDER BY actor,operation,request_key',
'edges':'SELECT src_kind,src_id,dst_kind,dst_id FROM dependencies WHERE namespace=? ORDER BY src_kind,src_id,dst_kind,dst_id',
'events':'SELECT seq,job_id,node_kind,node_id,event_kind,details FROM revoke_cleanup_events WHERE namespace=? ORDER BY seq',
'frontier':'SELECT seq,job_id,node_kind,node_id,expanded,cursor_src_kind,cursor_src_id FROM revoke_cleanup_frontier WHERE namespace=? ORDER BY seq',
'jobs':'SELECT job_id,source_kind,source_id,watermark_seq,watermark_digest,state,processed_nodes,last_error,created_at FROM revoke_cleanup_jobs WHERE namespace=? ORDER BY job_id',
'audit':'SELECT payload,previous_hash,digest FROM audit WHERE namespace=? ORDER BY seq',
'watermark':'SELECT seq,digest FROM revoke_watermark WHERE namespace=?',
}
print(json.dumps({k:[dict(r) for r in c.execute(v,(sys.argv[2],))] for k,v in q.items()}))
"#;
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(code)
        .arg(path)
        .arg(namespace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        assert!(!entry.file_type().unwrap().is_symlink());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
        }
    }
}

async fn new_authorities(store: &Store, namespace: &str) -> Vec<StoredTraceAuthority> {
    let authorities: Vec<_> = ["primary", "secondary", "child-a", "child-b", "child-c"]
        .into_iter()
        .map(authority)
        .collect();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&ctx(namespace, Role::Admin), &hash(b"initial-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    for record in &authorities {
        store_trace_authority(store, &ctx(namespace, Role::Host), record)
            .await
            .unwrap();
    }
    let mut session = store.session().await.unwrap();
    // Explicit closure fixture edges, not a production import/learning pipeline.
    for record in &authorities[2..] {
        for source in ["primary", "secondary"] {
            session
                .put_edge(
                    &ctx(namespace, Role::Admin),
                    "run",
                    &record.record.id,
                    "run",
                    source,
                )
                .await
                .unwrap();
        }
    }
    session.commit().await.unwrap();
    authorities
}

async fn begin(store: &Store, namespace: &str) -> CleanupStatus {
    LifecycleStore::begin_revoke(
        &ctx(namespace, Role::Admin),
        store,
        TypedObjectRef {
            kind: "run".into(),
            id: "secondary".into(),
        },
        "revoke non-primary contributor",
        10,
    )
    .await
    .unwrap()
}

async fn drain_old_queue(
    store: &Store,
    namespace: &str,
    mut status: CleanupStatus,
) -> CleanupStatus {
    for now in 11..211 {
        if status.pending_nodes == 0 {
            break;
        }
        status = LifecycleStore::cleanup_step(
            &ctx(namespace, Role::Worker),
            store,
            &status.job_id,
            1,
            now,
        )
        .await
        .unwrap();
    }
    assert_eq!(status.state, CleanupState::Failed, "{status:?}");
    assert_eq!(status.pending_nodes, 0);
    status
}

/// Invoke on a pre-AG068 checkout with GENERATE set to a fresh local directory.
/// The default invocation does not generate anything and is not upgrade proof.
#[tokio::test]
async fn generate_legacy_failed_backup_with_real_host_writes_and_cleanup() {
    let Some(destination) = std::env::var_os(GENERATE) else {
        eprintln!("LEGACY_GENERATOR_NOT_REQUESTED");
        return;
    };
    let destination = PathBuf::from(destination);
    assert!(
        !destination.exists(),
        "refuse to overwrite a legacy artifact"
    );
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let records = new_authorities(&store, DEFAULT_NS).await;
    let failed = drain_old_queue(&store, DEFAULT_NS, begin(&store, DEFAULT_NS).await).await;
    let before = snapshot(&path, DEFAULT_NS);
    assert!(
        before["frontier"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["expanded"] == 1)
    );
    for record in &records[2..] {
        let row = before["objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["kind"] == "run" && row["id"] == record.record.id)
            .unwrap();
        let stored: StoredTraceAuthority =
            serde_json::from_str(row["body"].as_str().unwrap()).unwrap();
        stored.validate().unwrap();
    }
    store.backup(&destination).await.unwrap();
    store.close().await;
    let source_head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(source_head.status.success());
    let proof = json!({"source_head":String::from_utf8(source_head.stdout).unwrap().trim(),"namespace":DEFAULT_NS,
        "native_ids":records[2..].iter().map(|r| &r.record.id).collect::<Vec<_>>(),"status":failed,
        "generation":"real Host writer, explicit typed fixture edges, lifecycle API, consistent backup; no SQL state mutation",
        "database_sha256":hash(&std::fs::read(destination.join("rsia.sqlite3")).unwrap())});
    std::fs::write(
        destination.join("controller-provenance.json"),
        serde_json::to_vec_pretty(&proof).unwrap(),
    )
    .unwrap();
    eprintln!("LEGACY_GENERATED {proof}");
}

#[tokio::test]
async fn failed_host_authorities_upgrade_through_explicit_admin_full_frontier_retry() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("store").join("rsia.sqlite3");
    let (namespace, native_ids, failed, mut store, actual_old_program) = if let Some(backup) =
        std::env::var_os(OLD_BACKUP)
    {
        let backup = PathBuf::from(backup);
        let manifest: BackupManifest =
            serde_json::from_slice(&std::fs::read(backup.join("backup-manifest.json")).unwrap())
                .unwrap();
        assert!(manifest.complete);
        assert_eq!(manifest.schema_version, "rsia.backup.v2");
        let source_hash = hash(&std::fs::read(backup.join(&manifest.database.path)).unwrap());
        assert_eq!(source_hash, manifest.database.sha256);
        for blob in &manifest.blobs {
            assert_eq!(
                hash(&std::fs::read(backup.join(&blob.path)).unwrap()),
                blob.sha256
            );
        }
        let proof: Value = serde_json::from_slice(
            &std::fs::read(backup.join("controller-provenance.json")).unwrap(),
        )
        .unwrap();
        let namespace = proof["namespace"].as_str().unwrap().to_owned();
        let native_ids: Vec<String> = serde_json::from_value(proof["native_ids"].clone()).unwrap();
        let failed: CleanupStatus = serde_json::from_value(proof["status"].clone()).unwrap();
        assert_eq!(failed.state, CleanupState::Failed);
        assert_eq!(failed.pending_nodes, 0);
        copy_tree(&backup, path.parent().unwrap());
        let store = Store::open(&path).await.unwrap();
        eprintln!(
            "ACTUAL_OLD_PROGRAM source_head={} database_sha256={source_hash} namespace={namespace} status={failed:?}",
            proof["source_head"]
        );
        (namespace, native_ids, failed, store, true)
    } else {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let store = Store::open(&path).await.unwrap();
        let records = new_authorities(&store, DEFAULT_NS).await;
        // A current-program fixture makes old unclassified shapes fail through
        // cleanup, then repairs only the bodies. It is not a legacy binary proof.
        let mut session = store.session().await.unwrap();
        for record in &records[2..] {
            session.put(&ctx(DEFAULT_NS, Role::Admin), "run", &record.record.id, "fixture", &json!({"id":record.record.id,"schema_version":"unknown.fixture.v9","body":"old unknown"})).await.unwrap();
        }
        session.commit().await.unwrap();
        let failed = drain_old_queue(&store, DEFAULT_NS, begin(&store, DEFAULT_NS).await).await;
        let mut session = store.session().await.unwrap();
        for record in &records[2..] {
            session
                .put(
                    &ctx(DEFAULT_NS, Role::Admin),
                    "run",
                    &record.record.id,
                    "fixture",
                    record,
                )
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
        eprintln!("SELF_CONTAINED_CURRENT_FIXTURE; no actual old-program input");
        (
            DEFAULT_NS.into(),
            records[2..].iter().map(|r| r.record.id.clone()).collect(),
            failed,
            store,
            false,
        )
    };
    let before = snapshot(&path, &namespace);
    assert_eq!(native_ids.len(), 3);
    assert_eq!(before["jobs"][0]["state"], "failed");
    assert!(
        before["frontier"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["expanded"] == 1)
    );
    let mut originals = Vec::new();
    for id in &native_ids {
        let row = before["objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["kind"] == "run" && row["id"] == *id)
            .unwrap();
        let body = row["body"].as_str().unwrap().to_owned();
        let authority: StoredTraceAuthority = serde_json::from_str(&body).unwrap();
        authority.validate().unwrap();
        originals.push((id.clone(), body, authority));
    }
    let worker = LifecycleStore::cleanup_step(
        &ctx(&namespace, Role::Worker),
        &store,
        &failed.job_id,
        1,
        250,
    )
    .await
    .unwrap();
    assert_eq!(worker.state, CleanupState::Failed);
    assert_eq!(worker.processed_nodes, failed.processed_nodes);
    let mut status = LifecycleStore::cleanup_step(
        &ctx(&namespace, Role::Admin),
        &store,
        &failed.job_id,
        1,
        300,
    )
    .await
    .unwrap();
    assert_eq!(
        status.state,
        CleanupState::Running,
        "old expanded frontier must reopen: {status:?}"
    );
    assert_eq!(status.processed_nodes, 1);
    assert_eq!(status.last_error, failed.last_error);
    for now in 301..601 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        store.close().await;
        store = Store::open(&path).await.unwrap();
        status = LifecycleStore::cleanup_step(
            &ctx(&namespace, Role::Admin),
            &store,
            &failed.job_id,
            1,
            now,
        )
        .await
        .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert!(status.last_error.is_none());
    assert_eq!(status.source, failed.source);
    assert_eq!(status.watermark_seq, failed.watermark_seq);
    assert_eq!(status.watermark_digest, failed.watermark_digest);
    assert_eq!(status.processed_nodes, failed.processed_nodes);
    let after = snapshot(&path, &namespace);
    assert_eq!(after["watermark"], before["watermark"]);
    assert_eq!(after["edges"], before["edges"]);
    assert_eq!(after["audit"], before["audit"]);
    assert_eq!(
        after["jobs"][0]["created_at"],
        before["jobs"][0]["created_at"]
    );
    let old_events = before["events"].as_array().unwrap();
    assert_eq!(
        &after["events"].as_array().unwrap()[..old_events.len()],
        old_events
    );
    let retry_events: Vec<_> = after["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event_kind"] == "cleanup_retry_started")
        .collect();
    assert_eq!(retry_events.len(), 1);
    let retry_details: Value =
        serde_json::from_str(retry_events[0]["details"].as_str().unwrap()).unwrap();
    assert_eq!(
        retry_details["previous_processed_nodes"],
        failed.processed_nodes
    );
    assert_eq!(
        retry_details["previous_last_error"],
        json!(failed.last_error)
    );
    for (id, original_body, authority) in &originals {
        let row = after["objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["kind"] == "run" && row["id"] == *id)
            .unwrap();
        let body: Value = serde_json::from_str(row["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["schema_version"], "rsia.redacted.v1");
        assert_eq!(body["id"], *id);
        assert_eq!(body["original_digest"], hash(original_body.as_bytes()));
        assert!(!body.to_string().contains("EXCERPT") && !body.to_string().contains("DIAGNOSIS"));
        let cache = after["cache"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["subject_id"] == *id)
            .unwrap();
        assert_eq!(cache["response"], "null");
        assert_eq!(cache["redacted"], 1);
        let mut session = store.session().await.unwrap();
        assert!(
            load_stored_source(&mut session, &ctx(&namespace, Role::Host), id)
                .await
                .is_err()
        );
        session.commit().await.unwrap();
        assert!(
            store_trace_authority(&store, &ctx(&namespace, Role::Host), authority)
                .await
                .is_err()
        );
    }
    for row in before["objects"].as_array().unwrap() {
        let old: Value = serde_json::from_str(row["body"].as_str().unwrap()).unwrap();
        if old["schema_version"] == "rsia.redacted.v1" {
            let current = after["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["kind"] == row["kind"] && v["id"] == row["id"])
                .unwrap();
            assert_eq!(
                current, row,
                "already redacted original identity/digest changed"
            );
        }
    }
    for now in 700..703 {
        store.close().await;
        store = Store::open(&path).await.unwrap();
        assert_eq!(
            LifecycleStore::cleanup_step(
                &ctx(&namespace, Role::Admin),
                &store,
                &failed.job_id,
                1,
                now
            )
            .await
            .unwrap()
            .state,
            CleanupState::Complete
        );
        assert_eq!(snapshot(&path, &namespace), after);
    }
    eprintln!("UPGRADE_COMPLETE actual_old_program={actual_old_program} status={status:?}");
}
