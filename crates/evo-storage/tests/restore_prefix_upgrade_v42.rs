//! §18.4: mechanical preservation of the original unqualified fourteen-kind seed.
//! Raw compiled migrations verify SQL/ledger/row preservation only; they do not
//! establish a trusted producer, qualified writer, restore admission or gate result.
//! The replaced healthy restore/delta/gate obligations are mapped to the existing
//! engine restore_economic_v42.rs real-producer prefix controls.

use evo_core::hash;
use evo_storage::lifecycle::{
    BACKUP_SCHEMA, BackupFileEntry, BackupManifest, BackupMigrationEntry, BackupWatermarkEntry,
};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection, migrate::Migrator, sqlite::SqliteConnectOptions};
use std::path::{Path, PathBuf};

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

fn digest_of(text: &str) -> String {
    hash(text.as_bytes())
}

async fn connect(path: &Path, create: bool) -> SqliteConnection {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(create)
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

    /// A copy of `source` in its own directory, migrated by the raw compiled set when
    /// `upgrade` (raw compiled SQL only; no qualified writer/admission).
    async fn copy(&self, source: &Path, name: &str, upgrade: bool) -> PathBuf {
        let directory = self.root.join(name);
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("rsia.sqlite3");
        std::fs::copy(source, &path).unwrap();
        if upgrade {
            let mut connection = connect(&path, false).await;
            migrator(&CURRENT_VERSIONS)
                .run(&mut connection)
                .await
                .unwrap();
            connection.close().await.unwrap();
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
        std::fs::create_dir(&directory).unwrap();
        let database = directory.join("rsia.sqlite3");
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
        std::fs::create_dir_all(directory.join("blobs/legacy-a")).unwrap();
        std::fs::write(
            directory.join(format!("blobs/legacy-a/{blob_digest}")),
            BLOB,
        )
        .unwrap();
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
        std::fs::write(
            directory.join("backup-manifest.json"),
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
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
}

// §18.4 replacement mapping: the old fourteen-kind seed is raw migration data,
// not a producer witness or a qualified writer/restore fixture. Its bytes stay
// unchanged above. Healthy restore/admission and fault controls live in the
// existing engine restore_economic_v42.rs tests, which can call HostService.
async fn raw_compiled_upgrade_preserves_image(versions: &[i64], applied: &[i64]) {
    let scenario = Scenario::new(versions).await;
    let history = image(&scenario.history).await;
    let backup = scenario.backup("backup", None, |_| {}).await;
    let database = backup.join("rsia.sqlite3");
    let raw_database = std::fs::read(&database).unwrap();
    let raw_manifest = std::fs::read(backup.join("backup-manifest.json")).unwrap();
    let manifest: BackupManifest = serde_json::from_slice(&raw_manifest).unwrap();
    assert_eq!(manifest.database.sha256, hash(&raw_database));
    assert_eq!(manifest.database.bytes, raw_database.len() as u64);
    assert_eq!(manifest.blobs.len(), 1);
    let blob_path = backup.join(&manifest.blobs[0].path);
    assert_eq!(std::fs::read(&blob_path).unwrap(), BLOB);
    assert_eq!(manifest.blobs[0].sha256, hash(BLOB));
    assert_eq!(manifest.blobs[0].bytes, BLOB.len() as u64);
    let before_timings = timings(&database).await;
    assert_eq!(
        before_timings.iter().map(|row| row.0).collect::<Vec<_>>(),
        versions
    );

    let together = scenario.copy(&database, "raw-together", true).await;
    let stepwise = scenario.copy(&database, "raw-stepwise", false).await;
    let mut prefix = versions.to_vec();
    for version in applied {
        prefix.push(*version);
        let mut connection = connect(&stepwise, false).await;
        migrator(&prefix).run(&mut connection).await.unwrap();
        connection.close().await.unwrap();
    }
    let upgraded = image(&together).await;
    let migrated = image(&stepwise).await;
    assert_eq!(upgraded.schema, migrated.schema);
    assert_eq!(upgraded.ledger, migrated.ledger);
    assert_eq!(
        upgraded.tables.iter().map(|row| &row.0).collect::<Vec<_>>(),
        migrated.tables.iter().map(|row| &row.0).collect::<Vec<_>>()
    );
    for (left, right) in upgraded.tables.iter().zip(&migrated.tables) {
        assert_eq!(left, right, "table {}", left.0);
    }
    assert_eq!(upgraded, migrated);
    // Every original table, including rowids, raw TEXT/BLOB and FTS shadows,
    // retains every row. Only the migration ledger gains the missing versions.
    for (name, rows) in &history.tables {
        let after = upgraded.tables.iter().find(|row| &row.0 == name).unwrap();
        assert_eq!(&after.1, rows, "raw legacy table {name}");
    }
    assert!(
        upgraded
            .tables
            .iter()
            .any(|(name, rows)| { name == "objects" && rows.len() == LEGACY_KINDS.len() + 5 })
    );
    let mut connection = connect(&together, false).await;
    for (index, kind) in LEGACY_KINDS.iter().enumerate() {
        let id = if *kind == "run" { "run-1" } else { "shared" };
        let row = sqlx::query(
            "SELECT rowid,owner,body,revision FROM objects WHERE namespace='legacy-a' AND kind=? AND id=?",
        )
        .bind(*kind).bind(id).fetch_one(&mut connection).await.unwrap();
        assert_eq!(row.get::<i64, _>("rowid"), 100 + index as i64 * 17);
        assert_eq!(row.get::<String, _>("owner"), format!("owner-{index}"));
        assert_eq!(
            row.get::<String, _>("body"),
            format!(" {{ \"id\" : \"{id}\", \"kind\" : \"{kind}\", \"state\" : \"queued\" }} ")
        );
        assert_eq!(row.get::<i64, _>("revision"), index as i64 + 2);
    }
    let namespaces: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT namespace FROM objects ORDER BY namespace")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(namespaces, ["legacy-a", "legacy-b"]);
    connection.close().await.unwrap();
    assert_ledger_is_the_compiled_set(&together).await;
    assert_ledger_is_the_compiled_set(&stepwise).await;
    let after_timings = timings(&together).await;
    assert_eq!(&after_timings[..versions.len()], before_timings.as_slice());
    for row in &after_timings[versions.len()..] {
        assert!(applied.contains(&row.0));
        assert_eq!((row.2.as_str(), row.4.as_str()), ("text", "integer"));
        assert!(row.3 >= 0, "{row:?}");
    }
    assert_eq!(std::fs::read(&database).unwrap(), raw_database);
    assert_eq!(
        std::fs::read(backup.join("backup-manifest.json")).unwrap(),
        raw_manifest
    );
    assert_eq!(std::fs::read(blob_path).unwrap(), BLOB);
}

#[tokio::test]
async fn raw_compiled_0006_upgrade_preserves_the_full_legacy_image() {
    raw_compiled_upgrade_preserves_image(&[1, 2, 4, 5, 6], &[7]).await;
}

#[tokio::test]
async fn raw_compiled_0005_upgrade_preserves_the_full_legacy_image() {
    raw_compiled_upgrade_preserves_image(&[1, 2, 4, 5], &[6, 7]).await;
}

#[tokio::test]
async fn raw_current_schema_requires_no_further_compiled_migration() {
    let scenario = Scenario::new(&CURRENT_VERSIONS).await;
    let before = image(&scenario.history).await;
    let before_timings = timings(&scenario.history).await;
    let mut connection = connect(&scenario.history, false).await;
    migrator(&CURRENT_VERSIONS)
        .run(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert_eq!(image(&scenario.history).await, before);
    assert_eq!(timings(&scenario.history).await, before_timings);
}

#[tokio::test]
async fn raw_0006_commit_survives_a_0007_sql_conflict_without_writer_admission() {
    let scenario = Scenario::new(&[1, 2, 4, 5]).await;
    let mut connection = connect(&scenario.history, false).await;
    sqlx::raw_sql("CREATE TABLE objects_formal_parent(x)")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let before = image(&scenario.history).await;
    let mut connection = connect(&scenario.history, false).await;
    let error = migrator(&CURRENT_VERSIONS)
        .run(&mut connection)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("table objects_formal_parent already exists")
    );
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(versions, [1, 2, 4, 5, 6]);
    connection.close().await.unwrap();
    let after = image(&scenario.history).await;
    for (name, rows) in &before.tables {
        assert_eq!(
            &after.tables.iter().find(|row| &row.0 == name).unwrap().1,
            rows,
            "{name}"
        );
    }
    for name in [
        "revoke_cleanup_jobs",
        "revoke_cleanup_frontier",
        "revoke_cleanup_events",
    ] {
        assert!(
            after
                .tables
                .iter()
                .any(|(table, rows)| table == name && rows.is_empty())
        );
    }
    // Qualified adapter refusal/rollback is independently retained by
    // migration_clock::{committed_six_survives_seven_post_ddl_admission_failure,
    // rogue_sql_and_conflicting_prefix_have_no_effects}; raw SQL is no admission.
}

#[tokio::test]
async fn legacy_delta_document_preserves_its_original_raw_binding_without_admission() {
    let scenario = Scenario::new(&[1, 2, 4, 5]).await;
    let backup = scenario.backup("backup", None, |_| {}).await;
    let bytes = std::fs::read(backup.join("backup-manifest.json")).unwrap();
    let manifest: BackupManifest = serde_json::from_slice(&bytes).unwrap();
    let base = manifest
        .watermarks
        .iter()
        .find(|row| row.namespace == "legacy-a")
        .unwrap();
    let watermark_digest = digest_of("legacy-a-4");
    let body = tombstone_body(
        "run-1",
        base.seq + 1,
        &watermark_digest,
        "revoked after the upgrade",
        9,
    );
    let tombstone_digest = hash(body.as_bytes());
    let event = json!({
        "seq":base.seq + 1,"previous_digest":base.digest,"digest":watermark_digest,
        "source_kind":"run","source_id":"run-1","tombstone_digest":tombstone_digest,
        "reason":"revoked after the upgrade","created_at":9,
    });
    let path = scenario.delta(
        &backup,
        "delta",
        Some((vec![event.clone()], base.seq + 1, watermark_digest.clone())),
    );
    let delta: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(delta["base_manifest_sha256"], hash(&bytes));
    let rows = delta["namespaces"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let selected = rows
        .iter()
        .find(|row| row["namespace"] == "legacy-a")
        .unwrap();
    assert_eq!(selected["base_seq"], base.seq);
    assert_eq!(selected["base_digest"], base.digest);
    assert_eq!(selected["events"], json!([event]));
    assert_eq!(selected["latest_seq"], base.seq + 1);
    assert_eq!(selected["latest_digest"], watermark_digest);
    assert_eq!(
        selected["events"][0]["tombstone_digest"],
        hash(body.as_bytes())
    );
    let other = rows
        .iter()
        .find(|row| row["namespace"] == "legacy-b")
        .unwrap();
    assert!(other["events"].as_array().unwrap().is_empty());
}
