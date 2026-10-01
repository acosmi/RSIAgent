//! E16.5 startup recovery quarantine and deployment gate (AG-018).
//!
//! Every positive and negative case that involves a restored directory starts
//! from a real `Store::backup` and a real run of `scripts/restore_backup.py`, so
//! the gate is tested against the receipt and the database the script actually
//! produces, not against a hand-written imitation.
//!
//! Zero-write is asserted structurally: every failure helper snapshots the data
//! directory (file list plus the sha256 of each file) before and after the
//! evaluation and requires the two to be identical.

use evo_core::{Context, Role, hash};
use evo_engine::startup_gate::{
    RECEIPT_SCOPE, RESTORE_ADMISSION_FILE, RESTORE_ADMISSION_SCHEMA, RESTORE_RECEIPT_FILE,
    RESTORE_RECEIPT_SCHEMA, RecoveryPosture, StartupGate, StartupGateError, deployment_config,
};
use evo_storage::Store;
use evo_storage::lifecycle::{
    BackupManifest, LifecycleStore, PROTECTED_OBJECT_KINDS, PROTECTED_SCHEMA_VERSIONS,
    ROOT_BUDGET_TABLE_QUERY, RevokeTombstone, TypedObjectRef,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RESTORE_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/restore_backup.py"
);
const ANCHOR_WITHOUT_RESTORE: &str = "startup_rejected: --trusted-revocations-db is only accepted for a restored data directory that has not been admitted";

const ROOT_BUDGET_FACTS: &str = "INSERT INTO root_budgets(billing_scope,root_budget_id,authorizing_namespace,currency,pricing_version,payment_subject,authorization_receipt_digest,per_call_cap_micros,total_limit_micros,spent_micros,reserved_micros,created_at) VALUES('scope-a','root-a','n','USD','pv1','subject','0000000000000000000000000000000000000000000000000000000000000000',100,1000,10,0,1);
INSERT INTO root_budget_namespaces(billing_scope,namespace) VALUES('scope-a','n');
INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','reservation','res-1','admin','{\"id\":\"res-1\",\"state\":\"reserved\"}');
INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','artifact','ledger-1','admin','{\"schema_version\":\"rsia.typed_artifact_envelope.v1\",\"id\":\"ledger-1\",\"record_kind\":\"exposure_ledger_v1\",\"payload\":{\"spent\":1}}')";

fn ctx(actor: &str, role: Role) -> Context {
    Context::new("n", actor, role).unwrap()
}

/// Raw SQL through Python's `sqlite3` (this crate has no direct sqlx dependency).
fn sql(database: &Path, script: &str) {
    let status = Command::new("python3")
        .arg("-c")
        .arg("import sqlite3,sys\ncon=sqlite3.connect(sys.argv[1])\ncon.executescript(sys.argv[2])\ncon.commit()\ncon.close()")
        .arg(database)
        .arg(script)
        .status()
        .unwrap();
    assert!(status.success(), "sql failed: {script}");
}

/// Relative path -> content hash (files), target (symlinks) or "dir".
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().display().to_string();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                out.insert(
                    relative,
                    format!("symlink:{}", std::fs::read_link(&path).unwrap().display()),
                );
            } else if meta.is_dir() {
                out.insert(relative, "dir".into());
                walk(root, &path, out);
            } else {
                out.insert(relative, hash(&std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// A real backup of a live database, restored by the real script into `restored`.
struct Fixture {
    _guard: tempfile::TempDir,
    root: PathBuf,
    /// The live database: the trusted anchor the restore was verified against.
    live: PathBuf,
    /// The restored data directory (`D`).
    restored: PathBuf,
    /// `D/rsia.sqlite3`, the `--data` file.
    data: PathBuf,
}

impl Fixture {
    /// The live database holds namespace `n` with two runs, a root budget row, a
    /// reservation and an exposure-ledger artifact. `run-2` is revoked after the
    /// backup, so the restore replays one revocation into `D`.
    async fn new() -> Self {
        let guard = tempfile::tempdir().unwrap();
        let root = guard.path().canonicalize().unwrap();
        let live = root.join("rsia.sqlite3");
        Store::open(&live).await.unwrap().close().await;
        sql(&live, ROOT_BUDGET_FACTS);

        let store = Store::open(&live).await.unwrap();
        let admin = ctx("admin", Role::Admin);
        let host = ctx("host", Role::Host);
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&admin, &hash(b"initial-watermark"))
            .await
            .unwrap();
        for id in ["run-1", "run-2"] {
            session
                .put(
                    &host,
                    "run",
                    id,
                    host.actor(),
                    &json!({"id":id,"schema_version":"rsia.optimization.source.v1","body":id}),
                )
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
        let backup = root.join("backup");
        store.backup(&backup).await.unwrap();

        let revoked = LifecycleStore::begin_revoke(
            &admin,
            &store,
            TypedObjectRef {
                kind: "run".into(),
                id: "run-2".into(),
            },
            "revoked after the backup",
            9,
        )
        .await
        .unwrap();
        let mut session = store.session().await.unwrap();
        let tombstone: RevokeTombstone = session.need(&admin, "tombstone", "run-2").await.unwrap();
        session.commit().await.unwrap();
        store.close().await;

        let manifest_bytes = std::fs::read(backup.join("backup-manifest.json")).unwrap();
        let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(manifest.watermarks.len(), 1);
        let base = &manifest.watermarks[0];
        let delta = root.join("delta.json");
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
                        "source_kind":"run",
                        "source_id":"run-2",
                        "tombstone_digest":hash(&serde_json::to_vec(&tombstone).unwrap()),
                        "reason":"revoked after the backup",
                        "created_at":9,
                    }],
                    "latest_seq":base.seq + 1,
                    "latest_digest":revoked.watermark_digest,
                }],
            }))
            .unwrap(),
        )
        .unwrap();

        let restored = root.join("restored");
        let output = Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg(RESTORE_SCRIPT)
            .arg("--backup")
            .arg(&backup)
            .arg("--dest")
            .arg(&restored)
            .arg("--revoke-delta")
            .arg(&delta)
            .arg("--trusted-revocations-db")
            .arg(&live)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("RESTORE_OK events=1"),
            "restore failed: {stdout} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let data = restored.join("rsia.sqlite3");
        Self {
            _guard: guard,
            root,
            live,
            restored,
            data,
        }
    }

    fn unanchored(&self) -> StartupGate {
        StartupGate::new(&self.data, None)
    }

    fn anchored(&self) -> StartupGate {
        self.with_anchor(&self.live)
    }

    fn with_anchor(&self, anchor: &Path) -> StartupGate {
        StartupGate::new(&self.data, Some(anchor))
    }

    fn receipt_path(&self) -> PathBuf {
        self.restored.join(RESTORE_RECEIPT_FILE)
    }

    fn receipt_bytes(&self) -> Vec<u8> {
        std::fs::read(self.receipt_path()).unwrap()
    }

    fn receipt_text(&self) -> String {
        String::from_utf8(self.receipt_bytes()).unwrap()
    }

    fn admission_path(&self) -> PathBuf {
        self.restored.join(RESTORE_ADMISSION_FILE)
    }

    /// A directory that holds an independent, byte-identical copy of the live
    /// database (the live database is closed, so the copy is complete).
    fn independent_anchor(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        std::fs::create_dir(&dir).unwrap();
        let anchor = dir.join("rsia.sqlite3");
        std::fs::copy(&self.live, &anchor).unwrap();
        anchor
    }

    /// Evaluate against the live anchor and admit, as the service does.
    async fn admit(&self) {
        let decision = self.anchored().evaluate().await.unwrap();
        assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);
        decision.admit().unwrap();
    }
}

/// The evaluation must be refused with `recovery_quarantine` naming `needle`,
/// and must not have touched `protected` (the data directory).
async fn expect_quarantine(gate: &StartupGate, protected: &Path, needle: &str) -> String {
    let before = snapshot(protected);
    let error = gate.evaluate().await.unwrap_err();
    let StartupGateError::Quarantine(reason) = &error else {
        panic!("expected a quarantine containing {needle:?}, got {error}");
    };
    let text = error.to_string();
    assert!(
        text.starts_with("recovery_quarantine: ") && text.ends_with("; not mounting"),
        "{text}"
    );
    assert!(reason.contains(needle), "{needle:?} not in {text:?}");
    assert_eq!(
        snapshot(protected),
        before,
        "a refused startup changed the data directory ({text})"
    );
    text
}

async fn expect_rejected(gate: &StartupGate, protected: &Path) {
    let before = snapshot(protected);
    let error = gate.evaluate().await.unwrap_err();
    assert_eq!(error.to_string(), ANCHOR_WITHOUT_RESTORE);
    assert!(matches!(error, StartupGateError::Rejected(_)));
    assert_eq!(snapshot(protected), before, "a rejected startup wrote");
}

fn no_admission_temp_files(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !name.contains(".tmp-"),
            "a temporary admission file was left behind: {name}"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. no receipt, no anchor: Normal, and the gate itself writes nothing
// ---------------------------------------------------------------------------
#[tokio::test]
async fn an_ordinary_directory_is_normal_and_the_gate_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("rsia.sqlite3");
    Store::open(&data).await.unwrap().close().await;
    let before = snapshot(dir.path());

    let decision = StartupGate::new(&data, None).evaluate().await.unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::Normal);
    assert!(decision.pending_admission().is_none());
    assert_eq!(
        decision.startup_line(),
        "rsia startup: code_execution=disabled sandbox=unavailable recovery=normal"
    );
    assert!(!decision.isolation().sandbox_available);
    decision.admit().unwrap();
    assert_eq!(snapshot(dir.path()), before, "evaluate and admit wrote");

    // A directory that does not exist yet is normal too, and stays nonexistent.
    let missing = dir.path().join("not-yet").join("rsia.sqlite3");
    let decision = StartupGate::new(&missing, None).evaluate().await.unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::Normal);
    assert!(!dir.path().join("not-yet").exists());

    // A bare file name reads its empty parent as the current directory (the
    // package root under `cargo test`, which holds no restore.json).
    let decision = StartupGate::new(Path::new("rsia.sqlite3"), None)
        .evaluate()
        .await
        .unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::Normal);
}

// ---------------------------------------------------------------------------
// 2. a receipt without an anchor is quarantined, untouched
// ---------------------------------------------------------------------------
#[tokio::test]
async fn a_restored_directory_without_an_anchor_is_quarantined_untouched() {
    let fixture = Fixture::new().await;
    let receipt = fixture.receipt_bytes();
    let database = std::fs::read(&fixture.data).unwrap();
    let before = snapshot(&fixture.restored);
    assert_eq!(
        before.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["restore.json", "rsia.sqlite3"]
    );

    expect_quarantine(
        &fixture.unanchored(),
        &fixture.restored,
        "no admission record and no --trusted-revocations-db",
    )
    .await;

    assert_eq!(fixture.receipt_bytes(), receipt);
    assert_eq!(std::fs::read(&fixture.data).unwrap(), database);
    assert_eq!(snapshot(&fixture.restored), before);
    assert!(!fixture.admission_path().exists());
}

// ---------------------------------------------------------------------------
// 3. verified -> admitted once -> admitted previously, anchor no longer needed
// ---------------------------------------------------------------------------
#[tokio::test]
async fn a_verified_restore_is_admitted_once_and_then_needs_no_anchor() {
    let fixture = Fixture::new().await;
    let root_before = snapshot(&fixture.root);

    let decision = fixture.anchored().evaluate().await.unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);
    assert_eq!(
        decision.startup_line(),
        "rsia startup: code_execution=disabled sandbox=unavailable recovery=restored_verified"
    );
    assert_eq!(
        snapshot(&fixture.root),
        root_before,
        "verification wrote somewhere (data directory, anchor directory or backup)"
    );
    let pending = decision.pending_admission().unwrap().clone();
    assert_eq!(pending.namespaces, vec!["n".to_owned()]);
    assert_eq!(
        pending.anchor_path_local_only,
        fixture.live.to_str().unwrap()
    );
    assert_eq!(pending.receipt_sha256, hash(&fixture.receipt_bytes()));
    assert_eq!(pending.verified_facts_digest.len(), 64);
    assert!(
        !fixture.admission_path().exists(),
        "evaluate admits nothing"
    );

    // The caller bootstraps the store between evaluating and admitting.
    Store::open(&fixture.data).await.unwrap().close().await;
    let receipt_before = fixture.receipt_bytes();
    decision.admit().unwrap();

    // restore.json stays; the admission binds to its exact bytes.
    assert_eq!(fixture.receipt_bytes(), receipt_before);
    let admission_bytes = std::fs::read(fixture.admission_path()).unwrap();
    assert!(admission_bytes.ends_with(b"\n"));
    let admission: Value = serde_json::from_slice(&admission_bytes).unwrap();
    let keys: Vec<&str> = admission
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut sorted_keys = keys.clone();
    sorted_keys.sort_unstable();
    assert_eq!(
        sorted_keys,
        vec![
            "admitted_at",
            "anchor_path_local_only",
            "namespaces",
            "receipt_sha256",
            "schema_version",
            "scope",
            "verified_facts_digest"
        ]
    );
    // on disk the fields follow the record's declared order
    let text = String::from_utf8(admission_bytes.clone()).unwrap();
    let positions: Vec<usize> = [
        "schema_version",
        "receipt_sha256",
        "anchor_path_local_only",
        "verified_facts_digest",
        "namespaces",
        "admitted_at",
        "scope",
    ]
    .iter()
    .map(|key| text.find(&format!("\"{key}\":")).unwrap())
    .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    assert_eq!(admission["schema_version"], RESTORE_ADMISSION_SCHEMA);
    assert_eq!(admission["scope"], RECEIPT_SCOPE);
    assert_eq!(admission["receipt_sha256"], hash(&receipt_before));
    assert_eq!(
        admission["anchor_path_local_only"],
        fixture.live.to_str().unwrap()
    );
    assert_eq!(admission["namespaces"], json!(["n"]));
    assert_eq!(
        admission["verified_facts_digest"],
        pending.verified_facts_digest
    );
    let admitted_at = admission["admitted_at"].as_i64().unwrap();
    assert!((evo_core::now() - admitted_at).abs() < 300);
    no_admission_temp_files(&fixture.restored);

    // Second start: the admission record is enough, and the anchor is not needed.
    let after_admission = snapshot(&fixture.restored);
    let second = fixture.unanchored().evaluate().await.unwrap();
    assert_eq!(second.posture(), RecoveryPosture::AdmittedPreviously);
    assert!(second.pending_admission().is_none());
    assert_eq!(
        second.startup_line(),
        "rsia startup: code_execution=disabled sandbox=unavailable recovery=admitted_previously"
    );
    second.admit().unwrap();
    assert_eq!(snapshot(&fixture.restored), after_admission);

    // An anchor for an admitted directory is refused.
    expect_rejected(&fixture.anchored(), &fixture.restored).await;

    // An admission is never overwritten: the stale decision cannot publish again.
    let again = decision.admit().unwrap_err().to_string();
    assert!(again.starts_with("recovery_admission_failed: "), "{again}");
    assert!(again.ends_with("; not serving"), "{again}");
    assert_eq!(
        std::fs::read(fixture.admission_path()).unwrap(),
        admission_bytes
    );
    no_admission_temp_files(&fixture.restored);
}

#[tokio::test]
async fn a_crash_before_admission_is_reverified_without_writing_anything() {
    let fixture = Fixture::new().await;
    let decision = fixture.anchored().evaluate().await.unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);
    // bootstrap ran (the database now carries a WAL-mode header), then the process died
    Store::open(&fixture.data).await.unwrap().close().await;
    assert!(!fixture.admission_path().exists());

    let before = snapshot(&fixture.restored);
    let again = fixture.anchored().evaluate().await.unwrap();
    assert_eq!(again.posture(), RecoveryPosture::RestoredVerified);
    assert_eq!(
        again.pending_admission().unwrap().verified_facts_digest,
        decision.pending_admission().unwrap().verified_facts_digest,
        "bootstrapping changed none of the compared facts"
    );
    assert_eq!(snapshot(&fixture.restored), before);
    // and without the anchor it is still quarantined
    expect_quarantine(
        &fixture.unanchored(),
        &fixture.restored,
        "no admission record",
    )
    .await;
}

// ---------------------------------------------------------------------------
// 4. the anchor moved on after the restore
// ---------------------------------------------------------------------------
#[tokio::test]
async fn an_anchor_that_advanced_since_the_restore_quarantines_the_directory() {
    let fixture = Fixture::new().await;
    // A new revocation lands on the live control plane after the restore.
    let store = Store::open(&fixture.live).await.unwrap();
    let admin = ctx("admin", Role::Admin);
    let host = ctx("host", Role::Host);
    let mut session = store.session().await.unwrap();
    session
        .put(
            &host,
            "run",
            "run-3",
            host.actor(),
            &json!({"id":"run-3","schema_version":"rsia.optimization.source.v1","body":"3"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "run-3".into(),
        },
        "revoked after the restore",
        11,
    )
    .await
    .unwrap();
    store.close().await;

    let reason = expect_quarantine(
        &fixture.anchored(),
        &fixture.restored,
        "advanced since restore",
    )
    .await;
    assert!(reason.contains("(namespace n: seq 2 → 3)"), "{reason}");
    assert!(reason.contains("re-run restore_backup.py"), "{reason}");
}

#[tokio::test]
async fn an_anchor_with_a_namespace_the_restore_never_saw_is_quarantined() {
    let fixture = Fixture::new().await;
    let store = Store::open(&fixture.live).await.unwrap();
    let other = Context::new("m", "admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&other, &hash(b"another-namespace"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;
    let reason = expect_quarantine(
        &fixture.anchored(),
        &fixture.restored,
        "advanced since restore (new namespace(s): m)",
    )
    .await;
    assert!(reason.contains("re-run restore_backup.py"), "{reason}");
}

// ---------------------------------------------------------------------------
// 5. the anchor consumed facts the restored directory does not have
// ---------------------------------------------------------------------------
#[tokio::test]
async fn an_anchor_that_consumed_more_exposure_is_quarantined() {
    let fixture = Fixture::new().await;
    sql(
        &fixture.live,
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','artifact','consumed-query','admin','{\"schema_version\":\"rsia.typed_artifact_envelope.v1\",\"id\":\"consumed-query\",\"record_kind\":\"exposure_ledger_v1\",\"payload\":{\"spent\":2}}')",
    );
    let reason = expect_quarantine(
        &fixture.anchored(),
        &fixture.restored,
        "consumed accounting facts differ",
    )
    .await;
    assert!(
        reason.contains("only in the trusted anchor: n/artifact/consumed-query"),
        "{reason}"
    );
}

#[tokio::test]
async fn an_anchor_that_spent_more_of_the_root_budget_is_quarantined() {
    let fixture = Fixture::new().await;
    sql(
        &fixture.live,
        "UPDATE root_budgets SET spent_micros=spent_micros+5 WHERE billing_scope='scope-a'",
    );
    let reason = expect_quarantine(
        &fixture.anchored(),
        &fixture.restored,
        "root budget tables differ",
    )
    .await;
    assert!(reason.contains("changed: root_budgets"), "{reason}");

    // an appended dispatch event is also consumed state
    let fixture = Fixture::new().await;
    sql(
        &fixture.live,
        "INSERT INTO root_budget_events(billing_scope,call_id,event_kind,event_at,details) VALUES('scope-a',NULL,'dispatched',5,'{}')",
    );
    let reason = expect_quarantine(
        &fixture.anchored(),
        &fixture.restored,
        "root budget tables differ",
    )
    .await;
    assert!(reason.contains("root_budget_events"), "{reason}");
}

// ---------------------------------------------------------------------------
// 6. the anchor must be independent of the directory it vouches for
// ---------------------------------------------------------------------------
#[cfg(unix)]
#[tokio::test]
async fn an_anchor_must_be_independent_of_the_data_directory() {
    let fixture = Fixture::new().await;

    // the data directory's own database
    expect_quarantine(
        &fixture.with_anchor(&fixture.data),
        &fixture.restored,
        "trusted anchor",
    )
    .await;
    // a hard link to it (also created outside the directory)
    let hard_link = fixture.root.join("hard-link.sqlite3");
    std::fs::hard_link(&fixture.data, &hard_link).unwrap();
    expect_quarantine(
        &fixture.with_anchor(&hard_link),
        &fixture.restored,
        "same device and inode",
    )
    .await;
    std::fs::remove_file(&hard_link).unwrap();

    // a copy inside the data directory
    let inside = fixture.restored.join("copy.sqlite3");
    std::fs::copy(&fixture.live, &inside).unwrap();
    expect_quarantine(
        &fixture.with_anchor(&inside),
        &fixture.restored,
        "lies inside the data directory",
    )
    .await;
    std::fs::remove_file(&inside).unwrap();

    // a symlink to a perfectly good database
    let link = fixture.root.join("link.sqlite3");
    std::os::unix::fs::symlink(&fixture.live, &link).unwrap();
    expect_quarantine(
        &fixture.with_anchor(&link),
        &fixture.restored,
        "must not be a symlink",
    )
    .await;

    // a path that reaches the anchor through a symlinked directory
    let via_dir = fixture.root.join("via-dir");
    std::os::unix::fs::symlink(&fixture.root, &via_dir).unwrap();
    expect_quarantine(
        &fixture.with_anchor(&via_dir.join("rsia.sqlite3")),
        &fixture.restored,
        "not canonical",
    )
    .await;

    // relative and dot-dot paths
    expect_quarantine(
        &fixture.with_anchor(Path::new("rsia.sqlite3")),
        &fixture.restored,
        "must be absolute",
    )
    .await;
    std::fs::create_dir(fixture.root.join("sub")).unwrap();
    expect_quarantine(
        &fixture.with_anchor(&fixture.root.join("sub").join("..").join("rsia.sqlite3")),
        &fixture.restored,
        "not canonical",
    )
    .await;

    // a directory that holds a backup manifest
    let backup_like = fixture.independent_anchor("backup-like");
    std::fs::write(
        backup_like.parent().unwrap().join("backup-manifest.json"),
        b"{}",
    )
    .unwrap();
    expect_quarantine(
        &fixture.with_anchor(&backup_like),
        &fixture.restored,
        "backup-manifest.json",
    )
    .await;

    // a missing and a non-database anchor
    expect_quarantine(
        &fixture.with_anchor(&fixture.root.join("missing.sqlite3")),
        &fixture.restored,
        "not accessible",
    )
    .await;
    let not_sqlite = fixture.root.join("not-sqlite.db");
    std::fs::write(&not_sqlite, vec![b'x'; 200]).unwrap();
    expect_quarantine(
        &fixture.with_anchor(&not_sqlite),
        &fixture.restored,
        "not a valid SQLite format 3 file",
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn an_anchor_directory_locked_by_a_running_rsia_process_is_refused() {
    let fixture = Fixture::new().await;

    // Another fd in this process holds the anchor directory's lock.
    let held = fixture.independent_anchor("held-by-fd");
    let lock_path = held.parent().unwrap().join(".rsia.lock");
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&lock_path)
        .unwrap();
    holder.lock().unwrap();
    expect_quarantine(
        &fixture.with_anchor(&held),
        &fixture.restored,
        "anchor is held by a running RSIA process",
    )
    .await;
    holder.unlock().unwrap();
    drop(holder);
    // once released the same anchor is fine
    let root_before = snapshot(&fixture.root);
    let decision = fixture.with_anchor(&held).evaluate().await.unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);
    assert_eq!(
        snapshot(&fixture.root),
        root_before,
        "the probe left no trace"
    );

    // Another process holds it.
    let held_by_process = fixture.independent_anchor("held-by-process");
    let process_lock = held_by_process.parent().unwrap().join(".rsia.lock");
    let mut child = Command::new("python3")
        .arg("-c")
        .arg("import fcntl,sys\nf=open(sys.argv[1],'a+')\nfcntl.flock(f,fcntl.LOCK_EX)\nprint('locked',flush=True)\nsys.stdin.read()")
        .arg(&process_lock)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "locked");
    expect_quarantine(
        &fixture.with_anchor(&held_by_process),
        &fixture.restored,
        "anchor is held by a running RSIA process",
    )
    .await;
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
    assert_eq!(
        fixture
            .with_anchor(&held_by_process)
            .evaluate()
            .await
            .unwrap()
            .posture(),
        RecoveryPosture::RestoredVerified
    );
}

#[tokio::test]
async fn an_independent_copy_of_the_live_database_is_an_acceptable_anchor() {
    // The positive control for the independence cases: nothing but the stated
    // property differs between this anchor and the refused ones.
    let fixture = Fixture::new().await;
    let anchor = fixture.independent_anchor("independent");
    let before = snapshot(&fixture.root);
    let decision = fixture.with_anchor(&anchor).evaluate().await.unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);
    assert_eq!(
        decision.pending_admission().unwrap().anchor_path_local_only,
        anchor.to_str().unwrap()
    );
    assert_eq!(snapshot(&fixture.root), before);
}

// ---------------------------------------------------------------------------
// 7. malformed receipts
// ---------------------------------------------------------------------------
#[tokio::test]
async fn malformed_restore_receipts_are_quarantined() {
    let fixture = Fixture::new().await;
    let original = fixture.receipt_text();
    assert!(original.contains("\"isolated\": false"));
    assert!(original.contains("\"receipt_scope\": \"local_only_not_for_export\""));
    let value: Value = serde_json::from_str(&original).unwrap();
    let watermark = value["trusted_anchor"]["watermarks"]["n"].clone();
    let watermark_text = serde_json::to_string(&watermark).unwrap();

    let with = |edit: &dyn Fn(&mut Value)| {
        let mut changed = value.clone();
        edit(&mut changed);
        serde_json::to_vec(&changed).unwrap()
    };
    let mut cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "first-version receipt",
            br#"{"watermark_rows":1,"isolated":false}"#.to_vec(),
            "no schema_version",
        ),
        (
            "isolated",
            original
                .replace("\"isolated\": false", "\"isolated\": true")
                .into_bytes(),
            "isolated",
        ),
        (
            "unknown top-level key",
            with(&|v| {
                v["extra"] = json!(1);
            }),
            "does not match the v2 schema",
        ),
        (
            "unknown anchor key",
            with(&|v| {
                v["trusted_anchor"]["extra"] = json!(1);
            }),
            "does not match the v2 schema",
        ),
        (
            "duplicate top-level key",
            original
                .replacen(
                    "\"isolated\": false",
                    "\"isolated\": false, \"isolated\": false",
                    1,
                )
                .into_bytes(),
            "duplicate JSON key: isolated",
        ),
        (
            "duplicate watermark namespace",
            original
                .replacen(
                    "\"watermarks\": {",
                    &format!("\"watermarks\": {{\"n\": {watermark_text}, "),
                    1,
                )
                .into_bytes(),
            "duplicate JSON key: n",
        ),
        (
            "wrong scope",
            original
                .replace("local_only_not_for_export", "for_export")
                .into_bytes(),
            "scope is",
        ),
        (
            "other schema version",
            original
                .replace("rsia.restore_receipt.v2", "rsia.restore_receipt.v1")
                .into_bytes(),
            "unsupported restore receipt schema_version",
        ),
        (
            "uppercase hex",
            with(&|v| {
                let digest = v["backup_manifest_sha256"].as_str().unwrap().to_uppercase();
                v["backup_manifest_sha256"] = json!(digest);
            }),
            "not 64 lowercase hex",
        ),
        (
            "short hex",
            with(&|v| {
                v["trusted_anchor"]["control_plane_digest"] = json!("abc");
            }),
            "not 64 lowercase hex",
        ),
        (
            "no watermark",
            with(&|v| {
                v["trusted_anchor"]["watermarks"] = json!({});
            }),
            "no revoke watermark",
        ),
        (
            "watermark that differs from the data directory",
            with(&|v| {
                v["trusted_anchor"]["watermarks"]["n"]["seq"] = json!(5);
            }),
            "data directory watermarks differ from the restore receipt",
        ),
        (
            "watermark digest that differs from the data directory",
            with(&|v| {
                v["trusted_anchor"]["watermarks"]["n"]["digest"] = json!("a".repeat(64));
            }),
            "data directory watermarks differ from the restore receipt",
        ),
        (
            "extra namespace in the receipt",
            with(&|v| {
                v["trusted_anchor"]["watermarks"]["m"] =
                    json!({"seq": 1, "digest": "b".repeat(64)});
            }),
            "data directory watermarks differ from the restore receipt",
        ),
        ("not UTF-8", vec![0xFF, 0xFE, b'{', b'}'], "not valid UTF-8"),
        ("empty", Vec::new(), "not strict JSON"),
        (
            "trailing content",
            [original.as_bytes(), b"x"].concat(),
            "not strict JSON",
        ),
        ("not an object", b"[]".to_vec(), "not a JSON object"),
    ];
    let mut oversized = original.clone().into_bytes();
    oversized.extend(std::iter::repeat_n(b' ', 64 * 1024));
    cases.push(("larger than 64 KiB", oversized, "larger than"));

    for (name, bytes, needle) in cases {
        std::fs::write(fixture.receipt_path(), &bytes).unwrap();
        let text = expect_quarantine(&fixture.anchored(), &fixture.restored, needle).await;
        assert!(!text.is_empty(), "{name}");
        assert!(!fixture.admission_path().exists(), "{name}");
    }

    // a symlink to an otherwise perfect receipt
    let elsewhere = fixture.root.join("elsewhere.json");
    std::fs::write(&elsewhere, original.as_bytes()).unwrap();
    std::fs::remove_file(fixture.receipt_path()).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&elsewhere, fixture.receipt_path()).unwrap();
        expect_quarantine(
            &fixture.anchored(),
            &fixture.restored,
            "must not be a symlink",
        )
        .await;
        expect_quarantine(
            &fixture.unanchored(),
            &fixture.restored,
            "must not be a symlink",
        )
        .await;
        std::fs::remove_file(fixture.receipt_path()).unwrap();
    }
    // a directory where the receipt should be
    std::fs::create_dir(fixture.receipt_path()).unwrap();
    expect_quarantine(&fixture.anchored(), &fixture.restored, "not a regular file").await;
    std::fs::remove_dir(fixture.receipt_path()).unwrap();

    // the genuine receipt verifies again
    std::fs::write(fixture.receipt_path(), original.as_bytes()).unwrap();
    assert_eq!(
        fixture.anchored().evaluate().await.unwrap().posture(),
        RecoveryPosture::RestoredVerified
    );
}

// ---------------------------------------------------------------------------
// 8. the restored directory was changed after the restore
// ---------------------------------------------------------------------------
#[tokio::test]
async fn a_restored_directory_altered_after_the_restore_is_quarantined() {
    let fixture = Fixture::new().await;
    let pristine = fixture.root.join("pristine.sqlite3");
    std::fs::copy(&fixture.data, &pristine).unwrap();
    let reset = || std::fs::copy(&pristine, &fixture.data).unwrap();
    let gate = fixture.anchored();
    // sanity: the copy verifies, so every failure below is the alteration
    assert_eq!(
        gate.evaluate().await.unwrap().posture(),
        RecoveryPosture::RestoredVerified
    );

    // a revocation tombstone deleted: the revoked run would come back
    sql(
        &fixture.data,
        "DELETE FROM objects WHERE namespace='n' AND kind='tombstone' AND id='run-2'",
    );
    let reason = expect_quarantine(&gate, &fixture.restored, "revocation tombstones differ").await;
    assert!(
        reason.contains("only in the trusted anchor: n/run-2"),
        "{reason}"
    );
    reset();

    // the watermark rolled back to the backup's
    sql(
        &fixture.data,
        "UPDATE revoke_watermark SET seq=1 WHERE namespace='n'",
    );
    expect_quarantine(&gate, &fixture.restored, "advanced since restore").await;
    reset();

    // a namespace the anchor does not know, and watermarks that disagree with it
    sql(
        &fixture.data,
        &format!(
            "INSERT INTO revoke_watermark(namespace,seq,digest) VALUES('z',1,'{}')",
            "d".repeat(64)
        ),
    );
    expect_quarantine(
        &gate,
        &fixture.restored,
        "namespace(s) the trusted anchor does not know: z",
    )
    .await;
    reset();
    sql(
        &fixture.data,
        "UPDATE revoke_watermark SET seq=9 WHERE namespace='n'",
    );
    expect_quarantine(
        &gate,
        &fixture.restored,
        "ahead of the trusted anchor (namespace n: seq 9 vs 2)",
    )
    .await;
    reset();
    sql(
        &fixture.data,
        &format!(
            "UPDATE revoke_watermark SET digest='{}' WHERE namespace='n'",
            "e".repeat(64)
        ),
    );
    expect_quarantine(
        &gate,
        &fixture.restored,
        "watermark digest differs at seq 2 (namespace n)",
    )
    .await;
    reset();
    // no watermark at all: nothing proves which revocations were applied
    sql(&fixture.data, "DELETE FROM revoke_watermark");
    expect_quarantine(&gate, &fixture.restored, "no revoke watermark").await;
    reset();

    // a root budget spend rolled back
    sql(
        &fixture.data,
        "UPDATE root_budgets SET spent_micros=spent_micros-1 WHERE billing_scope='scope-a'",
    );
    expect_quarantine(&gate, &fixture.restored, "root budget tables differ").await;
    reset();

    // an exposure-ledger fact removed
    sql(&fixture.data, "DELETE FROM objects WHERE id='ledger-1'");
    let reason =
        expect_quarantine(&gate, &fixture.restored, "consumed accounting facts differ").await;
    assert!(reason.contains("n/artifact/ledger-1"), "{reason}");
    reset();

    // a consumed fact edited in place
    sql(
        &fixture.data,
        "UPDATE objects SET body='{\"id\":\"res-1\",\"state\":\"released\"}' WHERE id='res-1'",
    );
    let reason =
        expect_quarantine(&gate, &fixture.restored, "consumed accounting facts differ").await;
    assert!(reason.contains("changed: n/reservation/res-1"), "{reason}");
    reset();

    // a fact the anchor never had
    sql(
        &fixture.data,
        "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('n','receipt','forged','admin','{\"id\":\"forged\"}')",
    );
    let reason =
        expect_quarantine(&gate, &fixture.restored, "consumed accounting facts differ").await;
    assert!(
        reason.contains("only in the data directory: n/receipt/forged"),
        "{reason}"
    );
    reset();

    // the pristine copy is accepted again
    assert_eq!(
        gate.evaluate().await.unwrap().posture(),
        RecoveryPosture::RestoredVerified
    );
}

// ---------------------------------------------------------------------------
// 9. migration checksums
// ---------------------------------------------------------------------------
#[tokio::test]
async fn a_migration_checksum_change_in_either_database_is_quarantined() {
    let fixture = Fixture::new().await;
    let pristine = fixture.root.join("pristine.sqlite3");
    std::fs::copy(&fixture.data, &pristine).unwrap();

    sql(
        &fixture.data,
        "UPDATE _sqlx_migrations SET checksum=zeroblob(48) WHERE version=1",
    );
    expect_quarantine(
        &fixture.anchored(),
        &fixture.restored,
        "data directory database: migration set is not the compiled-in set",
    )
    .await;
    std::fs::copy(&pristine, &fixture.data).unwrap();

    // a migration missing from the data directory
    sql(
        &fixture.data,
        "DELETE FROM _sqlx_migrations WHERE version=6",
    );
    expect_quarantine(
        &fixture.anchored(),
        &fixture.restored,
        "data directory database: migration set is not the compiled-in set",
    )
    .await;
    std::fs::copy(&pristine, &fixture.data).unwrap();

    // a migration that did not succeed
    sql(
        &fixture.data,
        "UPDATE _sqlx_migrations SET success=0 WHERE version=5",
    );
    expect_quarantine(
        &fixture.anchored(),
        &fixture.restored,
        "data directory database: migration set is not the compiled-in set",
    )
    .await;
    std::fs::copy(&pristine, &fixture.data).unwrap();

    // the anchor's checksum changed
    let anchor = fixture.independent_anchor("tampered-anchor");
    sql(
        &anchor,
        "UPDATE _sqlx_migrations SET checksum=zeroblob(48) WHERE version=2",
    );
    expect_quarantine(
        &fixture.with_anchor(&anchor),
        &fixture.restored,
        "trusted anchor database: migration set is not the compiled-in set",
    )
    .await;

    // a database that is not an RSIA database at all
    let foreign = fixture.root.join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    let foreign = foreign.join("rsia.sqlite3");
    sql(&foreign, "CREATE TABLE t(x)");
    expect_quarantine(
        &fixture.with_anchor(&foreign),
        &fixture.restored,
        "trusted anchor database:",
    )
    .await;
}

// ---------------------------------------------------------------------------
// 10. the anchor is only for a restored directory that has not been admitted
// ---------------------------------------------------------------------------
#[tokio::test]
async fn an_anchor_is_rejected_unless_the_directory_is_restored_and_unadmitted() {
    // never restored
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("rsia.sqlite3");
    Store::open(&data).await.unwrap().close().await;
    let anchor = Path::new("/anchor/rsia.sqlite3");
    expect_rejected(&StartupGate::new(&data, Some(anchor)), dir.path()).await;

    // a directory that does not even exist
    let missing = dir.path().join("missing").join("rsia.sqlite3");
    let error = StartupGate::new(&missing, Some(anchor))
        .evaluate()
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), ANCHOR_WITHOUT_RESTORE);
    assert!(!dir.path().join("missing").exists());

    // restored and already admitted
    let fixture = Fixture::new().await;
    fixture.admit().await;
    expect_rejected(&fixture.anchored(), &fixture.restored).await;
    expect_rejected(
        &fixture.with_anchor(&fixture.independent_anchor("other")),
        &fixture.restored,
    )
    .await;
    // and it still starts without one
    assert_eq!(
        fixture.unanchored().evaluate().await.unwrap().posture(),
        RecoveryPosture::AdmittedPreviously
    );
}

// ---------------------------------------------------------------------------
// 11. admission records that do not match the receipt
// ---------------------------------------------------------------------------
#[tokio::test]
async fn an_admission_record_that_does_not_match_the_receipt_quarantines_the_directory() {
    let fixture = Fixture::new().await;
    fixture.admit().await;
    let admission = std::fs::read_to_string(fixture.admission_path()).unwrap();
    let record: Value = serde_json::from_str(&admission).unwrap();
    let receipt = fixture.receipt_bytes();
    let with = |edit: &dyn Fn(&mut Value)| {
        let mut changed = record.clone();
        edit(&mut changed);
        serde_json::to_vec(&changed).unwrap()
    };
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "bound to another receipt",
            with(&|v| v["receipt_sha256"] = json!("c".repeat(64))),
            "bound to a different restore receipt",
        ),
        ("unparsable", b"{".to_vec(), "not strict JSON"),
        ("empty", Vec::new(), "not strict JSON"),
        ("not UTF-8", vec![0xFF, 0xFF], "not valid UTF-8"),
        (
            "unknown field",
            with(&|v| v["extra"] = json!(true)),
            "does not match its schema",
        ),
        (
            "missing field",
            with(&|v| {
                v.as_object_mut().unwrap().remove("scope");
            }),
            "does not match its schema",
        ),
        (
            "duplicate key",
            admission
                .replacen("\"scope\"", "\"scope\": \"x\", \"scope\"", 1)
                .into_bytes(),
            "duplicate JSON key: scope",
        ),
        (
            "other schema",
            with(&|v| v["schema_version"] = json!("rsia.restore_admission.v0")),
            "malformed",
        ),
        (
            "other scope",
            with(&|v| v["scope"] = json!("exportable")),
            "malformed",
        ),
        (
            "short digest",
            with(&|v| v["verified_facts_digest"] = json!("abc")),
            "malformed",
        ),
        (
            "no namespaces",
            with(&|v| v["namespaces"] = json!([])),
            "malformed",
        ),
        (
            "unsorted namespaces",
            with(&|v| v["namespaces"] = json!(["z", "a"])),
            "malformed",
        ),
        (
            "negative time",
            with(&|v| v["admitted_at"] = json!(-1)),
            "malformed",
        ),
    ];
    for (name, bytes, needle) in cases {
        std::fs::write(fixture.admission_path(), &bytes).unwrap();
        let text = expect_quarantine(&fixture.unanchored(), &fixture.restored, needle).await;
        assert!(!text.is_empty(), "{name}");
        // an anchor does not turn a bad admission into an admission
        expect_quarantine(&fixture.anchored(), &fixture.restored, needle).await;
    }

    // a symlinked admission record
    let elsewhere = fixture.root.join("admission-elsewhere.json");
    std::fs::write(&elsewhere, admission.as_bytes()).unwrap();
    std::fs::remove_file(fixture.admission_path()).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&elsewhere, fixture.admission_path()).unwrap();
        expect_quarantine(
            &fixture.unanchored(),
            &fixture.restored,
            "must not be a symlink",
        )
        .await;
        std::fs::remove_file(fixture.admission_path()).unwrap();
    }

    // a valid record, but the receipt it covered was replaced
    std::fs::write(fixture.admission_path(), admission.as_bytes()).unwrap();
    assert_eq!(
        fixture.unanchored().evaluate().await.unwrap().posture(),
        RecoveryPosture::AdmittedPreviously
    );
    std::fs::write(fixture.receipt_path(), [receipt.as_slice(), b" "].concat()).unwrap();
    expect_quarantine(
        &fixture.unanchored(),
        &fixture.restored,
        "bound to a different restore receipt",
    )
    .await;
    std::fs::write(fixture.receipt_path(), &receipt).unwrap();
    assert_eq!(
        fixture.unanchored().evaluate().await.unwrap().posture(),
        RecoveryPosture::AdmittedPreviously
    );
}

#[tokio::test]
async fn without_a_receipt_the_directory_is_normal_even_beside_an_admission_record() {
    // Known boundary: the gate cannot tell a directory that was never restored
    // from one whose receipt an administrator deleted.
    let fixture = Fixture::new().await;
    fixture.admit().await;
    std::fs::remove_file(fixture.receipt_path()).unwrap();
    let decision = fixture.unanchored().evaluate().await.unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::Normal);
}

// ---------------------------------------------------------------------------
// 12. zero write on every failure path
// ---------------------------------------------------------------------------
#[tokio::test]
async fn failure_paths_leave_the_data_directory_byte_identical() {
    let fixture = Fixture::new().await;
    let pristine = fixture.root.join("pristine.sqlite3");
    std::fs::copy(&fixture.data, &pristine).unwrap();

    // Each scenario runs against an unchanged restored directory, and the
    // snapshot (file list and sha256 of every file) must not move.
    let scenarios: Vec<(&str, StartupGate)> = vec![
        ("no anchor", fixture.unanchored()),
        (
            "missing anchor",
            fixture.with_anchor(&fixture.root.join("missing.sqlite3")),
        ),
        (
            "anchor is the data file",
            fixture.with_anchor(&fixture.data),
        ),
        (
            "relative anchor",
            fixture.with_anchor(Path::new("rsia.sqlite3")),
        ),
    ];
    for (name, gate) in &scenarios {
        let before = snapshot(&fixture.restored);
        assert!(gate.evaluate().await.is_err(), "{name}");
        assert_eq!(snapshot(&fixture.restored), before, "{name}");
    }

    // Failures that only surface after both databases were opened and compared.
    sql(
        &fixture.live,
        "UPDATE root_budgets SET spent_micros=spent_micros+1",
    );
    let before = snapshot(&fixture.restored);
    for _ in 0..3 {
        assert!(fixture.anchored().evaluate().await.is_err());
        assert_eq!(snapshot(&fixture.restored), before, "compared facts differ");
    }
    sql(
        &fixture.live,
        "UPDATE root_budgets SET spent_micros=spent_micros-1",
    );
    sql(&fixture.data, "DELETE FROM objects WHERE kind='tombstone'");
    let before = snapshot(&fixture.restored);
    assert!(fixture.anchored().evaluate().await.is_err());
    assert_eq!(snapshot(&fixture.restored), before, "tombstone deleted");

    // A restored directory whose database was opened by the service before
    // (WAL-mode header, no side files) is read without creating them.
    std::fs::copy(&pristine, &fixture.data).unwrap();
    Store::open(&fixture.data).await.unwrap().close().await;
    sql(
        &fixture.live,
        "UPDATE root_budgets SET spent_micros=spent_micros+1",
    );
    let before = snapshot(&fixture.restored);
    assert!(fixture.anchored().evaluate().await.is_err());
    assert_eq!(snapshot(&fixture.restored), before, "WAL-mode database");
    assert!(
        !fixture.restored.join(".rsia.lock").exists(),
        "the gate never takes the data lock"
    );
}

// ---------------------------------------------------------------------------
// 13. drift guard against scripts/restore_backup.py
// ---------------------------------------------------------------------------
fn python_tuple(source: &str, marker: &str) -> Vec<String> {
    assert_eq!(
        source.matches(marker).count(),
        1,
        "{marker:?} must appear exactly once in restore_backup.py"
    );
    let rest = &source[source.find(marker).unwrap() + marker.len()..];
    let body = &rest[..rest.find(')').expect("closing parenthesis")];
    let mut out = Vec::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            out.push(chars.by_ref().take_while(|c| *c != '"').collect());
        }
    }
    out
}

#[test]
fn protected_fact_selectors_match_restore_backup_py() {
    let script = std::fs::read_to_string(RESTORE_SCRIPT).unwrap();
    let protected = &script[script.find("def protected_facts").unwrap()..];
    let protected = &protected[..protected.find("def verify_delta").unwrap()];

    let mut kinds = python_tuple(protected, "if kind in (");
    let mut schemas = python_tuple(protected, "schema in (");
    let mut rust_kinds: Vec<String> = PROTECTED_OBJECT_KINDS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let mut rust_schemas: Vec<String> = PROTECTED_SCHEMA_VERSIONS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert!(!kinds.is_empty() && !schemas.is_empty());
    kinds.sort();
    schemas.sort();
    rust_kinds.sort();
    rust_schemas.sort();
    assert_eq!(rust_kinds, kinds, "protected object kinds drifted");
    assert_eq!(rust_schemas, schemas, "protected schema versions drifted");
    let mut deduped = schemas.clone();
    deduped.dedup();
    assert_eq!(deduped, schemas, "duplicate schema in the script");

    // the root budget tables are selected by the very same SQL text
    assert!(
        protected.contains(ROOT_BUDGET_TABLE_QUERY),
        "root budget table selector drifted"
    );
    // and the tombstone / watermark / receipt vocabulary the gate relies on
    assert!(script.contains("kind='tombstone'"));
    assert!(script.contains("SELECT namespace,seq,digest FROM revoke_watermark"));
    assert!(script.contains(&format!("\"schema_version\":\"{RESTORE_RECEIPT_SCHEMA}\"")));
    assert!(script.contains(&format!("\"receipt_scope\":\"{RECEIPT_SCOPE}\"")));
    assert!(script.contains(&format!("(temp/\"{RESTORE_RECEIPT_FILE}\")")));
}

// ---------------------------------------------------------------------------
// deployment gate: code execution stays disabled
// ---------------------------------------------------------------------------
#[tokio::test]
async fn every_posture_reports_code_execution_disabled_and_no_sandbox() {
    let fixture = Fixture::new().await;
    let normal_dir = tempfile::tempdir().unwrap();
    let normal = StartupGate::new(&normal_dir.path().join("rsia.sqlite3"), None)
        .evaluate()
        .await
        .unwrap();
    let verified = fixture.anchored().evaluate().await.unwrap();
    verified.admit().unwrap();
    let admitted = fixture.unanchored().evaluate().await.unwrap();
    for (decision, posture) in [
        (&normal, "normal"),
        (&verified, "restored_verified"),
        (&admitted, "admitted_previously"),
    ] {
        assert_eq!(
            decision.startup_line(),
            format!("rsia startup: code_execution=disabled sandbox=unavailable recovery={posture}")
        );
        assert_eq!(decision.isolation().code_execution, "disabled");
        assert!(!decision.isolation().sandbox_available);
        assert!(!decision.isolation().allow_network);
        assert_eq!(
            decision
                .isolation()
                .run_shell("true")
                .unwrap_err()
                .to_string(),
            "invalid input: sandbox_unavailable",
            "no host shell fallback"
        );
        assert!(decision.isolation().execute_code(b"x").is_err());
    }
    let config = deployment_config(Some(&fixture.live));
    assert!(
        !config.allow_code_execution && !config.sandbox_enabled && !config.allow_external_network
    );
}
