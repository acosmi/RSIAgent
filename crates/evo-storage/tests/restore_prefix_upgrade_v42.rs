//! E08 / plan §11 (AG-104): a consistent backup taken on a strict prefix of the
//! migration set (0001-0005 or 0001-0006) is restored into a new directory, and
//! `scripts/restore_backup.py` applies the missing migrations to its private copy
//! before the anchor comparison, the delta replay and publication.
//!
//! The historical databases are written by the real sqlx migrator (the compiled
//! `sqlx::migrate!` set with its later versions removed, as in
//! `formal_parent_kind_migration_v42.rs`), and the backups are built the way
//! `Store::backup` builds them: `VACUUM INTO` plus an `rsia.backup.v2` manifest
//! read from that database. The restored database must equal the same backup
//! database migrated by `Store::open` in its schema SQL, its `_sqlx_migrations`
//! version, description, success and checksum, and every row of every table
//! (`installed_on` and `execution_time` aside), and it must pass
//! `read_control_plane_facts` with the same facts as the trusted anchor, which is
//! what the startup gate compares. The anchor and the gate stay on the exact
//! compiled set; every other migration ledger is still isolated.

use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::lifecycle::{
    BACKUP_SCHEMA, BackupFileEntry, BackupManifest, BackupMigrationEntry, BackupWatermarkEntry,
    CleanupState, ControlPlaneFacts, LifecycleStore, RevokeTombstone, TypedObjectRef,
    parse_strict_json, read_control_plane_facts,
};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection, migrate::Migrator, sqlite::SqliteConnectOptions};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RESTORE_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/restore_backup.py"
);
const CURRENT_VERSIONS: [i64; 6] = [1, 2, 4, 5, 6, 7];
const LEGACY_KINDS: [&str; 14] = [
    "run",
    "feedback",
    "event",
    "candidate",
    "release",
    "pointer",
    "receipt",
    "dataset",
    "evaluation",
    "budget",
    "reservation",
    "job",
    "improvement",
    "artifact",
];
const NOT_A_PREFIX: &str =
    "unsupported manifest migrations: not a prefix of the current migration set";
const PREDATES: &str =
    "backup predates the root budget ledger (0005); protected facts cannot be verified";
const RECEIPT_KEYS: [&str; 6] = [
    "backup_manifest_sha256",
    "isolated",
    "receipt_scope",
    "replayed_revoke_events",
    "schema_version",
    "trusted_anchor",
];
const ANCHOR_KEYS: [&str; 5] = [
    "control_plane_digest",
    "path_local_only",
    "scope",
    "verified_at",
    "watermarks",
];
const BLOB: &[u8] = b"legacy blob bytes";

type SchemaRow = (String, String, String, Option<String>);
type QuotedLedgerRow = (String, String, String, String);
type TimingRow = (i64, String, String, i64, String);
type TableRows = (String, Vec<Vec<String>>);

/// Everything the equivalence compares: no page layout, no timing columns.
#[derive(Debug, PartialEq, Eq)]
struct Image {
    schema: Vec<SchemaRow>,
    ledger: Vec<QuotedLedgerRow>,
    tables: Vec<TableRows>,
}

fn admin(namespace: &str) -> Context {
    Context::new(namespace, "admin", Role::Admin).unwrap()
}

fn digest_of(text: &str) -> String {
    hash(text.as_bytes())
}

fn private_directory(path: &std::path::Path) {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.recursive(true).create(path).unwrap();
}

fn private_member(path: &std::path::Path, bytes: &[u8]) {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).unwrap().write_all(bytes).unwrap();
}

async fn connect(path: &Path, create: bool) -> SqliteConnection {
    if create && !path.try_exists().unwrap() {
        let mut file = std::fs::OpenOptions::new();
        file.create_new(true).write(true);
        #[cfg(unix)]
        file.mode(0o600);
        drop(file.open(path).unwrap());
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .foreign_keys(true);
    SqliteConnection::connect_with(&options).await.unwrap()
}

/// The compiled migrations restricted to `versions`: the actual SQL, descriptions
/// and checksums, so running it writes a genuine historical sqlx ledger.
fn migrator(versions: &[i64]) -> Migrator {
    let mut migrations = sqlx::migrate!("./migrations");
    migrations
        .migrations
        .to_mut()
        .retain(|migration| versions.contains(&migration.version));
    assert_eq!(
        migrations.iter().map(|m| m.version).collect::<Vec<_>>(),
        versions
    );
    migrations
}

fn quoted_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn tombstone_body(id: &str, seq: i64, digest: &str, reason: &str, created_at: i64) -> String {
    json!({
        "id": id,
        "schema_version": "rsia.revoke_tombstone.v1",
        "source_kind": "run",
        "reason": reason,
        "watermark_seq": seq,
        "watermark_digest": digest,
        "created_at": created_at,
    })
    .to_string()
}

/// Facts of every kind the restore and the gate read, in two watermarked
/// namespaces, with explicitly gapped rowids and nondefault revisions.
async fn seed(connection: &mut SqliteConnection, versions: &[i64]) {
    for (index, kind) in LEGACY_KINDS.iter().enumerate() {
        let id = if *kind == "run" { "run-1" } else { "shared" };
        sqlx::query(
            "INSERT INTO objects(rowid,namespace,kind,id,owner,body,revision) VALUES(?,?,?,?,?,?,?)",
        )
        .bind(100 + index as i64 * 17)
        .bind("legacy-a")
        .bind(*kind)
        .bind(id)
        .bind(format!("owner-{index}"))
        .bind(format!(
            " {{ \"id\" : \"{id}\", \"kind\" : \"{kind}\", \"state\" : \"queued\" }} "
        ))
        .bind(index as i64 + 2)
        .execute(&mut *connection)
        .await
        .unwrap();
    }
    let ledger = json!({
        "id": "ledger-1",
        "schema_version": "rsia.typed_artifact_envelope.v1",
        "record_kind": "exposure_ledger_v1",
        "payload": {"spent": 1},
    })
    .to_string();
    let earlier = tombstone_body(
        "run-0",
        2,
        &digest_of("legacy-a-2"),
        "revoked before the backup",
        5,
    );
    for (rowid, namespace, kind, id, body, revision) in [
        (
            8,
            "legacy-a",
            "artifact",
            "first",
            " { \"id\" : \"first\", \"v\" : 1 } ".to_owned(),
            19,
        ),
        (900, "legacy-a", "artifact", "ledger-1", ledger, 23),
        (950, "legacy-a", "tombstone", "run-0", earlier, 1),
        (
            3,
            "legacy-b",
            "artifact",
            "shared",
            "{\"id\":\"shared\",\"v\":\"other namespace\"}".to_owned(),
            31,
        ),
        (
            970,
            "legacy-b",
            "evaluation",
            "eval-1",
            "{\"id\":\"eval-1\",\"score\":0.5}".to_owned(),
            7,
        ),
    ] {
        sqlx::query(
            "INSERT INTO objects(rowid,namespace,kind,id,owner,body,revision) VALUES(?,?,?,?,'admin',?,?)",
        )
        .bind(rowid)
        .bind(namespace)
        .bind(kind)
        .bind(id)
        .bind(body)
        .bind(revision)
        .execute(&mut *connection)
        .await
        .unwrap();
    }
    for (namespace, seq) in [("legacy-a", 3), ("legacy-b", 1)] {
        sqlx::query("INSERT INTO revoke_watermark VALUES(?,?,?)")
            .bind(namespace)
            .bind(seq)
            .bind(digest_of(&format!("{namespace}-{seq}")))
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    sqlx::raw_sql(
        r#"INSERT INTO idempotency VALUES('legacy-a','actor','operation','key','payload','shared',' { "ok" : true } ',0);
        INSERT INTO audit(seq,namespace,payload,previous_hash,digest) VALUES(27,'legacy-a',' { "event" : "old" } ','previous','digest');
        INSERT INTO dependencies VALUES('legacy-a','artifact','first','run','run-0');
        INSERT INTO replay_worlds VALUES('legacy-a','world',1,' { "sealed" : true } ');
        INSERT INTO skill_fts(namespace,candidate_id,tokens) VALUES('legacy-a','shared','old tokens');"#,
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    if versions.contains(&5) {
        sqlx::raw_sql(
            r#"INSERT INTO root_budgets(billing_scope,root_budget_id,authorizing_namespace,currency,pricing_version,payment_subject,authorization_receipt_digest,per_call_cap_micros,total_limit_micros,spent_micros,reserved_micros,created_at)
             VALUES('scope','root','legacy-a','USD','price','payer','authorization',100,1000,11,7,123);
            INSERT INTO root_budget_namespaces VALUES('scope','legacy-a');
            INSERT INTO root_budget_dispatch_groups(billing_scope,dispatch_group_id,owner_namespace,created_at) VALUES('scope','group','legacy-a',123);
            INSERT INTO root_budget_calls(billing_scope,call_id,dispatch_group_id,namespace,stage,actual_input_digest,reserved_micros,state,lease_token,lease_epoch,lease_until,created_at)
             VALUES('scope','call','group','legacy-a','optimization','input',7,'reserved','lease',3,300,123);
            INSERT INTO root_budget_events(seq,billing_scope,call_id,event_kind,event_at,details) VALUES(41,'scope','call','reserved',123,' { "old" : true } ');"#,
        )
        .execute(&mut *connection)
        .await
        .unwrap();
    }
    if versions.contains(&6) {
        sqlx::raw_sql(
            r#"INSERT INTO revoke_cleanup_jobs(namespace,job_id,source_kind,source_id,watermark_seq,watermark_digest,state,created_at,updated_at)
             VALUES('legacy-a','cleanup','run','run-0',2,'watermark','pending',123,124);
            INSERT INTO revoke_cleanup_frontier(seq,namespace,job_id,node_kind,node_id) VALUES(61,'legacy-a','cleanup','artifact','first');
            INSERT INTO revoke_cleanup_events(seq,namespace,job_id,node_kind,node_id,event_kind,event_at,details)
             VALUES(71,'legacy-a','cleanup','artifact','first','queued',124,' { "old" : true } ');"#,
        )
        .execute(&mut *connection)
        .await
        .unwrap();
    }
}

/// Schema SQL, the ledger as `quote()`d values without its timing columns, and
/// every table's rows (with the rowid where the table has one), sorted.
async fn image(path: &Path) -> Image {
    let mut connection = connect(path, false).await;
    let schema =
        sqlx::query_as("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    let ledger = sqlx::query_as(
        "SELECT quote(version),quote(description),quote(success),quote(checksum)
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name<>'_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    let mut tables = Vec::new();
    for table in names {
        let column_names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                .bind(&table)
                .fetch_all(&mut connection)
                .await
                .unwrap();
        let mut columns: Vec<String> = column_names
            .iter()
            .map(String::as_str)
            .map(quoted_identifier)
            .collect();
        // FTS5 shadow tables are WITHOUT ROWID; every other table keeps its rowid.
        if sqlx::query(&format!(
            "SELECT rowid FROM {} LIMIT 0",
            quoted_identifier(&table)
        ))
        .fetch_all(&mut connection)
        .await
        .is_ok()
        {
            columns.insert(0, "rowid".into());
        }
        let expressions = columns
            .iter()
            .map(|column| format!("quote({column})"))
            .collect::<Vec<_>>()
            .join(",");
        let rows = sqlx::query(&format!(
            "SELECT {expressions} FROM {}",
            quoted_identifier(&table)
        ))
        .fetch_all(&mut connection)
        .await
        .unwrap();
        let mut values: Vec<Vec<String>> = rows
            .into_iter()
            .map(|row| {
                (0..columns.len())
                    .map(|index| row.try_get::<String, _>(index).unwrap())
                    .collect()
            })
            .collect();
        values.sort();
        tables.push((table, values));
    }
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(integrity, "ok", "{}", path.display());
    connection.close().await.unwrap();
    Image {
        schema,
        ledger,
        tables,
    }
}

async fn timings(path: &Path) -> Vec<TimingRow> {
    let mut connection = connect(path, false).await;
    let rows = sqlx::query_as(
        "SELECT version,CAST(installed_on AS TEXT),typeof(installed_on),execution_time,typeof(execution_time)
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    rows
}

/// The ledger equals what `sqlx::migrate!` compiled: version, description,
/// success and checksum, row for row.
async fn assert_ledger_is_the_compiled_set(path: &Path) {
    let mut connection = connect(path, false).await;
    let rows: Vec<(i64, String, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT version,description,success,checksum FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    let compiled = sqlx::migrate!("./migrations");
    assert_eq!(
        rows.iter().map(|row| row.0).collect::<Vec<_>>(),
        CURRENT_VERSIONS
    );
    for (version, description, success, checksum) in rows {
        let migration = compiled.iter().find(|m| m.version == version).unwrap();
        assert_eq!(description, migration.description.as_ref(), "{version}");
        assert!(success, "{version}");
        assert_eq!(
            checksum.as_slice(),
            migration.checksum.as_ref(),
            "{version}"
        );
    }
}

struct Scenario {
    _guard: tempfile::TempDir,
    root: PathBuf,
    /// The historical live database, its sqlx ledger on `versions`.
    history: PathBuf,
}

impl Scenario {
    async fn new(versions: &[i64]) -> Self {
        let guard = tempfile::tempdir().unwrap();
        // restore_backup.py refuses a symlink anywhere in a supplied path.
        let root = guard.path().canonicalize().unwrap();
        let history = root.join("history.sqlite3");
        let mut connection = connect(&history, true).await;
        migrator(versions).run(&mut connection).await.unwrap();
        seed(&mut connection, versions).await;
        connection.close().await.unwrap();
        Self {
            _guard: guard,
            root,
            history,
        }
    }

    /// A copy of `source` in its own directory, migrated by `Store::open` when
    /// `upgrade` (the binary upgrade of a live control plane).
    async fn copy(&self, source: &Path, name: &str, upgrade: bool) -> PathBuf {
        let directory = self.root.join(name);
        private_directory(&directory);
        let path = directory.join("rsia.sqlite3");
        std::fs::copy(source, &path).unwrap();
        if upgrade {
            Store::open(&path).await.unwrap().close().await;
        }
        path
    }

    /// `VACUUM INTO`, one blob and a manifest read from the copied database, as
    /// `Store::backup` writes them. `db_sql` then changes the copied database
    /// (its digest is taken afterwards); `mutate` edits the manifest document.
    async fn backup(
        &self,
        name: &str,
        db_sql: Option<&str>,
        mutate: impl FnOnce(&mut Value),
    ) -> PathBuf {
        let directory = self.root.join(name);
        private_directory(&directory);
        let database = directory.join("rsia.sqlite3");
        private_member(&database, b"");
        let mut connection = connect(&self.history, false).await;
        sqlx::query("VACUUM INTO ?")
            .bind(database.to_str().unwrap())
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
        let mut connection = connect(&database, false).await;
        let migrations = sqlx::query(
            "SELECT version,description,checksum,success FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&mut connection)
        .await
        .unwrap()
        .into_iter()
        .map(|row| BackupMigrationEntry {
            version: row.get("version"),
            description: row.get("description"),
            checksum_hex: row
                .get::<Vec<u8>, _>("checksum")
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            success: row.get::<i64, _>("success") != 0,
        })
        .collect();
        let watermarks =
            sqlx::query("SELECT namespace,seq,digest FROM revoke_watermark ORDER BY namespace")
                .fetch_all(&mut connection)
                .await
                .unwrap()
                .into_iter()
                .map(|row| BackupWatermarkEntry {
                    namespace: row.get("namespace"),
                    seq: row.get("seq"),
                    digest: row.get("digest"),
                })
                .collect();
        if let Some(statement) = db_sql {
            sqlx::raw_sql(statement)
                .execute(&mut connection)
                .await
                .unwrap();
        }
        connection.close().await.unwrap();
        let blob_digest = hash(BLOB);
        private_directory(&directory.join("blobs/legacy-a"));
        private_member(&directory.join(format!("blobs/legacy-a/{blob_digest}")), BLOB);
        let bytes = std::fs::read(database).unwrap();
        let manifest = BackupManifest {
            schema_version: BACKUP_SCHEMA.into(),
            complete: true,
            database: BackupFileEntry {
                path: "rsia.sqlite3".into(),
                sha256: hash(&bytes),
                bytes: bytes.len() as u64,
            },
            blobs: vec![BackupFileEntry {
                path: format!("blobs/legacy-a/{blob_digest}"),
                sha256: blob_digest,
                bytes: BLOB.len() as u64,
            }],
            migrations,
            watermarks,
            created_at: 1_700_000_000,
        };
        let mut document = serde_json::to_value(&manifest).unwrap();
        mutate(&mut document);
        private_member(&directory.join("backup-manifest.json"), &serde_json::to_vec_pretty(&document).unwrap());
        directory
    }

    /// The exact revoke delta of `backup`; `events` and `latest` replace the
    /// tail of `legacy-a`.
    fn delta(
        &self,
        backup: &Path,
        name: &str,
        events: Option<(Vec<Value>, i64, String)>,
    ) -> PathBuf {
        let manifest_bytes = std::fs::read(backup.join("backup-manifest.json")).unwrap();
        let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
        let namespaces = manifest["watermarks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|mark| {
                let mut entry = json!({
                    "namespace": mark["namespace"],
                    "base_seq": mark["seq"],
                    "base_digest": mark["digest"],
                    "events": [],
                    "latest_seq": mark["seq"],
                    "latest_digest": mark["digest"],
                });
                if let Some((events, seq, digest)) = &events
                    && mark["namespace"] == "legacy-a"
                {
                    entry["events"] = json!(events);
                    entry["latest_seq"] = json!(seq);
                    entry["latest_digest"] = json!(digest);
                }
                entry
            })
            .collect::<Vec<_>>();
        let path = self.root.join(format!("{name}.json"));
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "schema_version": "rsia.revoke_delta.v1",
                "base_manifest_sha256": hash(&manifest_bytes),
                "namespaces": namespaces,
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    fn restore(&self, backup: &Path, anchor: &Path, delta: &Path, name: &str) -> (PathBuf, Output) {
        let dest = self.root.join(name);
        let output = Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg(RESTORE_SCRIPT)
            .arg("--backup")
            .arg(backup)
            .arg("--dest")
            .arg(&dest)
            .arg("--revoke-delta")
            .arg(delta)
            .arg("--trusted-revocations-db")
            .arg(anchor)
            .output()
            .unwrap();
        (dest, output)
    }

    fn assert_isolated(&self, dest: &Path, output: &Output, reason: &str) {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{reason}: {stdout} {stderr}");
        assert_eq!(stderr, format!("ISOLATE {reason}; not mounting\n"));
        assert!(stdout.is_empty(), "{stdout}");
        assert!(
            !dest.exists(),
            "{reason}: an isolated restore was published"
        );
        let staging = format!(".{}.restore-", dest.file_name().unwrap().to_string_lossy());
        for entry in std::fs::read_dir(&self.root).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(!name.starts_with(&staging), "staging left behind: {name}");
        }
    }
}

fn assert_restored(output: &Output, events: usize, applied: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stdout} {stderr}");
    assert_eq!(
        stdout,
        format!("RESTORE_OK events={events} migrations_applied={applied}\n"),
        "{stderr}"
    );
    assert!(stderr.is_empty(), "{stderr}");
}

/// What the startup gate verifies before it admits a restored directory: the
/// receipt has exactly the v2 fields, and the data directory's control-plane
/// facts, read without `Store::open` and on the exact compiled migration set,
/// equal the anchor's and the receipt's watermarks.
async fn assert_gate_facts_admit(restored: &Path, anchor: &Path) -> ControlPlaneFacts {
    let receipt_bytes = std::fs::read(restored.join("restore.json")).unwrap();
    let receipt = parse_strict_json(std::str::from_utf8(&receipt_bytes).unwrap()).unwrap();
    let mut keys: Vec<&str> = receipt
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, RECEIPT_KEYS);
    let mut anchor_keys: Vec<&str> = receipt["trusted_anchor"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    anchor_keys.sort_unstable();
    assert_eq!(anchor_keys, ANCHOR_KEYS);
    assert_eq!(receipt["schema_version"], "rsia.restore_receipt.v2");
    assert_eq!(receipt["isolated"], json!(false));
    assert_eq!(receipt["receipt_scope"], "local_only_not_for_export");

    let data = read_control_plane_facts(&restored.join("rsia.sqlite3"))
        .await
        .unwrap();
    let trusted = read_control_plane_facts(anchor).await.unwrap();
    assert_eq!(data, trusted);
    assert!(!data.watermarks.is_empty());
    for (namespace, mark) in &data.watermarks {
        let recorded = &receipt["trusted_anchor"]["watermarks"][namespace];
        assert_eq!(recorded["seq"], json!(mark.seq), "{namespace}");
        assert_eq!(recorded["digest"], json!(mark.digest), "{namespace}");
    }
    assert_eq!(
        receipt["trusted_anchor"]["watermarks"]
            .as_object()
            .unwrap()
            .len(),
        data.watermarks.len()
    );
    data
}

/// The seeded scenarios hold protected objects, root budget rows and tombstones
/// in both namespaces, so the facts the gate compares are not vacuous.
fn assert_seeded_facts(facts: &ControlPlaneFacts) {
    assert_eq!(
        facts
            .watermarks
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["legacy-a", "legacy-b"]
    );
    assert!(
        facts
            .protected_objects
            .keys()
            .any(|(namespace, kind, _)| namespace == "legacy-a" && kind == "budget")
    );
    assert!(
        facts
            .protected_objects
            .keys()
            .any(|(namespace, _, id)| namespace == "legacy-a" && id == "ledger-1")
    );
    assert!(
        facts
            .protected_objects
            .keys()
            .any(|(namespace, kind, _)| namespace == "legacy-b" && kind == "evaluation")
    );
    assert!(
        facts
            .root_budget_tables
            .get("root_budgets")
            .is_some_and(|rows| rows.len() == 1)
    );
    assert!(
        facts
            .tombstones
            .contains_key(&("legacy-a".to_owned(), "run-0".to_owned()))
    );
}

fn migrations_record(restored: &Path) -> Value {
    let text = std::fs::read_to_string(restored.join("restore.migrations.json")).unwrap();
    assert!(text.ends_with('\n'));
    let value = parse_strict_json(&text).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["applied", "current_versions", "schema", "source_versions"]
    );
    // on disk the fields follow the documented order
    let positions: Vec<usize> = ["schema", "source_versions", "applied", "current_versions"]
        .iter()
        .map(|key| text.find(&format!("\"{key}\":")).unwrap())
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    value
}

fn checksum_hex(version: i64) -> String {
    let compiled = sqlx::migrate!("./migrations");
    let migration = compiled.iter().find(|m| m.version == version).unwrap();
    migration
        .checksum
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A prefix backup restored without later revocations: the script's upgrade and
/// `Store::open` on the same backup database agree on everything compared.
async fn upgrade_matches_store_open(versions: &[i64], applied: &[i64]) {
    let scenario = Scenario::new(versions).await;
    let backup = scenario.backup("backup", None, |_| {}).await;
    let backup_timings = timings(&backup.join("rsia.sqlite3")).await;
    assert_eq!(
        backup_timings.iter().map(|row| row.0).collect::<Vec<_>>(),
        versions
    );
    // The live control plane after the binary upgrade, and the same backup
    // database migrated by Store::open.
    let anchor = scenario.copy(&scenario.history, "live", true).await;
    let reference = scenario
        .copy(&backup.join("rsia.sqlite3"), "store-open", true)
        .await;
    let delta = scenario.delta(&backup, "delta", None);
    let (restored, output) = scenario.restore(&backup, &anchor, &delta, "restored");
    let applied_text = applied
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    assert_restored(&output, 0, &applied_text);

    let mut names: Vec<String> = std::fs::read_dir(&restored)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "blobs",
            "restore.json",
            "restore.migrations.json",
            "rsia.sqlite3"
        ]
    );
    assert_eq!(
        std::fs::read(restored.join(format!("blobs/legacy-a/{}", hash(BLOB)))).unwrap(),
        BLOB
    );
    let record = migrations_record(&restored);
    assert_eq!(
        record,
        json!({
            "schema": "rsia.restore_migrations.v1",
            "source_versions": versions,
            "applied": applied
                .iter()
                .map(|version| json!({"version": version, "checksum_sha384": checksum_hex(*version)}))
                .collect::<Vec<_>>(),
            "current_versions": CURRENT_VERSIONS,
        })
    );

    // Equivalence with Store::open: schema SQL, ledger, every row.
    let restored_db = restored.join("rsia.sqlite3");
    let upgraded = image(&restored_db).await;
    let migrated = image(&reference).await;
    assert_eq!(upgraded.schema, migrated.schema);
    assert_eq!(upgraded.ledger, migrated.ledger);
    assert_eq!(
        upgraded.tables.iter().map(|t| &t.0).collect::<Vec<_>>(),
        migrated.tables.iter().map(|t| &t.0).collect::<Vec<_>>()
    );
    for (left, right) in upgraded.tables.iter().zip(&migrated.tables) {
        assert_eq!(left, right, "table {}", left.0);
    }
    assert_eq!(upgraded, migrated);
    assert_eq!(upgraded, image(&anchor).await);
    assert!(
        upgraded
            .tables
            .iter()
            .any(|(name, rows)| name == "objects" && rows.len() == LEGACY_KINDS.len() + 5)
    );
    assert_ledger_is_the_compiled_set(&restored_db).await;
    assert_ledger_is_the_compiled_set(&reference).await;

    // The backup's ledger rows are kept as they were; the new rows carry the
    // column types sqlx writes and a measured execution time.
    let restored_timings = timings(&restored_db).await;
    assert_eq!(
        &restored_timings[..versions.len()],
        backup_timings.as_slice()
    );
    for row in &restored_timings[versions.len()..] {
        assert!(applied.contains(&row.0));
        assert_eq!((row.2.as_str(), row.4.as_str()), ("text", "integer"));
        assert!(row.3 >= 0, "{row:?}");
    }

    // The restored directory passes what the startup gate reads.
    let facts = assert_gate_facts_admit(&restored, &anchor).await;
    assert_seeded_facts(&facts);
    assert_eq!(
        read_control_plane_facts(&restored_db).await.unwrap(),
        read_control_plane_facts(&reference).await.unwrap()
    );
    // And the product opens it without migrating anything further.
    let before = image(&restored_db).await;
    Store::open(&restored_db).await.unwrap().close().await;
    assert_eq!(image(&restored_db).await, before);
}

#[tokio::test]
async fn a_backup_on_0001_to_0006_is_upgraded_exactly_like_store_open() {
    upgrade_matches_store_open(&[1, 2, 4, 5, 6], &[7]).await;
}

#[tokio::test]
async fn a_backup_on_0001_to_0005_is_upgraded_exactly_like_store_open() {
    upgrade_matches_store_open(&[1, 2, 4, 5], &[6, 7]).await;
}

#[tokio::test]
async fn a_0005_backup_replays_a_revocation_made_after_the_binary_upgrade() {
    let versions = [1, 2, 4, 5];
    let scenario = Scenario::new(&versions).await;
    let backup = scenario.backup("backup", None, |_| {}).await;
    let manifest: BackupManifest =
        serde_json::from_slice(&std::fs::read(backup.join("backup-manifest.json")).unwrap())
            .unwrap();
    let base = manifest
        .watermarks
        .iter()
        .find(|mark| mark.namespace == "legacy-a")
        .unwrap()
        .clone();

    // The upgraded live control plane revokes a run after the backup; the
    // revocation tables it writes did not exist when the backup was taken.
    let anchor = scenario.copy(&scenario.history, "live", false).await;
    let store = Store::open(&anchor).await.unwrap();
    let context = admin("legacy-a");
    let revoked = LifecycleStore::begin_revoke(
        &context,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "run-1".into(),
        },
        "revoked after the upgrade",
        9,
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    let tombstone: RevokeTombstone = session.need(&context, "tombstone", "run-1").await.unwrap();
    session.commit().await.unwrap();
    store.close().await;
    let event = json!({
        "seq": base.seq + 1,
        "previous_digest": base.digest,
        "digest": revoked.watermark_digest,
        "source_kind": "run",
        "source_id": "run-1",
        "tombstone_digest": hash(&serde_json::to_vec(&tombstone).unwrap()),
        "reason": "revoked after the upgrade",
        "created_at": 9,
    });
    let delta = scenario.delta(
        &backup,
        "delta",
        Some((vec![event], base.seq + 1, revoked.watermark_digest.clone())),
    );
    let (restored, output) = scenario.restore(&backup, &anchor, &delta, "restored");
    assert_restored(&output, 1, "6,7");
    let record = migrations_record(&restored);
    assert_eq!(record["source_versions"], json!(versions));
    assert_eq!(record["current_versions"], json!(CURRENT_VERSIONS));
    let restored_db = restored.join("rsia.sqlite3");
    assert_ledger_is_the_compiled_set(&restored_db).await;
    let facts = assert_gate_facts_admit(&restored, &anchor).await;
    assert_seeded_facts(&facts);
    assert_eq!(
        facts.tombstones[&("legacy-a".to_owned(), "run-1".to_owned())]["watermark_seq"],
        json!(base.seq + 1)
    );

    let restored_store = Store::open(&restored_db).await.unwrap();
    let mut session = restored_store.session().await.unwrap();
    let replayed: RevokeTombstone = session.need(&context, "tombstone", "run-1").await.unwrap();
    assert_eq!(replayed.watermark_digest, revoked.watermark_digest);
    assert_eq!(
        session.watermark(&context).await.unwrap(),
        Some((base.seq + 1, revoked.watermark_digest.clone()))
    );
    session.commit().await.unwrap();
    let status = LifecycleStore::cleanup_status(&context, &restored_store, &revoked.job_id)
        .await
        .unwrap();
    assert_eq!(status.state, CleanupState::Pending);
    restored_store.close().await;
}

#[tokio::test]
async fn a_current_backup_applies_nothing_and_writes_no_migration_record() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let live = root.join("rsia.sqlite3");
    let store = Store::open(&live).await.unwrap();
    let context = admin("n");
    let mut session = store.session().await.unwrap();
    session
        .put(
            &context,
            "artifact",
            "a",
            context.actor(),
            &json!({"id":"a"}),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&context, &hash(b"initial-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let backup = root.join("backup");
    store.backup(&backup).await.unwrap();
    store.close().await;
    let scenario = Scenario {
        _guard: directory,
        root,
        history: live.clone(),
    };
    let delta = scenario.delta(&backup, "delta", None);
    let (restored, output) = scenario.restore(&backup, &live, &delta, "restored");
    assert_restored(&output, 0, "none");
    let mut names: Vec<String> = std::fs::read_dir(&restored)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["restore.json", "rsia.sqlite3"]);
    assert_gate_facts_admit(&restored, &live).await;
}

#[tokio::test]
async fn every_other_migration_ledger_is_still_isolated() {
    // A gap: 0007 without 0006, written by the real migrator.
    let gap = Scenario::new(&[1, 2, 4, 5, 7]).await;
    let backup = gap.backup("backup", None, |_| {}).await;
    let anchor = gap.copy(&gap.history, "live", true).await;
    let delta = gap.delta(&backup, "delta", None);
    let (dest, output) = gap.restore(&backup, &anchor, &delta, "restored");
    gap.assert_isolated(&dest, &output, NOT_A_PREFIX);

    // A prefix that predates the root budget ledger.
    let old = Scenario::new(&[1, 2, 4]).await;
    let backup = old.backup("backup", None, |_| {}).await;
    let anchor = old.copy(&old.history, "live", true).await;
    let delta = old.delta(&backup, "delta", None);
    let (dest, output) = old.restore(&backup, &anchor, &delta, "restored");
    old.assert_isolated(&dest, &output, PREDATES);

    // An unknown future version in the database and the manifest.
    let current = Scenario::new(&CURRENT_VERSIONS).await;
    let anchor = current.copy(&current.history, "live", true).await;
    let backup = current
        .backup(
            "future",
            Some("INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(999,'future',TRUE,zeroblob(48),0)"),
            |manifest| {
                manifest["migrations"].as_array_mut().unwrap().push(json!({
                    "version": 999,
                    "description": "future",
                    "checksum_hex": "00".repeat(48),
                    "success": true,
                }));
            },
        )
        .await;
    let delta = current.delta(&backup, "future-delta", None);
    let (dest, output) = current.restore(&backup, &anchor, &delta, "restored-future");
    current.assert_isolated(&dest, &output, NOT_A_PREFIX);

    // A manifest that does not describe its database's ledger.
    let backup = current
        .backup("short-manifest", None, |manifest| {
            manifest["migrations"].as_array_mut().unwrap().pop();
        })
        .await;
    let delta = current.delta(&backup, "short-manifest-delta", None);
    let (dest, output) = current.restore(&backup, &anchor, &delta, "restored-short-manifest");
    current.assert_isolated(
        &dest,
        &output,
        "backup database migrations differ from its manifest",
    );

    let prefix = Scenario::new(&[1, 2, 4, 5, 6]).await;
    let anchor = prefix.copy(&prefix.history, "live", true).await;
    let backup = prefix
        .backup("long-manifest", None, |manifest| {
            manifest["migrations"].as_array_mut().unwrap().push(json!({
                "version": 7,
                "description": "formal parent kind",
                "checksum_hex": checksum_hex(7),
                "success": true,
            }));
        })
        .await;
    let delta = prefix.delta(&backup, "long-manifest-delta", None);
    let (dest, output) = prefix.restore(&backup, &anchor, &delta, "restored-long-manifest");
    prefix.assert_isolated(
        &dest,
        &output,
        "backup database migrations differ from its manifest",
    );

    // A changed checksum inside the prefix, recorded in both places or only in the database.
    let tamper = "UPDATE _sqlx_migrations SET checksum=zeroblob(48) WHERE version=1";
    let backup = prefix
        .backup("checksum", Some(tamper), |manifest| {
            manifest["migrations"][0]["checksum_hex"] = json!("00".repeat(48));
        })
        .await;
    let delta = prefix.delta(&backup, "checksum-delta", None);
    let (dest, output) = prefix.restore(&backup, &anchor, &delta, "restored-checksum");
    prefix.assert_isolated(
        &dest,
        &output,
        "manifest migration checksums differ from current code",
    );
    let backup = prefix.backup("checksum-db", Some(tamper), |_| {}).await;
    let delta = prefix.delta(&backup, "checksum-db-delta", None);
    let (dest, output) = prefix.restore(&backup, &anchor, &delta, "restored-checksum-db");
    prefix.assert_isolated(
        &dest,
        &output,
        "database migration checksums differ from current code",
    );

    // An unsuccessful migration, recorded in both places or only in the database.
    let failed = "UPDATE _sqlx_migrations SET success=0 WHERE version=5";
    let backup = prefix
        .backup("unsuccessful", Some(failed), |manifest| {
            manifest["migrations"][3]["success"] = json!(false);
        })
        .await;
    let delta = prefix.delta(&backup, "unsuccessful-delta", None);
    let (dest, output) = prefix.restore(&backup, &anchor, &delta, "restored-unsuccessful");
    prefix.assert_isolated(&dest, &output, "migration is not successful");
    let backup = prefix.backup("unsuccessful-db", Some(failed), |_| {}).await;
    let delta = prefix.delta(&backup, "unsuccessful-db-delta", None);
    let (dest, output) = prefix.restore(&backup, &anchor, &delta, "restored-unsuccessful-db");
    prefix.assert_isolated(
        &dest,
        &output,
        "backup database has an unsuccessful migration",
    );

    // The anchor stays on the exact compiled set: a prefix anchor is refused,
    // while the same backup against the upgraded anchor is restorable.
    let backup = prefix.backup("valid", None, |_| {}).await;
    let stale_anchor = prefix.copy(&prefix.history, "stale-live", false).await;
    let delta = prefix.delta(&backup, "valid-delta", None);
    let (dest, output) = prefix.restore(&backup, &stale_anchor, &delta, "restored-stale-anchor");
    prefix.assert_isolated(
        &dest,
        &output,
        "database migration checksums differ from current code",
    );
    let (restored, output) = prefix.restore(&backup, &anchor, &delta, "restored-valid");
    assert_restored(&output, 0, "7");
    assert_gate_facts_admit(&restored, &anchor).await;

    // A migration that cannot apply to the copy isolates the restore, and the
    // Rust migrator refuses the same database.
    let backup = prefix
        .backup(
            "conflict",
            Some("CREATE TABLE objects_formal_parent(x)"),
            |_| {},
        )
        .await;
    let delta = prefix.delta(&backup, "conflict-delta", None);
    let (dest, output) = prefix.restore(&backup, &anchor, &delta, "restored-conflict");
    prefix.assert_isolated(&dest, &output, "table objects_formal_parent already exists");
    let refused = prefix
        .copy(&backup.join("rsia.sqlite3"), "conflict-store-open", false)
        .await;
    match Store::open(&refused).await {
        Err(Error::Internal) => {}
        Err(error) => panic!("unexpected refusal: {error:?}"),
        Ok(store) => {
            store.close().await;
            panic!("Store::open applied 0007 over a conflicting table");
        }
    }
}
