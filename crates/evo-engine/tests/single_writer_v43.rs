use evo_engine::data_directory_lock::{DataDirectoryLock, mount_gate};
use evo_engine::startup_gate::{DATA_LOCK_FILE, RESTORE_RECEIPT_FILE};
use evo_storage::Store;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::process::{Command, Output, Stdio};

// Isolate process-spawning fixtures: fork/exec may briefly inherit another
// test thread's file descriptor and keep its OS lock alive past parent drop.
static PROCESS_FIXTURES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let key = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                out.insert(format!("{key}/"), Vec::new());
                visit(root, &path, out);
            } else {
                out.insert(key, fs::read(path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn formal(database: &Path, operation: &str) -> Output {
    let mut command = serde_json::json!({"operation":operation,"database":database,
        "namespace":"writer-test","actor":"grader","ticket_id":"missing-ticket"});
    if operation == "grade" {
        command["receipt_id"] = "missing-receipt".into();
        command["target_id"] = "missing-target".into();
        command["candidate_execution_receipt_id"] = "missing-candidate".into();
        command["baseline_execution_receipt_id"] = "missing-baseline".into();
        command["issued_at_unix_seconds"] = 1.into();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_formal_local_host"))
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_ne!(child.id(), std::process::id());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&command).unwrap())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn live_artifacts(root: &Path) {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.recursive(true).create(root.join(".blob-staging/live-writer")).unwrap();
    let mut options = File::options();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(root.join(".blob-staging/live-writer/payload.tmp"))
        .unwrap().write_all(b"in-progress payload").unwrap();
    fs::write(
        root.join("closed-facts-sentinel"),
        b"unchanged close and in-flight facts",
    )
    .unwrap();
}

#[test]
fn locked_formal_grade_and_finalize_leave_live_writer_bytes_unchanged() {
    let _fixture = PROCESS_FIXTURES.blocking_lock();
    for operation in ["grade", "finalize"] {
        let dir = tempfile::tempdir().unwrap();
        live_artifacts(dir.path());
        let mut options = File::options();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut lock = options.open(dir.path().join(DATA_LOCK_FILE)).unwrap();
        lock.write_all(b"live writer lock marker").unwrap();
        lock.try_lock().unwrap();
        let before = snapshot(dir.path());
        let output = formal(&dir.path().join("database.sqlite3"), operation);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
            serde_json::json!({"status":"refused","reason":"conflict"})
        );
        assert_eq!(snapshot(dir.path()), before);
    }
}

#[test]
fn restored_formal_commands_refuse_before_lock_or_database_writes() {
    let _fixture = PROCESS_FIXTURES.blocking_lock();
    for operation in ["grade", "finalize"] {
        let dir = tempfile::tempdir().unwrap();
        live_artifacts(dir.path());
        let mut options = File::options();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        options.open(dir.path().join(RESTORE_RECEIPT_FILE)).unwrap()
            .write_all(b"restore bytes have no admission or trusted anchor").unwrap();
        let before = snapshot(dir.path());
        let output = formal(&dir.path().join("database.sqlite3"), operation);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
            serde_json::json!({"status":"refused","reason":"conflict"})
        );
        assert_eq!(snapshot(dir.path()), before);
        assert!(!dir.path().join(DATA_LOCK_FILE).exists());
    }
}

#[test]
fn different_database_names_and_canonical_aliases_share_one_lock() {
    let _fixture = PROCESS_FIXTURES.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let first = DataDirectoryLock::acquire(&dir.path().join("a.sqlite3")).unwrap();
    for path in [dir.path().join("b.sqlite3"), dir.path().join("./a.sqlite3")] {
        assert_eq!(
            DataDirectoryLock::acquire(&path).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    drop(first);
    DataDirectoryLock::acquire(&dir.path().join("b.sqlite3")).unwrap();
}

#[tokio::test]
async fn closed_store_releases_guard_only_after_connections_close() {
    let _fixture = PROCESS_FIXTURES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("database.sqlite3");
    let (guard, decision) = mount_gate(&data, None).await.unwrap();
    let store = Store::open(&data).await.unwrap();
    decision.admit().unwrap();
    store.close().await;
    // Closing a pool does not itself release the OS guard.
    assert!(DataDirectoryLock::acquire(&data).is_err());
    drop(guard);
    let (next_guard, next_decision) = mount_gate(&data, None).await.unwrap();
    let next = Store::open(&data).await.unwrap();
    next_decision.admit().unwrap();
    assert_eq!(next.integrity().await.unwrap(), "ok");
    next.close().await;
    drop(next_guard);
}

#[test]
fn failed_formal_operation_closes_and_unlocks_for_next_writer() {
    let _fixture = PROCESS_FIXTURES.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("database.sqlite3");
    for _ in 0..2 {
        let output = formal(&data, "finalize");
        assert!(!output.status.success());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap()["status"],
            "refused"
        );
        assert!(data.is_file());
        let guard = DataDirectoryLock::acquire(&data).unwrap();
        drop(guard);
    }
}

#[tokio::test]
async fn live_dispatched_call_and_execution_close_facts_survive_formal_refusal() {
    let _fixture = PROCESS_FIXTURES.lock().await;
    use evo_core::{Context, Role, hash};
    use evo_storage::budget::{
        BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization,
    };
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("database.sqlite3");
    let (guard, gate) = mount_gate(&data, None).await.unwrap();
    let store = Store::open(&data).await.unwrap();
    gate.admit().unwrap();
    let admin = Context::new("writer-test", "admin", Role::Admin).unwrap();
    let host = Context::new("writer-test", "host", Role::Host).unwrap();
    // An accounting fixture reserves one virtual micro-unit. No transport is
    // called and no monetary charge or execution result is produced.
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root".into(),
                billing_scope: "scope".into(),
                allowed_namespaces: vec!["writer-test".into()],
                currency: "USD".into(),
                pricing_version: "fixture".into(),
                payment_subject: "fixture".into(),
                authorization_receipt_digest: hash(b"fixture authorization"),
                per_call_cap_micros: 1,
                total_limit_micros: 1,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let call = store
        .reserve_budget_call(
            &host,
            &BudgetCallReservation {
                billing_scope: "scope".into(),
                call_id: "live-call".into(),
                dispatch_group_id: "group".into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: hash(b"fixture input"),
                request_artifact: None,
                max_cost_micros: 1,
                lease_token: "lease".into(),
                lease_until: 100,
                now: 2,
            },
        )
        .await
        .unwrap();
    let dispatched = store
        .begin_budget_dispatch(
            &host,
            &BudgetCallFence {
                billing_scope: call.billing_scope.clone(),
                call_id: call.call_id.clone(),
                actual_input_digest: call.actual_input_digest.clone(),
                lease_token: call.lease_token.clone(),
                lease_epoch: call.lease_epoch,
                now: 3,
            },
        )
        .await
        .unwrap()
        .call;
    live_artifacts(dir.path());
    for operation in ["grade", "finalize"] {
        let before = snapshot(dir.path());
        let output = formal(&data, operation);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
            serde_json::json!({"status":"refused","reason":"conflict"})
        );
        assert_eq!(snapshot(dir.path()), before);
        assert_eq!(
            store
                .budget_call(&host, "scope", "live-call")
                .await
                .unwrap()
                .unwrap(),
            dispatched
        );
    }
    store.close().await;
    drop(guard);
}

#[tokio::test(flavor = "current_thread")]
async fn failed_migration_joins_pool_connections_before_returning() {
    let _fixture = PROCESS_FIXTURES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("database.sqlite3");
    let store = evo_storage::Store::open(&data).await.unwrap();
    store.close().await;
    let tamper=std::process::Command::new("python3").arg("-c").arg("import sqlite3,sys\nc=sqlite3.connect(sys.argv[1])\nc.execute(\"UPDATE _sqlx_migrations SET checksum=X'00' WHERE version=(SELECT MIN(version) FROM _sqlx_migrations)\")\nc.commit()\nc.close()")
        .arg(&data).output().unwrap();
    assert!(
        tamper.status.success(),
        "{}",
        String::from_utf8_lossy(&tamper.stderr)
    );
    let failed = evo_storage::Store::open(&data).await;
    assert!(failed.is_err(), "checksum drift should reject migration");
    // This synchronous child starts before the current-thread executor can
    // run any cleanup task left behind by a failed Store::open.
    let probe=std::process::Command::new("python3").arg("-c").arg("import sqlite3,sys\nc=sqlite3.connect(sys.argv[1],timeout=0.2)\nprint(c.execute('PRAGMA journal_mode=DELETE').fetchone()[0])\nc.close()")
        .arg(&data).output().unwrap();
    println!(
        "migration error: probe_exit={:?} stdout={} stderr={}",
        probe.status.code(),
        String::from_utf8_lossy(&probe.stdout),
        String::from_utf8_lossy(&probe.stderr)
    );
    assert!(
        probe.status.success(),
        "failed open retained a SQLite connection after returning"
    );
    assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), "delete");
}
