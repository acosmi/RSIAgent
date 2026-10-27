"""Migration-prefix upgrade in scripts/restore_backup.py (AG-104, E08 / plan §11).

Pure Python. The fixtures execute the repository's migration files with sqlite3
and write the `_sqlx_migrations` rows the way sqlx 0.8.6 writes them, then build
an rsia.backup.v2 package the way `Store::backup` does (VACUUM INTO plus a
manifest read from the database). The real script runs as a subprocess.

These tests pin the prefix rule, the sqlx description and checksum derivation,
the one-transaction-per-migration upgrade and every rejection. The Rust test
crates/evo-storage/tests/restore_prefix_upgrade_v42.rs proves the upgraded
database equal to `Store::open` on ledgers the real sqlx migrator wrote.
"""

from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts import restore_backup as restore

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/restore_backup.py"
MIGRATIONS = ROOT / "crates/evo-storage/migrations"
CURRENT = [1, 2, 4, 5, 6, 7]
# The descriptions sqlx 0.8.6 derives from the current file names.
DESCRIPTIONS = {
    1: "runtime",
    2: "revoke graph",
    4: "replay worlds",
    5: "root budget",
    6: "revoke cleanup",
    7: "formal parent kind",
}
# sqlx-sqlite 0.8.6 `ensure_migrations_table`.
LEDGER_DDL = """
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);
"""
NOT_A_PREFIX = "unsupported manifest migrations: not a prefix of the current migration set"
PREDATES = "backup predates the root budget ledger (0005); protected facts cannot be verified"
RECEIPT_KEYS = {
    "schema_version",
    "backup_manifest_sha256",
    "replayed_revoke_events",
    "isolated",
    "trusted_anchor",
    "receipt_scope",
}
ANCHOR_KEYS = {"path_local_only", "watermarks", "control_plane_digest", "verified_at", "scope"}


def sha256_text(text):
    return hashlib.sha256(text.encode()).hexdigest()


WATERMARKS = {"legacy-a": (3, sha256_text("legacy-a-3")), "legacy-b": (1, sha256_text("legacy-b-1"))}


def migration_file(version):
    (path,) = MIGRATIONS.glob(f"{version:04d}_*.sql")
    return path


def connect(path):
    con = sqlite3.connect(path, isolation_level=None)
    con.execute("PRAGMA foreign_keys=ON")
    return con


def sqlx_migrate(con, versions):
    """Apply `versions` the way sqlx 0.8.6 does: one transaction per file with its ledger row."""
    con.executescript(LEDGER_DDL)
    for version in versions:
        raw = migration_file(version).read_bytes()
        con.executescript("BEGIN;\n" + raw.decode())
        con.execute(
            "INSERT INTO _sqlx_migrations ( version, description, success, checksum, execution_time ) "
            "VALUES ( ?, ?, TRUE, ?, -1 )",
            (version, DESCRIPTIONS[version], hashlib.sha384(raw).digest()),
        )
        con.execute("COMMIT")
        con.execute("UPDATE _sqlx_migrations SET execution_time = 1000 WHERE version = ?", (version,))


def tombstone(source_id, seq, digest, reason="revoked before the backup", created_at=5):
    return {
        "id": source_id,
        "schema_version": "rsia.revoke_tombstone.v1",
        "source_kind": "run",
        "reason": reason,
        "watermark_seq": seq,
        "watermark_digest": digest,
        "created_at": created_at,
    }


def seed(con, versions):
    """Facts of every kind the restore reads, in two watermarked namespaces, with gapped rowids."""
    objects = [
        (8, "legacy-a", "artifact", "first", "owner-first", ' { "id" : "first", "v" : 1 } ', 19),
        (100, "legacy-a", "run", "run-1", "host", '{"id":"run-1","schema_version":"rsia.optimization.source.v1"}', 2),
        (117, "legacy-a", "budget", "budget-1", "admin", '{"id":"budget-1","limit":10}', 1),
        (
            134,
            "legacy-a",
            "artifact",
            "ledger-1",
            "admin",
            json.dumps(
                {
                    "id": "ledger-1",
                    "schema_version": "rsia.typed_artifact_envelope.v1",
                    "record_kind": "exposure_ledger_v1",
                    "payload": {"spent": 1},
                }
            ),
            4,
        ),
        (151, "legacy-a", "tombstone", "run-0", "admin", json.dumps(tombstone("run-0", 2, sha256_text("legacy-a-2"))), 1),
        (900, "legacy-b", "evaluation", "eval-1", "owner-b", '{"id":"eval-1","score":0.5}', 7),
        (3, "legacy-b", "artifact", "shared", "owner-b", '{"id":"shared","v":"other namespace"}', 31),
    ]
    con.executemany(
        "INSERT INTO objects(rowid,namespace,kind,id,owner,body,revision) VALUES(?,?,?,?,?,?,?)", objects
    )
    for namespace, (seq, digest) in WATERMARKS.items():
        con.execute("INSERT INTO revoke_watermark VALUES(?,?,?)", (namespace, seq, digest))
    con.executescript(
        """
        INSERT INTO idempotency VALUES('legacy-a','actor','operation','key','payload','shared',' { "ok" : true } ',0);
        INSERT INTO audit(seq,namespace,payload,previous_hash,digest) VALUES(27,'legacy-a',' { "event" : "old" } ','previous','digest');
        INSERT INTO dependencies VALUES('legacy-a','artifact','first','run','run-1');
        INSERT INTO skill_fts(namespace,candidate_id,tokens) VALUES('legacy-a','shared','old tokens');
        """
    )
    if 4 in versions:
        con.execute("INSERT INTO replay_worlds VALUES('legacy-a','world',1,' { \"sealed\" : true } ')")
    if 5 in versions:
        con.executescript(
            """
            INSERT INTO root_budgets(billing_scope,root_budget_id,authorizing_namespace,currency,pricing_version,payment_subject,authorization_receipt_digest,per_call_cap_micros,total_limit_micros,spent_micros,reserved_micros,created_at)
             VALUES('scope','root','legacy-a','USD','price','payer','authorization',100,1000,11,7,123);
            INSERT INTO root_budget_namespaces VALUES('scope','legacy-a');
            INSERT INTO root_budget_dispatch_groups(billing_scope,dispatch_group_id,owner_namespace,created_at) VALUES('scope','group','legacy-a',123);
            INSERT INTO root_budget_calls(billing_scope,call_id,dispatch_group_id,namespace,stage,actual_input_digest,reserved_micros,state,lease_token,lease_epoch,lease_until,created_at)
             VALUES('scope','call','group','legacy-a','optimization','input',7,'reserved','lease',3,300,123);
            INSERT INTO root_budget_events(seq,billing_scope,call_id,event_kind,event_at,details) VALUES(41,'scope','call','reserved',123,' { "old" : true } ');
            """
        )
    if 6 in versions:
        con.executescript(
            """
            INSERT INTO revoke_cleanup_jobs(namespace,job_id,source_kind,source_id,watermark_seq,watermark_digest,state,created_at,updated_at)
             VALUES('legacy-a','cleanup','run','run-0',2,'watermark','pending',123,124);
            INSERT INTO revoke_cleanup_frontier(seq,namespace,job_id,node_kind,node_id) VALUES(61,'legacy-a','cleanup','artifact','first');
            INSERT INTO revoke_cleanup_events(seq,namespace,job_id,node_kind,node_id,event_kind,event_at,details)
             VALUES(71,'legacy-a','cleanup','artifact','first','queued',124,' { "old" : true } ');
            """
        )


def quoted(name):
    return '"' + name.replace('"', '""') + '"'


def database_image(path):
    """Schema SQL, the ledger without its timing columns, and every table's rows (with rowids)."""
    con = sqlite3.connect(Path(path).resolve().as_uri() + "?mode=ro", uri=True)
    try:
        schema = con.execute("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name").fetchall()
        ledger = con.execute(
            "SELECT quote(version),quote(description),quote(success),quote(checksum) FROM _sqlx_migrations ORDER BY version"
        ).fetchall()
        tables = {}
        for (table,) in con.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name<>'_sqlx_migrations' ORDER BY name"
        ).fetchall():
            columns = [row[0] for row in con.execute("SELECT name FROM pragma_table_info(?) ORDER BY cid", (table,))]
            try:
                con.execute(f"SELECT rowid FROM {quoted(table)} LIMIT 0")
                columns = ["rowid", *columns]
            except sqlite3.OperationalError:
                pass  # WITHOUT ROWID (FTS5 shadow tables)
            selected = ",".join(f"quote({column if column == 'rowid' else quoted(column)})" for column in columns)
            tables[table] = sorted(con.execute(f"SELECT {selected} FROM {quoted(table)}").fetchall())
        integrity = con.execute("PRAGMA integrity_check").fetchall()
        foreign_keys = con.execute("PRAGMA foreign_key_check").fetchall()
        return {"schema": schema, "ledger": ledger, "tables": tables, "integrity": integrity, "foreign_keys": foreign_keys}
    finally:
        con.close()


class Scenario:
    """A historical live database on `versions`, its backups, and anchors made from it."""

    def __init__(self, test, versions):
        self.base = Path(tempfile.mkdtemp(prefix="ag104-")).resolve()
        test.addCleanup(shutil.rmtree, self.base, True)
        self.versions = list(versions)
        self.history = self.base / "history.sqlite3"
        con = connect(self.history)
        sqlx_migrate(con, self.versions)
        seed(con, self.versions)
        con.close()

    def anchor(self, name="live", upgrade=True):
        """The current control plane: the live database after the binary upgrade."""
        directory = self.base / name
        directory.mkdir()
        path = directory / "rsia.sqlite3"
        shutil.copyfile(self.history, path)
        if upgrade:
            con = connect(path)
            sqlx_migrate(con, [version for version in CURRENT if version not in self.versions])
            con.close()
        return path

    def backup(self, name="backup", *, db_sql=None, manifest=None):
        """VACUUM INTO and a manifest read from that database, like `Store::backup`.

        `db_sql` changes the copied database after the manifest's migration and
        watermark lists were read; `manifest` mutates the document."""
        directory = self.base / name
        directory.mkdir()
        database = directory / "rsia.sqlite3"
        con = connect(self.history)
        con.execute("VACUUM INTO ?", (str(database),))
        con.close()
        con = connect(database)
        migrations = [
            {"version": version, "description": description, "checksum_hex": bytes(checksum).hex(), "success": bool(success)}
            for version, description, checksum, success in con.execute(
                "SELECT version,description,checksum,success FROM _sqlx_migrations ORDER BY version"
            )
        ]
        watermarks = [
            {"namespace": namespace, "seq": seq, "digest": digest}
            for namespace, seq, digest in con.execute("SELECT namespace,seq,digest FROM revoke_watermark ORDER BY namespace")
        ]
        if db_sql:
            con.executescript(db_sql)
        con.close()
        data = database.read_bytes()
        document = {
            "schema_version": "rsia.backup.v2",
            "complete": True,
            "database": {"path": "rsia.sqlite3", "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)},
            "blobs": [],
            "migrations": migrations,
            "watermarks": watermarks,
            "created_at": 1,
        }
        if manifest:
            manifest(document)
        (directory / "backup-manifest.json").write_bytes(json.dumps(document, indent=2).encode())
        return directory

    def delta(self, backup, events=None, latest=None):
        """The exact revoke delta; `events`/`latest` replace legacy-a's tail."""
        manifest = (backup / "backup-manifest.json").read_bytes()
        namespaces = []
        for mark in json.loads(manifest)["watermarks"]:
            entry = {
                "namespace": mark["namespace"],
                "base_seq": mark["seq"],
                "base_digest": mark["digest"],
                "events": [],
                "latest_seq": mark["seq"],
                "latest_digest": mark["digest"],
            }
            if mark["namespace"] == "legacy-a" and events is not None:
                entry["events"] = events
                entry["latest_seq"], entry["latest_digest"] = latest
            namespaces.append(entry)
        path = backup.parent / f"{backup.name}-delta-{len(list(backup.parent.glob('*-delta-*')))}.json"
        path.write_text(
            json.dumps(
                {
                    "schema_version": "rsia.revoke_delta.v1",
                    "base_manifest_sha256": hashlib.sha256(manifest).hexdigest(),
                    "namespaces": namespaces,
                }
            )
        )
        return path

    def restore(self, backup, anchor, name="restored", delta=None):
        dest = self.base / name
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--backup", str(backup),
                "--dest", str(dest),
                "--revoke-delta", str(delta or self.delta(backup)),
                "--trusted-revocations-db", str(anchor),
            ],
            capture_output=True,
            text=True,
            timeout=120,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )
        return dest, result


class PrefixRuleTests(unittest.TestCase):
    def test_only_the_current_set_or_a_prefix_through_0005_is_restorable(self):
        self.assertEqual(restore.missing_migrations([1, 2, 4, 5, 6, 7]), [])
        self.assertEqual(restore.missing_migrations([1, 2, 4, 5, 6]), [7])
        self.assertEqual(restore.missing_migrations([1, 2, 4, 5]), [6, 7])
        for versions in ([1, 2, 4], [1, 2], [1]):
            with self.assertRaisesRegex(restore.IsolationError, r"^backup predates the root budget ledger \(0005\); protected facts cannot be verified$"):
                restore.missing_migrations(versions)
        for versions in (
            [],
            [1, 2, 4, 5, 7],
            [1, 2, 4, 5, 6, 7, 999],
            [1, 2, 4, 5, 999],
            [2, 4, 5],
            [1, 2, 3, 4, 5],
            [1, 1, 2, 4, 5],
            [1, 2, 5, 4],
            [7],
        ):
            with self.assertRaisesRegex(restore.IsolationError, f"^{NOT_A_PREFIX}$"):
                restore.missing_migrations(versions)

    def test_descriptions_follow_the_sqlx_file_name_rule(self):
        self.assertEqual(restore.sqlx_description("0007_formal_parent_kind.sql"), "formal parent kind")
        self.assertEqual(restore.sqlx_description("0001_runtime.sql"), "runtime")
        # split once at "_"; trim_end_matches strips a repeated suffix; type suffixes
        self.assertEqual(restore.sqlx_description("12_a_b_c.sql"), "a b c")
        self.assertEqual(restore.sqlx_description("12_twice.sql.sql"), "twice")
        self.assertEqual(restore.sqlx_description("12_up_step.up.sql"), "up step")
        self.assertEqual(restore.sqlx_description("12_down_step.down.sql"), "down step")
        self.assertEqual(restore.sqlx_description("12_.sql"), "")

    def test_code_migrations_read_each_file_once_with_sqlx_checksums(self):
        code = restore.code_migrations()
        self.assertEqual(sorted(code), CURRENT)
        for version, (description, raw, checksum) in code.items():
            path = migration_file(version)
            self.assertEqual(description, DESCRIPTIONS[version])
            self.assertEqual(raw, path.read_bytes())
            self.assertEqual(checksum, hashlib.sha384(path.read_bytes()).hexdigest())
        self.assertEqual(restore.current_migrations(), {version: entry[2] for version, entry in code.items()})


class UpgradeStepTests(unittest.TestCase):
    def test_every_missing_migration_commits_in_its_own_transaction(self):
        scenario = Scenario(self, [1, 2, 4, 5])
        database = scenario.base / "copy.sqlite3"
        shutil.copyfile(scenario.history, database)
        con = connect(database)
        con.execute("CREATE TABLE objects_formal_parent(x)")  # makes 0007 fail
        objects_sql = con.execute("SELECT sql FROM sqlite_master WHERE name='objects'").fetchone()
        con.close()
        with self.assertRaisesRegex(sqlite3.OperationalError, "objects_formal_parent already exists"):
            restore.apply_missing_migrations(database, [1, 2, 4, 5])
        con = connect(database)
        # 0006 and its ledger row committed; 0007 rolled back as a whole.
        self.assertEqual([row[0] for row in con.execute("SELECT version FROM _sqlx_migrations ORDER BY version")], [1, 2, 4, 5, 6])
        self.assertIsNotNone(con.execute("SELECT 1 FROM sqlite_master WHERE name='revoke_cleanup_jobs'").fetchone())
        self.assertEqual(con.execute("SELECT sql FROM sqlite_master WHERE name='objects'").fetchone(), objects_sql)
        self.assertEqual(con.execute("SELECT count(*) FROM objects_formal_parent").fetchone(), (0,))
        self.assertFalse(Path(str(database) + "-journal").exists())
        con.close()

    def test_a_migration_that_ends_its_own_transaction_is_refused(self):
        scenario = Scenario(self, [1, 2, 4, 5, 6])
        database = scenario.base / "copy.sqlite3"
        shutil.copyfile(scenario.history, database)
        code = restore.code_migrations()
        rogue = b"CREATE TABLE rogue(x);\nCOMMIT;\n"
        code[7] = (code[7][0], rogue, hashlib.sha384(rogue).hexdigest())
        with mock.patch.object(restore, "code_migrations", return_value=code):
            with self.assertRaisesRegex(restore.IsolationError, "^migration 7 ended its own transaction$"):
                restore.apply_missing_migrations(database, [1, 2, 4, 5, 6])
        con = connect(database)
        self.assertEqual([row[0] for row in con.execute("SELECT version FROM _sqlx_migrations ORDER BY version")], [1, 2, 4, 5, 6])
        con.close()

    def test_the_current_set_is_not_touched(self):
        scenario = Scenario(self, CURRENT)
        before = scenario.history.read_bytes()
        self.assertEqual(restore.apply_missing_migrations(scenario.history, CURRENT), [])
        self.assertEqual(scenario.history.read_bytes(), before)


class RestoreTests(unittest.TestCase):
    def assert_restored(self, dest, result, events, applied):
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")
        self.assertEqual(result.stdout, f"RESTORE_OK events={events} migrations_applied={applied}\n")
        receipt = json.loads((dest / "restore.json").read_text())
        self.assertEqual(set(receipt), RECEIPT_KEYS)
        self.assertEqual(set(receipt["trusted_anchor"]), ANCHOR_KEYS)
        self.assertEqual(receipt["schema_version"], "rsia.restore_receipt.v2")
        self.assertEqual(receipt["replayed_revoke_events"], events)
        self.assertIs(receipt["isolated"], False)

    def assert_isolated(self, scenario, dest, result, reason):
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertEqual(result.stderr, f"ISOLATE {reason}; not mounting\n")
        self.assertFalse(dest.exists())
        leftovers = [path.name for path in scenario.base.iterdir() if path.name.startswith(f".{dest.name}.restore-")]
        self.assertEqual(leftovers, [])

    def test_a_0006_backup_is_upgraded_before_publication_and_equals_the_upgraded_anchor(self):
        scenario = Scenario(self, [1, 2, 4, 5, 6])
        backup = scenario.backup()
        anchor = scenario.anchor()
        dest, result = scenario.restore(backup, anchor)
        self.assert_restored(dest, result, 0, "7")
        self.assertEqual(sorted(path.name for path in dest.iterdir()), ["restore.json", "restore.migrations.json", "rsia.sqlite3"])
        record = (dest / "restore.migrations.json").read_text()
        self.assertTrue(record.endswith("\n"))
        self.assertEqual(
            json.loads(record),
            {
                "schema": "rsia.restore_migrations.v1",
                "source_versions": [1, 2, 4, 5, 6],
                "applied": [{"version": 7, "checksum_sha384": hashlib.sha384(migration_file(7).read_bytes()).hexdigest()}],
                "current_versions": CURRENT,
            },
        )
        self.assertEqual(list(json.loads(record)), ["schema", "source_versions", "applied", "current_versions"])
        restored, upgraded = database_image(dest / "rsia.sqlite3"), database_image(anchor)
        self.assertEqual(restored, upgraded)
        self.assertEqual(restored["integrity"], [("ok",)])
        self.assertEqual(restored["foreign_keys"], [])
        # The backup's ledger rows are kept byte for byte; the new row is written like sqlx's.
        con = sqlite3.connect(dest / "rsia.sqlite3")
        old = sqlite3.connect(backup / "rsia.sqlite3")
        everything = "SELECT version,description,installed_on,success,checksum,execution_time FROM _sqlx_migrations"
        self.assertEqual(con.execute(everything + " WHERE version<7 ORDER BY version").fetchall(), old.execute(everything + " ORDER BY version").fetchall())
        row = con.execute(
            "SELECT version,description,typeof(installed_on),typeof(success),success,typeof(checksum),length(checksum),typeof(execution_time),execution_time>=0 FROM _sqlx_migrations WHERE version=7"
        ).fetchone()
        self.assertEqual(row, (7, "formal parent kind", "text", "integer", 1, "blob", 48, "integer", 1))
        self.assertEqual(
            con.execute("SELECT rowid,namespace,kind,id,owner,body,revision FROM objects ORDER BY rowid").fetchall(),
            old.execute("SELECT rowid,namespace,kind,id,owner,body,revision FROM objects ORDER BY rowid").fetchall(),
        )
        con.close()
        old.close()
        # The backup itself was only read.
        self.assertEqual(sorted(path.name for path in backup.iterdir()), ["backup-manifest.json", "rsia.sqlite3"])

    def test_a_0005_backup_replays_a_later_revocation_after_the_upgrade(self):
        scenario = Scenario(self, [1, 2, 4, 5])
        backup = scenario.backup()
        anchor = scenario.anchor()
        base_seq, base_digest = WATERMARKS["legacy-a"]
        latest_digest = sha256_text("legacy-a-4")
        body = json.dumps(tombstone("run-1", base_seq + 1, latest_digest, "revoked after the backup", 9))
        con = connect(anchor)
        con.execute("INSERT INTO objects(namespace,kind,id,owner,body) VALUES('legacy-a','tombstone','run-1','admin',?)", (body,))
        con.execute("UPDATE revoke_watermark SET seq=?,digest=? WHERE namespace='legacy-a'", (base_seq + 1, latest_digest))
        con.close()
        event = {
            "seq": base_seq + 1,
            "previous_digest": base_digest,
            "digest": latest_digest,
            "source_kind": "run",
            "source_id": "run-1",
            "tombstone_digest": sha256_text(body),
            "reason": "revoked after the backup",
            "created_at": 9,
        }
        delta = scenario.delta(backup, [event], (base_seq + 1, latest_digest))
        dest, result = scenario.restore(backup, anchor, delta=delta)
        self.assert_restored(dest, result, 1, "6,7")
        self.assertEqual(json.loads((dest / "restore.migrations.json").read_text())["applied"], [
            {"version": 6, "checksum_sha384": hashlib.sha384(migration_file(6).read_bytes()).hexdigest()},
            {"version": 7, "checksum_sha384": hashlib.sha384(migration_file(7).read_bytes()).hexdigest()},
        ])
        con = sqlite3.connect(dest / "rsia.sqlite3")
        self.assertEqual(con.execute("SELECT version FROM _sqlx_migrations ORDER BY version").fetchall(), [(v,) for v in CURRENT])
        self.assertEqual(con.execute("SELECT seq,digest FROM revoke_watermark WHERE namespace='legacy-a'").fetchone(), (base_seq + 1, latest_digest))
        self.assertEqual(
            json.loads(con.execute("SELECT body FROM objects WHERE namespace='legacy-a' AND kind='tombstone' AND id='run-1'").fetchone()[0]),
            json.loads(body),
        )
        self.assertEqual(con.execute("SELECT source_id,state FROM revoke_cleanup_jobs").fetchall(), [("run-1", "pending")])
        self.assertEqual(con.execute("SELECT count(*) FROM revoke_cleanup_frontier").fetchone(), (3,))
        self.assertEqual(con.execute("SELECT event_kind FROM revoke_cleanup_events").fetchall(), [("logical_block_restored",)])
        con.close()

    def test_a_current_set_backup_is_restored_as_before(self):
        scenario = Scenario(self, CURRENT)
        backup = scenario.backup()
        anchor = scenario.anchor()
        dest, result = scenario.restore(backup, anchor)
        self.assert_restored(dest, result, 0, "none")
        self.assertEqual(sorted(path.name for path in dest.iterdir()), ["restore.json", "rsia.sqlite3"])
        self.assertEqual((dest / "rsia.sqlite3").read_bytes(), (backup / "rsia.sqlite3").read_bytes())

    def test_a_gap_is_isolated(self):
        scenario = Scenario(self, [1, 2, 4, 5, 7])
        dest, result = scenario.restore(scenario.backup(), scenario.anchor())
        self.assert_isolated(scenario, dest, result, NOT_A_PREFIX)

    def test_an_unknown_future_version_is_isolated(self):
        scenario = Scenario(self, CURRENT)
        backup = scenario.backup(
            db_sql="INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(999,'future',TRUE,zeroblob(48),0)",
            manifest=lambda m: m["migrations"].append({"version": 999, "description": "future", "checksum_hex": "00" * 48, "success": True}),
        )
        dest, result = scenario.restore(backup, scenario.anchor())
        self.assert_isolated(scenario, dest, result, NOT_A_PREFIX)

    def test_a_prefix_older_than_0005_is_isolated(self):
        scenario = Scenario(self, [1, 2, 4])
        dest, result = scenario.restore(scenario.backup(), scenario.anchor())
        self.assert_isolated(scenario, dest, result, PREDATES)

    def test_a_changed_checksum_inside_the_prefix_is_isolated(self):
        scenario = Scenario(self, [1, 2, 4, 5, 6])
        anchor = scenario.anchor()
        tamper = "UPDATE _sqlx_migrations SET checksum=zeroblob(48) WHERE version=1"
        # recorded in both the database and the manifest, as a backup of that database would
        both = scenario.backup("both", db_sql=tamper, manifest=lambda m: m["migrations"][0].update(checksum_hex="00" * 48))
        dest, result = scenario.restore(both, anchor, "restored-both")
        self.assert_isolated(scenario, dest, result, "manifest migration checksums differ from current code")
        # only in the database
        database_only = scenario.backup("database-only", db_sql=tamper)
        dest, result = scenario.restore(database_only, anchor, "restored-database-only")
        self.assert_isolated(scenario, dest, result, "database migration checksums differ from current code")

    def test_an_unsuccessful_migration_is_isolated(self):
        scenario = Scenario(self, [1, 2, 4, 5, 6])
        anchor = scenario.anchor()
        failed = "UPDATE _sqlx_migrations SET success=0 WHERE version=5"
        both = scenario.backup("both", db_sql=failed, manifest=lambda m: m["migrations"][3].update(success=False))
        dest, result = scenario.restore(both, anchor, "restored-both")
        self.assert_isolated(scenario, dest, result, "migration is not successful")
        database_only = scenario.backup("database-only", db_sql=failed)
        dest, result = scenario.restore(database_only, anchor, "restored-database-only")
        self.assert_isolated(scenario, dest, result, "backup database has an unsuccessful migration")

    def test_a_manifest_that_disagrees_with_its_database_is_isolated(self):
        current = Scenario(self, CURRENT)
        backup = current.backup(manifest=lambda m: m["migrations"].pop())
        dest, result = current.restore(backup, current.anchor())
        self.assert_isolated(current, dest, result, "backup database migrations differ from its manifest")
        prefix = Scenario(self, [1, 2, 4, 5, 6])
        backup = prefix.backup(
            manifest=lambda m: m["migrations"].append(
                {
                    "version": 7,
                    "description": DESCRIPTIONS[7],
                    "checksum_hex": hashlib.sha384(migration_file(7).read_bytes()).hexdigest(),
                    "success": True,
                }
            )
        )
        dest, result = prefix.restore(backup, prefix.anchor())
        self.assert_isolated(prefix, dest, result, "backup database migrations differ from its manifest")

    def test_an_anchor_on_a_prefix_is_isolated(self):
        scenario = Scenario(self, [1, 2, 4, 5, 6])
        backup = scenario.backup()
        dest, result = scenario.restore(backup, scenario.anchor("stale-anchor", upgrade=False))
        self.assert_isolated(scenario, dest, result, "database migration checksums differ from current code")
        # the same backup against an upgraded anchor is restorable
        dest, result = scenario.restore(backup, scenario.anchor(), "restored-current-anchor")
        self.assert_restored(dest, result, 0, "7")

    def test_a_migration_that_fails_on_the_copy_is_isolated(self):
        scenario = Scenario(self, [1, 2, 4, 5, 6])
        backup = scenario.backup(db_sql="CREATE TABLE objects_formal_parent(x)")
        dest, result = scenario.restore(backup, scenario.anchor())
        self.assert_isolated(scenario, dest, result, "table objects_formal_parent already exists")


    def test_current_proposal_restore_protection_matches_rust(self):
        import re
        rust=(ROOT / "crates/evo-storage/src/lifecycle.rs").read_text()
        body=re.search(r"PROTECTED_ACTION_SCHEMA_VERSIONS: \[&str; 12\] = \[(.*?)\];",rust,re.S).group(1)
        schemas=re.findall(r'"([^"]+)"',body)
        self.assertEqual(len(schemas),12)
        self.assertEqual(schemas[-1],"rsia.monitoring.consolidation_proposal.v2")
        con=sqlite3.connect(":memory:")
        self.addCleanup(con.close)
        con.execute("CREATE TABLE objects(namespace,kind,id,body)")
        for index,schema in enumerate(schemas):
            con.execute("INSERT INTO objects VALUES('n','artifact',?,?)",(str(index),json.dumps({"id":str(index),"schema_version":schema,"content":"fixture content"})))
        facts=restore.protected_facts(con,{"n"})
        self.assertEqual(len(facts),12)
        self.assertEqual({fact[3]["schema_version"] for fact in facts},set(schemas))

    def test_current_proposal_tamper_isolates_restore(self):
        scenario=Scenario(self,CURRENT)
        con=connect(scenario.history)
        for suffix in ("v1","v2"):
            body=json.dumps({"id":"proposal-"+suffix,"schema_version":"rsia.monitoring.consolidation_proposal."+suffix,"payload":"original source-derived content"})
            con.execute("INSERT INTO objects(namespace,kind,id,owner,body) VALUES('legacy-a','artifact',?,'admin',?)",("proposal-"+suffix,body))
        con.close()
        anchor=scenario.anchor()
        backup=scenario.backup("healthy-proposals")
        dest,result=scenario.restore(backup,anchor,"healthy-proposal-restore")
        self.assert_restored(dest,result,0,"none")
        changed=scenario.backup("changed-proposal",db_sql="UPDATE objects SET body=json_set(body,'$.payload','changed source-derived content') WHERE id='proposal-v2'")
        before=database_image(anchor)
        dest,result=scenario.restore(changed,anchor,"refused-proposal-restore")
        self.assert_isolated(scenario,dest,result,"consumed query/alpha/dispatch/accounting facts changed; make a fresh backup")
        self.assertEqual(database_image(anchor),before)

    def test_current_host_raw_precision_refusal_preserves_old_restore_bytes(self):
        scenario=Scenario(self,CURRENT)
        # Complete reviewed source/manifest/extraction, with the exact typed
        # field order that the Rust producer fingerprints. This grants no role.
        declaration={"name":"option","value_type":"object","enum_values":[],"default":{"kind":"value","value":{"decimal":0.1}}}
        original=json.dumps({"schema_version":"rsia.host_reviewed_declarations.v1","host":"restore-host","host_version":"1","declarations":[declaration]},separators=(",",":"))
        source={"schema_version":"rsia.host_source_envelope.v1","source_kind":"reviewed_fixture","source_schema":"rsia.host_reviewed_declarations.v1","extractor_version":"rsia.host_reviewed_declarations.extractor.v1","original_utf8":original}
        encoded=lambda v:json.dumps(v,ensure_ascii=False,separators=(",",":"))
        digest=lambda v:sha256_text(encoded(v))
        item={"name":"option","coverage":"unsupported","mapped_field":None,"consumer":None,"reason":"restore content control","renamed_from":None,"test_ref":None,"capability":None,
            "value_type":"object","enum_values":[],"default":declaration["default"]}
        manifest={"schema_version":"rsia.host_surface.v2","host":"restore-host","host_version":"1","adapter_version":"1","source_kind":"reviewed_fixture",
            "source_schema":source["source_schema"],"extractor_version":source["extractor_version"],"source_envelope_digest":digest(source),"source_digest":sha256_text(original),"items":[item],"retired":[]}
        body={"id":"host-new","schema_version":"rsia.host_surface_record.v2","manifest":manifest,"manifest_digest":digest(manifest),"source":source,
            "extracted":["option"],"extracted_typed":[declaration],"predecessor_surface_id":None,
            "upgrade_diff":{key:["option"] if key=="added" else [] for key in ("added","removed","renamed","type_changed","enum_changed","default_changed","mapping_changed","legacy_untyped")}}
        current=encoded(body)
        legacy='{"id":"host-old","schema_version":"rsia.host_surface_record.v1","default":{"kind":"value","value":0.100000000000000001}}'
        con=connect(scenario.history)
        for identity,raw in (("host-old",legacy),("host-new",current)):
            con.execute("INSERT INTO objects(namespace,kind,id,owner,body) VALUES('legacy-a','artifact',?,'admin',?)",(identity,raw))
        con.close()
        anchor=scenario.anchor()
        backup=scenario.backup("healthy-host-raw")
        dest,result=scenario.restore(backup,anchor,"healthy-host-restore")
        self.assert_restored(dest,result,0,"none")
        con=connect(dest / "rsia.sqlite3")
        self.assertEqual(con.execute("SELECT body FROM objects WHERE id='host-old'").fetchone()[0],legacy)
        self.assertEqual(con.execute("SELECT body FROM objects WHERE id='host-new'").fetchone()[0],current)
        con.close()
        before_anchor=database_image(anchor)
        escaped_duplicate=original.replace('"decimal":0.1','"decimal":0.1,"\\u0064ecimal":0.1')
        with self.assertRaisesRegex(restore.IsolationError,"duplicate JSON object key"):
            restore.read_host_json_tokens(escaped_duplicate)
        faults={"outer-precision":current.replace('"decimal":0.1','"decimal":0.100000000000000001',1)}
        for fault,raw_source in {
            "source-precision":original.replace("0.1","0.100000000000000001"),
            "source-escaped-key":original.replace('"decimal":0.1','"decimal":0.1,"\\u0064ecimal":0.1'),
            "source-mismatch":original.replace("0.1","0.2"),
            "source-resource":original.replace("0.1","["*257+"0"+"]"*257),
        }.items():
            changed=json.loads(current);changed["source"]["original_utf8"]=raw_source;faults[fault]=encoded(changed)
        wrong=json.loads(current);wrong["id"]="another-physical-id"
        with self.assertRaisesRegex(restore.IsolationError,"invalid current host raw JSON"):
            restore.read_current_host_raw(encoded(wrong),"legacy-a","artifact","host-new")
        for fault,damaged in faults.items():
            with self.subTest(fault=fault):
                self.assertNotEqual(damaged,current)
                con=connect(scenario.history)
                con.execute("UPDATE objects SET body=? WHERE id='host-new'",(damaged,));con.close()
                changed=scenario.backup("damaged-host-"+fault)
                before_backup=database_image(changed / "rsia.sqlite3")
                dest,result=scenario.restore(changed,anchor,"refused-host-"+fault)
                self.assert_isolated(scenario,dest,result,"invalid current host raw JSON")
                self.assertEqual(database_image(anchor),before_anchor)
                self.assertEqual(database_image(changed / "rsia.sqlite3"),before_backup)


def _host_database_family(path):
    path=Path(path)
    family={}
    for suffix in ("","-wal","-shm"):
        member=Path(str(path)+suffix)
        if not member.exists():
            assert suffix
            continue
        assert member.is_file() and not member.is_symlink()
        family[suffix]=(member.stat().st_mode,member.read_bytes())
    return family


def _host_private_database_image(path):
    # Opening a read-only WAL database can update SHM. Observe a complete
    # private family copy and prove the original bytes/permissions unchanged.
    path=Path(path);before=_host_database_family(path)
    with tempfile.TemporaryDirectory(prefix="host-raw-observer-") as private:
        copy=Path(private)/path.name
        for suffix,(mode,raw) in before.items():
            member=Path(str(copy)+suffix);member.write_bytes(raw);member.chmod(mode)
        image=database_image(copy)
    assert _host_database_family(path)==before
    return image,before


@contextmanager
def _host_anchor_read_observer(test,path):
    """Attribute this closed-Store fixture's WAL setup, then restore its cold image."""
    path=Path(path);cold_family=_host_database_family(path)
    test.assertEqual(set(cold_family),{""},"existing anchor WAL/SHM must remain untouched")
    cold=_host_private_database_image(path)
    test.assertEqual(cold[1],cold_family)
    diagnostic={"cold":{suffix:{"mode":mode,"bytes":len(raw),"sha256":hashlib.sha256(raw).hexdigest()}
        for suffix,(mode,raw) in cold_family.items()}}
    observer=sqlite3.connect(path.resolve().as_uri()+"?mode=rw",uri=True,isolation_level=None)
    try:
        observer.execute("PRAGMA query_only=ON")
        observer.execute("BEGIN")
        observer.execute("SELECT count(*) FROM sqlite_master").fetchone()
        warm=_host_private_database_image(path)
        diagnostic["warm"]={suffix:{"mode":mode,"bytes":len(raw),"sha256":hashlib.sha256(raw).hexdigest()}
            for suffix,(mode,raw) in warm[1].items()}
        test.assertEqual(warm[0],cold[0],"observer setup changed the complete logical image")
        test.assertEqual(warm[1][""],cold_family[""],"observer setup changed main DB bytes/permissions")
        try:
            yield warm,diagnostic
        finally:
            test.assertEqual(_host_private_database_image(path),warm,
                "CLI changed the complete warm anchor image: "+repr(diagnostic))
    finally:
        observer.close()
        closed=_host_private_database_image(path)
        diagnostic["closed"]={suffix:{"mode":mode,"bytes":len(raw),"sha256":hashlib.sha256(raw).hexdigest()}
            for suffix,(mode,raw) in closed[1].items()}
        test.assertEqual(closed,cold,"observer close did not restore the cold full family: "+repr(diagnostic))


def _host_backup_image(path):
    image={}
    for member in sorted(Path(path).rglob("*")):
        assert not member.is_symlink()
        image[str(member.relative_to(path))]=(member.is_dir(),member.stat().st_mode,None if member.is_dir() else member.read_bytes())
    return image


def _host_rewrite_backup(backup,identity,wire):
    database=backup/"rsia.sqlite3"
    con=connect(database)
    try:
        changed=con.execute("UPDATE objects SET body=? WHERE namespace='name-coverage' AND kind='artifact' AND id=?",(wire,identity))
        assert changed.rowcount==1
    finally:
        con.close()
    manifest_path=backup/"backup-manifest.json"
    manifest=json.loads(manifest_path.read_text())
    raw=database.read_bytes()
    manifest["database"]["sha256"]=hashlib.sha256(raw).hexdigest()
    manifest["database"]["bytes"]=len(raw)
    manifest_path.write_text(json.dumps(manifest,indent=2)+"\n")


# Actual Rust producer bytes, including all six exported fields.
RUST_CURRENT_HOST_MARKER_BYTES = b'{"manifest_wire":"{\\"schema_version\\":\\"rsia.host_surface.v2\\",\\"host\\":\\"reference\\",\\"host_version\\":\\"0.1.0\\",\\"adapter_version\\":\\"r8-9\\",\\"source_kind\\":\\"reviewed_fixture\\",\\"source_schema\\":\\"rsia.host_reviewed_declarations.v1\\",\\"extractor_version\\":\\"rsia.host_reviewed_declarations.extractor.v1\\",\\"source_envelope_digest\\":\\"0ccd87b4fc71dd55c8cdfa719712d6beb2cbd62f278df453cfdfb1a7eae9e137\\",\\"source_digest\\":\\"c156ec7bdc91a14c3e4b35dad968ccfe48dfc81ce9d75bcf2145abb231d482f5\\",\\"items\\":[{\\"name\\":\\"enabled\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"bool\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":true}},{\\"name\\":\\"disabled\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"bool\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":false}},{\\"name\\":\\"count\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"integer\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":1}},{\\"name\\":\\"mode\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"enum\\",\\"enum_values\\":[\\"a\\",\\"b\\"],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":\\"b\\"}},{\\"name\\":\\"reserved\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":null},\\"kind\\":\\"value\\",\\"value\\":null}}},{\\"name\\":\\"numbers\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"exponent_high\\":1e+16,\\"exponent_low\\":1e-6,\\"fixed_high\\":1000000000000000.0,\\"fixed_low\\":0.00001,\\"flags\\":[true,false,{\\"on\\":true}],\\"fraction\\":0.1,\\"large_int\\":1e+21,\\"negative_zero\\":-0.0,\\"one\\":1.0,\\"zero\\":-0.0}}},{\\"name\\":\\"boundary\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"string\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":\\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\"}},{\\"name\\":\\"compact_numbers\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"n\\":[1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16]}}},{\\"name\\":\\"raw_zeros\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"zero_0\\":0.0,\\"zero_1\\":-0.0,\\"zero_10\\":0.0,\\"zero_11\\":-0.0,\\"zero_2\\":0.0,\\"zero_3\\":-0.0,\\"zero_4\\":0.0,\\"zero_5\\":-0.0,\\"zero_6\\":0.0,\\"zero_7\\":-0.0,\\"zero_8\\":0.0,\\"zero_9\\":-0.0}}}],\\"retired\\":[]}","number_wires":[{"bits":"0000000000000000","wire":"0.0"},{"bits":"0000000000000001","wire":"5e-324"},{"bits":"0000000000000002","wire":"1e-323"},{"bits":"000fffffffffffff","wire":"2.225073858507201e-308"},{"bits":"0010000000000000","wire":"2.2250738585072014e-308"},{"bits":"0010000000000001","wire":"2.225073858507202e-308"},{"bits":"3eb0c6f7a0b5ed8c","wire":"9.999999999999997e-7"},{"bits":"3eb0c6f7a0b5ed8d","wire":"1e-6"},{"bits":"3eb0c6f7a0b5ed8e","wire":"1.0000000000000002e-6"},{"bits":"3ee4f8b588e368f0","wire":"9.999999999999999e-6"},{"bits":"3ee4f8b588e368f1","wire":"0.00001"},{"bits":"3ee4f8b588e368f2","wire":"0.000010000000000000003"},{"bits":"3fb9999999999999","wire":"0.09999999999999999"},{"bits":"3fb999999999999a","wire":"0.1"},{"bits":"3fb999999999999b","wire":"0.10000000000000002"},{"bits":"3feb0e7009b61cdf","wire":"0.84551240822557"},{"bits":"3feb0e7009b61ce0","wire":"0.8455124082255701"},{"bits":"3feb0e7009b61ce1","wire":"0.8455124082255702"},{"bits":"3fefffffffffffff","wire":"0.9999999999999999"},{"bits":"3ff0000000000000","wire":"1.0"},{"bits":"3ff0000000000001","wire":"1.0000000000000002"},{"bits":"3ff0000000000002","wire":"1.0000000000000004"},{"bits":"3ff3c0ca428c59fa","wire":"1.2345678901234565"},{"bits":"3ff3c0ca428c59fb","wire":"1.2345678901234567"},{"bits":"3ff3c0ca428c59fc","wire":"1.234567890123457"},{"bits":"4003ffffffffffff","wire":"2.4999999999999996"},{"bits":"4004000000000000","wire":"2.5"},{"bits":"4004000000000001","wire":"2.5000000000000004"},{"bits":"430c6bf52633ffff","wire":"999999999999999.9"},{"bits":"430c6bf526340000","wire":"1000000000000000.0"},{"bits":"430c6bf526340001","wire":"1000000000000000.1"},{"bits":"4341c37937e07fff","wire":"9999999999999998.0"},{"bits":"4341c37937e08000","wire":"1e+16"},{"bits":"4341c37937e08001","wire":"1.0000000000000002e+16"},{"bits":"444b1ae4d6e2ef4f","wire":"9.999999999999999e+20"},{"bits":"444b1ae4d6e2ef50","wire":"1e+21"},{"bits":"444b1ae4d6e2ef51","wire":"1.0000000000000001e+21"},{"bits":"44b52d02c7e14af5","wire":"9.999999999999997e+22"},{"bits":"44b52d02c7e14af6","wire":"1e+23"},{"bits":"44b52d02c7e14af7","wire":"1.0000000000000001e+23"},{"bits":"7feffffffffffffe","wire":"1.7976931348623155e+308"},{"bits":"7fefffffffffffff","wire":"1.7976931348623157e+308"},{"bits":"8000000000000000","wire":"-0.0"},{"bits":"8000000000000001","wire":"-5e-324"},{"bits":"8000000000000002","wire":"-1e-323"},{"bits":"800fffffffffffff","wire":"-2.225073858507201e-308"},{"bits":"8010000000000000","wire":"-2.2250738585072014e-308"},{"bits":"8010000000000001","wire":"-2.225073858507202e-308"},{"bits":"beb0c6f7a0b5ed8c","wire":"-9.999999999999997e-7"},{"bits":"beb0c6f7a0b5ed8d","wire":"-1e-6"},{"bits":"beb0c6f7a0b5ed8e","wire":"-1.0000000000000002e-6"},{"bits":"bee4f8b588e368f0","wire":"-9.999999999999999e-6"},{"bits":"bee4f8b588e368f1","wire":"-0.00001"},{"bits":"bee4f8b588e368f2","wire":"-0.000010000000000000003"},{"bits":"bfb9999999999999","wire":"-0.09999999999999999"},{"bits":"bfb999999999999a","wire":"-0.1"},{"bits":"bfb999999999999b","wire":"-0.10000000000000002"},{"bits":"bfeb0e7009b61cdf","wire":"-0.84551240822557"},{"bits":"bfeb0e7009b61ce0","wire":"-0.8455124082255701"},{"bits":"bfeb0e7009b61ce1","wire":"-0.8455124082255702"},{"bits":"bfefffffffffffff","wire":"-0.9999999999999999"},{"bits":"bff0000000000000","wire":"-1.0"},{"bits":"bff0000000000001","wire":"-1.0000000000000002"},{"bits":"bff0000000000002","wire":"-1.0000000000000004"},{"bits":"bff3c0ca428c59fa","wire":"-1.2345678901234565"},{"bits":"bff3c0ca428c59fb","wire":"-1.2345678901234567"},{"bits":"bff3c0ca428c59fc","wire":"-1.234567890123457"},{"bits":"c003ffffffffffff","wire":"-2.4999999999999996"},{"bits":"c004000000000000","wire":"-2.5"},{"bits":"c004000000000001","wire":"-2.5000000000000004"},{"bits":"c30c6bf52633ffff","wire":"-999999999999999.9"},{"bits":"c30c6bf526340000","wire":"-1000000000000000.0"},{"bits":"c30c6bf526340001","wire":"-1000000000000000.1"},{"bits":"c341c37937e07fff","wire":"-9999999999999998.0"},{"bits":"c341c37937e08000","wire":"-1e+16"},{"bits":"c341c37937e08001","wire":"-1.0000000000000002e+16"},{"bits":"c44b1ae4d6e2ef4f","wire":"-9.999999999999999e+20"},{"bits":"c44b1ae4d6e2ef50","wire":"-1e+21"},{"bits":"c44b1ae4d6e2ef51","wire":"-1.0000000000000001e+21"},{"bits":"c4b52d02c7e14af5","wire":"-9.999999999999997e+22"},{"bits":"c4b52d02c7e14af6","wire":"-1e+23"},{"bits":"c4b52d02c7e14af7","wire":"-1.0000000000000001e+23"},{"bits":"ffeffffffffffffe","wire":"-1.7976931348623155e+308"},{"bits":"ffefffffffffffff","wire":"-1.7976931348623157e+308"}],"raw_token_faults":["0e9223372036854775808","0e-9223372036854775809","0e00000000000000000009223372036854775808","0.0e-9223372036854775808","-0.0e-9223372036854775808","10e9223372036854775807","1e-1000000000000000000","1e1000000000000000000","0.100000000000000001","00e1","0e+","0.e1"],"raw_zero_tokens":["0e1000000000000000000","-0e1000000000000000000","0e-1000000000000000000","-0e-1000000000000000000","0e9223372036854775807","-0e9223372036854775807","0e-9223372036854775808","-0e-9223372036854775808","0e+00000000000000000009223372036854775807","-0e-00000000000000000009223372036854775808","0.0e9223372036854775807","-0.0e-9223372036854775807"],"record_wire":"{\\"id\\":\\"surface-name-coverage\\",\\"schema_version\\":\\"rsia.host_surface_record.v2\\",\\"manifest\\":{\\"schema_version\\":\\"rsia.host_surface.v2\\",\\"host\\":\\"reference\\",\\"host_version\\":\\"0.1.0\\",\\"adapter_version\\":\\"r8-9\\",\\"source_kind\\":\\"reviewed_fixture\\",\\"source_schema\\":\\"rsia.host_reviewed_declarations.v1\\",\\"extractor_version\\":\\"rsia.host_reviewed_declarations.extractor.v1\\",\\"source_envelope_digest\\":\\"0ccd87b4fc71dd55c8cdfa719712d6beb2cbd62f278df453cfdfb1a7eae9e137\\",\\"source_digest\\":\\"c156ec7bdc91a14c3e4b35dad968ccfe48dfc81ce9d75bcf2145abb231d482f5\\",\\"items\\":[{\\"name\\":\\"enabled\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"bool\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":true}},{\\"name\\":\\"disabled\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"bool\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":false}},{\\"name\\":\\"count\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"integer\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":1}},{\\"name\\":\\"mode\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"enum\\",\\"enum_values\\":[\\"a\\",\\"b\\"],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":\\"b\\"}},{\\"name\\":\\"reserved\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":null},\\"kind\\":\\"value\\",\\"value\\":null}}},{\\"name\\":\\"numbers\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"exponent_high\\":1e+16,\\"exponent_low\\":1e-6,\\"fixed_high\\":1000000000000000.0,\\"fixed_low\\":0.00001,\\"flags\\":[true,false,{\\"on\\":true}],\\"fraction\\":0.1,\\"large_int\\":1e+21,\\"negative_zero\\":-0.0,\\"one\\":1.0,\\"zero\\":-0.0}}},{\\"name\\":\\"boundary\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"string\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":\\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\"}},{\\"name\\":\\"compact_numbers\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"n\\":[1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16]}}},{\\"name\\":\\"raw_zeros\\",\\"coverage\\":\\"unsupported\\",\\"mapped_field\\":null,\\"consumer\\":null,\\"reason\\":\\"unmapped reviewed software fixture\\",\\"renamed_from\\":null,\\"test_ref\\":null,\\"capability\\":null,\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"zero_0\\":0.0,\\"zero_1\\":-0.0,\\"zero_10\\":0.0,\\"zero_11\\":-0.0,\\"zero_2\\":0.0,\\"zero_3\\":-0.0,\\"zero_4\\":0.0,\\"zero_5\\":-0.0,\\"zero_6\\":0.0,\\"zero_7\\":-0.0,\\"zero_8\\":0.0,\\"zero_9\\":-0.0}}}],\\"retired\\":[]},\\"manifest_digest\\":\\"c46ac130d4771e559c428b29d4f15ee68f89c951d3e1bfdd97eff02388d24b52\\",\\"source\\":{\\"schema_version\\":\\"rsia.host_source_envelope.v1\\",\\"source_kind\\":\\"reviewed_fixture\\",\\"source_schema\\":\\"rsia.host_reviewed_declarations.v1\\",\\"extractor_version\\":\\"rsia.host_reviewed_declarations.extractor.v1\\",\\"original_utf8\\":\\"{\\\\\\"declarations\\\\\\":[{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":true},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"enabled\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"bool\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":false},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"disabled\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"bool\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":1},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"count\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"integer\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":\\\\\\"b\\\\\\"},\\\\\\"enum_values\\\\\\":[\\\\\\"a\\\\\\",\\\\\\"b\\\\\\"],\\\\\\"name\\\\\\":\\\\\\"mode\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"enum\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":null},\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":null}},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"reserved\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"object\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":{\\\\\\"exponent_high\\\\\\":1e+16,\\\\\\"exponent_low\\\\\\":1e-6,\\\\\\"fixed_high\\\\\\":1000000000000000.0,\\\\\\"fixed_low\\\\\\":0.00001,\\\\\\"flags\\\\\\":[true,false,{\\\\\\"on\\\\\\":true}],\\\\\\"fraction\\\\\\":0.1,\\\\\\"large_int\\\\\\":1000000000000000000000,\\\\\\"negative_zero\\\\\\":-0,\\\\\\"one\\\\\\":1.0,\\\\\\"zero\\\\\\":-0.0}},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"numbers\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"object\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":\\\\\\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\\\\\"},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"boundary\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"string\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":{\\\\\\"n\\\\\\":[1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16]}},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"compact_numbers\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"object\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":{\\\\\\"zero_0\\\\\\":0e1000000000000000000,\\\\\\"zero_1\\\\\\":-0e1000000000000000000,\\\\\\"zero_10\\\\\\":0.0e9223372036854775807,\\\\\\"zero_11\\\\\\":-0.0e-9223372036854775807,\\\\\\"zero_2\\\\\\":0e-1000000000000000000,\\\\\\"zero_3\\\\\\":-0e-1000000000000000000,\\\\\\"zero_4\\\\\\":0e9223372036854775807,\\\\\\"zero_5\\\\\\":-0e9223372036854775807,\\\\\\"zero_6\\\\\\":0e-9223372036854775808,\\\\\\"zero_7\\\\\\":-0e-9223372036854775808,\\\\\\"zero_8\\\\\\":0e+00000000000000000009223372036854775807,\\\\\\"zero_9\\\\\\":-0e-00000000000000000009223372036854775808}},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"raw_zeros\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"object\\\\\\"}],\\\\\\"host\\\\\\":\\\\\\"reference\\\\\\",\\\\\\"host_version\\\\\\":\\\\\\"0.1.0\\\\\\",\\\\\\"schema_version\\\\\\":\\\\\\"rsia.host_reviewed_declarations.v1\\\\\\"}\\"},\\"extracted\\":[\\"enabled\\",\\"disabled\\",\\"count\\",\\"mode\\",\\"reserved\\",\\"numbers\\",\\"boundary\\",\\"compact_numbers\\",\\"raw_zeros\\"],\\"extracted_typed\\":[{\\"name\\":\\"enabled\\",\\"value_type\\":\\"bool\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":true}},{\\"name\\":\\"disabled\\",\\"value_type\\":\\"bool\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":false}},{\\"name\\":\\"count\\",\\"value_type\\":\\"integer\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":1}},{\\"name\\":\\"mode\\",\\"value_type\\":\\"enum\\",\\"enum_values\\":[\\"a\\",\\"b\\"],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":\\"b\\"}},{\\"name\\":\\"reserved\\",\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":null},\\"kind\\":\\"value\\",\\"value\\":null}}},{\\"name\\":\\"numbers\\",\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"exponent_high\\":1e+16,\\"exponent_low\\":1e-6,\\"fixed_high\\":1000000000000000.0,\\"fixed_low\\":0.00001,\\"flags\\":[true,false,{\\"on\\":true}],\\"fraction\\":0.1,\\"large_int\\":1e+21,\\"negative_zero\\":-0.0,\\"one\\":1.0,\\"zero\\":-0.0}}},{\\"name\\":\\"boundary\\",\\"value_type\\":\\"string\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":\\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\"}},{\\"name\\":\\"compact_numbers\\",\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"n\\":[1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16]}}},{\\"name\\":\\"raw_zeros\\",\\"value_type\\":\\"object\\",\\"enum_values\\":[],\\"default\\":{\\"kind\\":\\"value\\",\\"value\\":{\\"zero_0\\":0.0,\\"zero_1\\":-0.0,\\"zero_10\\":0.0,\\"zero_11\\":-0.0,\\"zero_2\\":0.0,\\"zero_3\\":-0.0,\\"zero_4\\":0.0,\\"zero_5\\":-0.0,\\"zero_6\\":0.0,\\"zero_7\\":-0.0,\\"zero_8\\":0.0,\\"zero_9\\":-0.0}}}],\\"predecessor_surface_id\\":null,\\"upgrade_diff\\":{\\"added\\":[\\"boundary\\",\\"compact_numbers\\",\\"count\\",\\"disabled\\",\\"enabled\\",\\"mode\\",\\"numbers\\",\\"raw_zeros\\",\\"reserved\\"],\\"removed\\":[],\\"renamed\\":[],\\"type_changed\\":[],\\"enum_changed\\":[],\\"default_changed\\":[],\\"mapping_changed\\":[],\\"legacy_untyped\\":[]}}","source_wire":"{\\"schema_version\\":\\"rsia.host_source_envelope.v1\\",\\"source_kind\\":\\"reviewed_fixture\\",\\"source_schema\\":\\"rsia.host_reviewed_declarations.v1\\",\\"extractor_version\\":\\"rsia.host_reviewed_declarations.extractor.v1\\",\\"original_utf8\\":\\"{\\\\\\"declarations\\\\\\":[{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":true},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"enabled\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"bool\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":false},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"disabled\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"bool\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":1},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"count\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"integer\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":\\\\\\"b\\\\\\"},\\\\\\"enum_values\\\\\\":[\\\\\\"a\\\\\\",\\\\\\"b\\\\\\"],\\\\\\"name\\\\\\":\\\\\\"mode\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"enum\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":null},\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":null}},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"reserved\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"object\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":{\\\\\\"exponent_high\\\\\\":1e+16,\\\\\\"exponent_low\\\\\\":1e-6,\\\\\\"fixed_high\\\\\\":1000000000000000.0,\\\\\\"fixed_low\\\\\\":0.00001,\\\\\\"flags\\\\\\":[true,false,{\\\\\\"on\\\\\\":true}],\\\\\\"fraction\\\\\\":0.1,\\\\\\"large_int\\\\\\":1000000000000000000000,\\\\\\"negative_zero\\\\\\":-0,\\\\\\"one\\\\\\":1.0,\\\\\\"zero\\\\\\":-0.0}},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"numbers\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"object\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":\\\\\\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\\\\\"},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"boundary\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"string\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":{\\\\\\"n\\\\\\":[1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16,1e+16]}},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"compact_numbers\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"object\\\\\\"},{\\\\\\"default\\\\\\":{\\\\\\"kind\\\\\\":\\\\\\"value\\\\\\",\\\\\\"value\\\\\\":{\\\\\\"zero_0\\\\\\":0e1000000000000000000,\\\\\\"zero_1\\\\\\":-0e1000000000000000000,\\\\\\"zero_10\\\\\\":0.0e9223372036854775807,\\\\\\"zero_11\\\\\\":-0.0e-9223372036854775807,\\\\\\"zero_2\\\\\\":0e-1000000000000000000,\\\\\\"zero_3\\\\\\":-0e-1000000000000000000,\\\\\\"zero_4\\\\\\":0e9223372036854775807,\\\\\\"zero_5\\\\\\":-0e9223372036854775807,\\\\\\"zero_6\\\\\\":0e-9223372036854775808,\\\\\\"zero_7\\\\\\":-0e-9223372036854775808,\\\\\\"zero_8\\\\\\":0e+00000000000000000009223372036854775807,\\\\\\"zero_9\\\\\\":-0e-00000000000000000009223372036854775808}},\\\\\\"enum_values\\\\\\":[],\\\\\\"name\\\\\\":\\\\\\"raw_zeros\\\\\\",\\\\\\"value_type\\\\\\":\\\\\\"object\\\\\\"}],\\\\\\"host\\\\\\":\\\\\\"reference\\\\\\",\\\\\\"host_version\\\\\\":\\\\\\"0.1.0\\\\\\",\\\\\\"schema_version\\\\\\":\\\\\\"rsia.host_reviewed_declarations.v1\\\\\\"}\\"}"}'


def _rust_current_host_restore_matrix(payload):
    """Consume this test run's real Rust producer/backup; never mint its digest.

    The Rust integration test owns every supplied path and closes the Store
    first. Stage B will also call this helper from a single Python top-level
    test with independently checked Rust raw constants.
    """
    test=RestoreTests()
    backup=Path(payload["backup"]);anchor=Path(payload["anchor"])
    identity=payload["identity"];wire=payload["record_wire"]
    manifest=json.loads((backup/"backup-manifest.json").read_text())
    test.assertTrue(manifest["watermarks"])
    test.assertIn("name-coverage",{mark["namespace"] for mark in manifest["watermarks"]})
    for entry in payload["number_wires"]:
        number=struct.unpack(">d",bytes.fromhex(entry["bits"]))[0]
        test.assertEqual(restore.host_json_wire(number),entry["wire"],entry["bits"])
    current=restore.read_current_host_raw(wire,"name-coverage","artifact",identity)
    if payload["typed_matrix"]:
        actual=json.loads(RUST_CURRENT_HOST_MARKER_BYTES)
        test.assertEqual(wire,actual["record_wire"])
        test.assertEqual(payload["number_wires"],actual["number_wires"])
        test.assertEqual(payload["raw_token_faults"],actual["raw_token_faults"])
        test.assertEqual(restore.host_json_wire(restore.host_ordered(current["source"],restore.HOST_SOURCE_FIELDS)),actual["source_wire"])
        test.assertEqual(restore.host_json_wire(restore.host_ordered(current["manifest"],restore.HOST_MANIFEST_FIELDS)),actual["manifest_wire"])
        for index,token in enumerate(actual["raw_zero_tokens"]):
            test.assertEqual(current["source"]["original_utf8"].count('"zero_'+str(index)+'":'+token),1)
    with _host_anchor_read_observer(test,anchor) as (anchor_image,observer):
        backup_image=_host_private_database_image(backup/"rsia.sqlite3")
        backup_files=_host_backup_image(backup)
        con=restore.readonly(backup/"rsia.sqlite3")
        try:
            test.assertEqual(con.execute("SELECT body FROM objects WHERE namespace='name-coverage' AND kind='artifact' AND id=?",(identity,)).fetchone(),(wire,))
        finally:
            con.close()
        test.assertEqual(_host_private_database_image(backup/"rsia.sqlite3"),backup_image)
        scenario=Scenario.__new__(Scenario);scenario.base=backup.parent
        dest,result=scenario.restore(backup,anchor,"rust-host-restored")
        test.assert_restored(dest,result,0,"none")
        test.assertEqual(_host_private_database_image(dest/"rsia.sqlite3")[0],backup_image[0])
        test.assertEqual(_host_private_database_image(anchor),anchor_image)
        test.assertEqual(_host_private_database_image(backup/"rsia.sqlite3"),backup_image)
        test.assertEqual(_host_backup_image(backup),backup_files)
        original=json.loads(wire)
        faults=[];equivalents=[]
        if payload["typed_matrix"]:
            def changed(): return json.loads(wire)
            def declarations(body): return {item["name"]:item for item in body["extracted_typed"]}
            for name,replacement in (("enabled",1),("disabled",0)):
                body=changed();declarations(body)[name]["default"]["value"]=replacement
                faults.append((name+"-as-number",restore.host_json_wire(body)))
            for position,replacement in ((0,1),(1,0)):
                body=changed();declarations(body)["numbers"]["default"]["value"]["flags"][position]=replacement
                faults.append(("nested-bool-"+str(position),restore.host_json_wire(body)))
            body=changed();declarations(body)["numbers"]["default"]["value"]["flags"][2]["on"]=1
            faults.append(("object-bool-as-number",restore.host_json_wire(body)))
            for key,replacement in (("one",True),("zero",False)):
                body=changed();declarations(body)["numbers"]["default"]["value"][key]=replacement
                faults.append(("number-as-bool-"+key,restore.host_json_wire(body)))
            body=changed();declarations(body)["count"]["default"]["value"]=1.0
            faults.append(("integer-as-float",restore.host_json_wire(body)))
            body=changed();declarations(body)["enabled"]["default"]["value"]=None
            faults.append(("real-default-null",restore.host_json_wire(body)))
            body=changed();declarations(body)["enabled"]["extra"]=None
            faults.append(("extra-typed-key",restore.host_json_wire(body)))
            body=changed();declarations(body)["enabled"].pop("enum_values")
            faults.append(("missing-typed-key",restore.host_json_wire(body)))
            body=changed();declarations(body)["enabled"]["value_type"]="integer"
            faults.append(("wrong-typed-type",restore.host_json_wire(body)))
            body=changed();declarations(body)["mode"]["enum_values"].append("c")
            faults.append(("wrong-typed-enum",restore.host_json_wire(body)))
            body=changed();declarations(body)["boundary"]["default"]["value"]+="x"
            faults.append(("default-4097",restore.host_json_wire(body)))
            body=changed();declarations(body)["compact_numbers"]["default"]["value"]["n"].append(1e16)
            faults.append(("numeric-default-4097",restore.host_json_wire(body)))
            for name,replacement in (("source-precision",'"fraction":0.100000000000000001'),("source-duplicate",'"fraction":0.1,"\\u0066raction":0.1')):
                body=changed();raw=body["source"]["original_utf8"]
                body["source"]["original_utf8"]=raw.replace('"fraction":0.1',replacement,1)
                test.assertNotEqual(body["source"]["original_utf8"],raw)
                faults.append((name,restore.host_json_wire(body)))
            faults.append(("outer-precision",wire.replace('"fraction":0.1','"fraction":0.100000000000000001',1)))
            for index,token in enumerate(payload.get("raw_token_faults",[])):
                body=changed();raw=body["source"]["original_utf8"]
                body["source"]["original_utf8"]=raw.replace('"zero_0":0e1000000000000000000','"zero_0":'+token,1)
                test.assertNotEqual(body["source"]["original_utf8"],raw)
                with test.assertRaisesRegex(restore.IsolationError,"^invalid current host raw JSON$"):
                    restore.read_host_json_tokens(body["source"]["original_utf8"])
                for field in ("manifest","manifest_digest","extracted","extracted_typed"):
                    test.assertEqual(restore.host_json_wire(body[field]),restore.host_json_wire(original[field]))
                faults.append(("raw-token-"+str(index),restore.host_json_wire(body)))
            body=changed();number_default=declarations(body)["numbers"]["default"]["value"]
            number_default["one"]=1;number_default["zero"]=0.0
            declarations(body)["numbers"]["default"]["value"]={key:number_default[key] for key in reversed(number_default)}
            equivalents.append(("number-value-and-object-order",restore.host_json_wire(body)))
        if payload["resource_fault"] is not None:
            faults.append(("final-record-resource",payload["resource_fault"]))
        equivalent_destinations=[]
        for name,raw in equivalents:
            body=json.loads(raw)
            for field in ("source","manifest","manifest_digest"):
                test.assertEqual(restore.host_json_wire(body[field]),restore.host_json_wire(original[field]))
            copy=backup.parent/("equivalent-"+name);shutil.copytree(backup,copy)
            _host_rewrite_backup(copy,identity,raw)
            before=_host_private_database_image(copy/"rsia.sqlite3")
            before_files=_host_backup_image(copy)
            equivalent,result=scenario.restore(copy,anchor,"restored-"+name)
            test.assert_restored(equivalent,result,0,"none")
            test.assertEqual(_host_private_database_image(equivalent/"rsia.sqlite3")[0],before[0])
            test.assertEqual(_host_private_database_image(copy/"rsia.sqlite3"),before)
            test.assertEqual(_host_backup_image(copy),before_files)
            test.assertEqual(_host_private_database_image(anchor),anchor_image)
            equivalent_destinations.append(str(equivalent))
        for name,raw in faults:
            test.assertNotEqual(raw,wire,name)
            if name not in ("source-precision","source-duplicate","outer-precision","final-record-resource") and not name.startswith("raw-token-"):
                body=json.loads(raw)
                for field in ("source","manifest","manifest_digest"):
                    test.assertEqual(restore.host_json_wire(body[field]),restore.host_json_wire(original[field]),name)
            copy=backup.parent/("damaged-"+name);shutil.copytree(backup,copy)
            _host_rewrite_backup(copy,identity,raw)
            before=_host_private_database_image(copy/"rsia.sqlite3")
            before_files=_host_backup_image(copy)
            refused,result=scenario.restore(copy,anchor,"refused-"+name)
            test.assert_isolated(scenario,refused,result,"invalid current host raw JSON")
            test.assertEqual(_host_private_database_image(copy/"rsia.sqlite3"),before)
            test.assertEqual(_host_backup_image(copy),before_files)
            test.assertEqual(_host_private_database_image(anchor),anchor_image)
            test.assertEqual(_host_private_database_image(backup/"rsia.sqlite3"),backup_image)
            test.assertEqual(_host_backup_image(backup),backup_files)
    return {"restored":str(dest),"equivalents":equivalent_destinations,"isolated":len(faults),"anchor_observer":observer}


def _rust_current_host_text_restore_matrix(payload):
    """Separate content-only text cases from the original canonical/run matrix.

    Every healthy row and malformed row's derived digests come from the actual
    Rust producer/typed serializer in this test run, never a Python auth factory.
    """
    test=RestoreTests()
    backup=Path(payload["backup"]);anchor=Path(payload["anchor"])
    records=payload["records"];faults=payload["faults"]
    test.assertTrue(records);test.assertTrue(faults)
    test.assertEqual(len(records),30);test.assertEqual(len(faults),162)
    manifest=json.loads((backup/"backup-manifest.json").read_text())
    test.assertIn("name-coverage",{row["namespace"] for row in manifest["watermarks"]})
    with _host_anchor_read_observer(test,anchor) as (anchor_image,observer):
        backup_image=_host_private_database_image(backup/"rsia.sqlite3")
        backup_files=_host_backup_image(backup)
        con=restore.readonly(backup/"rsia.sqlite3")
        try:
            for row in records:
                wire=row["wire"];identity=row["id"]
                restore.read_current_host_raw(wire,"name-coverage","artifact",identity)
                test.assertEqual(con.execute("SELECT body FROM objects WHERE namespace='name-coverage' AND kind='artifact' AND id=?",(identity,)).fetchone(),(wire,))
        finally:
            con.close()
        scenario=Scenario.__new__(Scenario);scenario.base=backup.parent
        dest,result=scenario.restore(backup,anchor,"text-content-restored")
        test.assert_restored(dest,result,0,"none")
        test.assertEqual(_host_private_database_image(dest/"rsia.sqlite3")[0],backup_image[0])
        test.assertEqual(_host_private_database_image(anchor),anchor_image)
        test.assertEqual(_host_private_database_image(backup/"rsia.sqlite3"),backup_image)
        test.assertEqual(_host_backup_image(backup),backup_files)
        originals={row["id"]:json.loads(row["wire"]) for row in records}
        original_wires={row["id"]:row["wire"] for row in records}
        text_count=0;shape_count=0
        for index,fault in enumerate(faults):
            wire=fault["wire"];identity=fault["id"];body=json.loads(wire);original=originals[identity]
            test.assertNotEqual(wire,restore.host_json_wire(original),fault["name"])
            for field in ("id","schema_version","extracted","extracted_typed","predecessor_surface_id","upgrade_diff"):
                test.assertEqual(restore.host_json_wire(body[field]),restore.host_json_wire(original[field]),fault["name"])
            if fault["field"]=="retired_shape":
                shape_count+=1
                test.assertIn(fault["shape"],("object","string"))
                replacement='"retired":{}' if fault["shape"]=="object" else '"retired":""'
                expected={} if fault["shape"]=="object" else ""
                test.assertIs(type(body["manifest"]["retired"]),type(expected))
                test.assertEqual(body["manifest"]["retired"],expected)
                test.assertEqual(original["manifest"]["retired"],[])
                test.assertIsNone(original["predecessor_surface_id"])
                test.assertEqual(original["extracted"],["old-setting"])
                test.assertEqual(original_wires[identity].count('"retired":[]'),1)
                test.assertEqual(wire,original_wires[identity].replace('"retired":[]',replacement,1))
                test.assertEqual(body["manifest_digest"],original["manifest_digest"])
                test.assertEqual(restore.host_digest(restore.host_ordered(original["manifest"],restore.HOST_MANIFEST_FIELDS)),original["manifest_digest"])
                for field in ("source_digest","source_envelope_digest"):
                    test.assertEqual(body["manifest"][field],original["manifest"][field])
                repaired=json.loads(wire);repaired["manifest"]["retired"]=[]
                test.assertEqual(repaired,original)
            else:
                text_count+=1
                # Explicitly prove the target text is reached with consistent hashes;
                # a stale manifest/source digest cannot masquerade as a text refusal.
                source=restore.host_ordered(body["source"],restore.HOST_SOURCE_FIELDS)
                ordered=restore.host_ordered(body["manifest"],restore.HOST_MANIFEST_FIELDS)
                test.assertEqual(body["manifest_digest"],restore.host_digest(ordered),fault["name"])
                test.assertEqual(ordered["source_envelope_digest"],restore.host_digest(source),fault["name"])
                test.assertEqual(ordered["source_digest"],sha256_text(source["original_utf8"]),fault["name"])
                if fault["field"]!="host_version":
                    test.assertEqual(restore.host_json_wire(body["source"]),restore.host_json_wire(original["source"]),fault["name"])
                else:
                    before=json.loads(original["source"]["original_utf8"])
                    after=json.loads(source["original_utf8"])
                    test.assertNotEqual(after["host_version"],before["host_version"])
                    test.assertEqual(after["host_version"],ordered["host_version"])
                    after.pop("host_version");before.pop("host_version")
                    test.assertEqual(after,before)
            with test.assertRaisesRegex(restore.IsolationError,"^invalid current host raw JSON$"):
                restore.read_current_host_raw(wire,"name-coverage","artifact",identity)
            copy=backup.parent/("text-damaged-"+str(index));shutil.copytree(backup,copy)
            _host_rewrite_backup(copy,identity,wire)
            before=_host_private_database_image(copy/"rsia.sqlite3");before_files=_host_backup_image(copy)
            refused,result=scenario.restore(copy,anchor,"text-refused-"+str(index))
            test.assert_isolated(scenario,refused,result,"invalid current host raw JSON")
            test.assertEqual(_host_private_database_image(copy/"rsia.sqlite3"),before)
            test.assertEqual(_host_backup_image(copy),before_files)
            test.assertEqual(_host_private_database_image(anchor),anchor_image)
            test.assertEqual(_host_private_database_image(backup/"rsia.sqlite3"),backup_image)
            test.assertEqual(_host_backup_image(backup),backup_files)
        test.assertEqual(text_count,160);test.assertEqual(shape_count,2)
    return {"restored":str(dest),"accepted":len(records),"isolated":len(faults),"text_isolated":text_count,"shape_isolated":shape_count,"anchor_observer":observer}


def _rust_current_host_number_admission_matrix(payload):
    """Compare pinned raw admission and restore this run's real number rows."""
    test=RestoreTests()
    raw_accepted=0;raw_refused=0
    for row in payload["raw_cases"]:
        raw='{"schema_version":"rsia.host_surface_record.v2","n":'+row["token"]+'}'
        if row["accepted"]:
            number=restore.read_host_json_tokens(raw)["n"]
            test.assertIs(type(number),float if row["number_kind"]=="float" else int)
            test.assertEqual(restore.host_json_wire(number),row["number_wire"],row["token"])
            raw_accepted+=1
        else:
            with test.assertRaisesRegex(restore.IsolationError,"^invalid current host raw JSON$"):
                restore.read_host_json_tokens(raw)
            raw_refused+=1
    test.assertEqual(raw_accepted,12);test.assertEqual(raw_refused,9)
    records=payload["records"];variants=payload["variants"]
    test.assertEqual(len(records),2)
    test.assertEqual([(row["field"],row["old_token"],row["new_token"],row["accepted"]) for row in variants],
        [("safe","1","1.0",True),("n","8000000000000004","8000000000000004.0",False)])
    backup=Path(payload["backup"]);anchor=Path(payload["anchor"])
    manifest=json.loads((backup/"backup-manifest.json").read_text())
    test.assertIn("name-coverage",{row["namespace"] for row in manifest["watermarks"]})
    with _host_anchor_read_observer(test,anchor) as (anchor_image,observer):
        backup_image=_host_private_database_image(backup/"rsia.sqlite3")
        backup_files=_host_backup_image(backup)
        con=restore.readonly(backup/"rsia.sqlite3")
        try:
            for row in records:
                restore.read_current_host_raw(row["wire"],"name-coverage","artifact",row["id"])
                test.assertEqual(con.execute("SELECT body FROM objects WHERE namespace='name-coverage' AND kind='artifact' AND id=?",(row["id"],)).fetchone(),(row["wire"],))
        finally:
            con.close()
        scenario=Scenario.__new__(Scenario);scenario.base=backup.parent
        dest,result=scenario.restore(backup,anchor,"number-content-restored")
        test.assert_restored(dest,result,0,"none")
        test.assertEqual(_host_private_database_image(dest/"rsia.sqlite3")[0],backup_image[0])
        test.assertEqual(_host_private_database_image(anchor),anchor_image)
        test.assertEqual(_host_private_database_image(backup/"rsia.sqlite3"),backup_image)
        test.assertEqual(_host_backup_image(backup),backup_files)
        original_wire=records[0]["wire"];original=json.loads(original_wire)
        typed_wire=payload["typed_wire"]
        test.assertEqual(json.loads(typed_wire),original["extracted_typed"])
        target='"extracted_typed":'+typed_wire
        test.assertEqual(original_wire.count(target),1)
        test.assertIs(type(original["extracted_typed"][0]["default"]["value"]["n"]),int)
        test.assertEqual(original["extracted_typed"][0]["default"]["value"]["n"],8000000000000004)
        test.assertEqual(original["manifest_digest"],restore.host_digest(restore.host_ordered(original["manifest"],restore.HOST_MANIFEST_FIELDS)))
        test.assertEqual(original["manifest"]["source_digest"],sha256_text(original["source"]["original_utf8"]))
        test.assertEqual(original["manifest"]["source_envelope_digest"],restore.host_digest(restore.host_ordered(original["source"],restore.HOST_SOURCE_FIELDS)))
        equivalent=None;equivalent_count=0;isolated=0
        for index,row in enumerate(variants):
            identity=row["id"];wire=row["wire"]
            test.assertEqual(identity,records[0]["id"])
            old='"'+row["field"]+'":'+row["old_token"]
            new='"'+row["field"]+'":'+row["new_token"]
            test.assertEqual(typed_wire.count(old),1)
            changed=typed_wire.replace(old,new,1)
            test.assertEqual(wire,original_wire.replace(target,'"extracted_typed":'+changed,1))
            body=json.loads(wire)
            for field in ("id","schema_version","source","manifest","manifest_digest","extracted","predecessor_surface_id","upgrade_diff"):
                test.assertEqual(body[field],original[field])
            test.assertIs(type(body["extracted_typed"][0]["default"]["value"][row["field"]]),float)
            if row["accepted"]:
                restore.read_current_host_raw(wire,"name-coverage","artifact",identity)
            else:
                with test.assertRaisesRegex(restore.IsolationError,"^invalid current host raw JSON$"):
                    restore.read_current_host_raw(wire,"name-coverage","artifact",identity)
            copy=backup.parent/("number-variant-"+str(index));shutil.copytree(backup,copy)
            _host_rewrite_backup(copy,identity,wire)
            before=_host_private_database_image(copy/"rsia.sqlite3");before_files=_host_backup_image(copy)
            restored,result=scenario.restore(copy,anchor,"number-result-"+str(index))
            if row["accepted"]:
                test.assert_restored(restored,result,0,"none")
                test.assertEqual(_host_private_database_image(restored/"rsia.sqlite3")[0],before[0])
                equivalent=str(restored)
                equivalent_count+=1
            else:
                test.assert_isolated(scenario,restored,result,"invalid current host raw JSON")
                isolated+=1
            test.assertEqual(_host_private_database_image(copy/"rsia.sqlite3"),before)
            test.assertEqual(_host_backup_image(copy),before_files)
            test.assertEqual(_host_private_database_image(anchor),anchor_image)
            test.assertEqual(_host_private_database_image(backup/"rsia.sqlite3"),backup_image)
            test.assertEqual(_host_backup_image(backup),backup_files)
        test.assertIsNotNone(equivalent);test.assertEqual(equivalent_count,1);test.assertEqual(isolated,1)
    return {"restored":str(dest),"accepted":len(records),"equivalent":equivalent,
        "equivalent_count":equivalent_count,"isolated":isolated,"raw_accepted":raw_accepted,"raw_refused":raw_refused,"anchor_observer":observer}


class RustCurrentHostWireTests(unittest.TestCase):
    def test_rust_current_host_wire_defaults_restore_and_typed_faults_isolate(self):
        actual=json.loads(RUST_CURRENT_HOST_MARKER_BYTES)
        self.assertEqual(set(actual),{"record_wire","source_wire","manifest_wire","number_wires","raw_zero_tokens","raw_token_faults"})
        record=json.loads(actual["record_wire"])
        scenario=Scenario(self,CURRENT)
        con=connect(scenario.history)
        try:
            # This fixture watermark binds its local anchor/backup, never a run grant.
            con.execute("INSERT INTO revoke_watermark VALUES(?,?,?)",
                ("name-coverage",1,sha256_text("static-rust-current-host-anchor-1")))
            con.execute("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,?,?,?,?)",
                ("name-coverage","artifact",record["id"],"trusted-actor",actual["record_wire"]))
        finally:
            con.close()
        anchor=scenario.anchor("static-rust-live")
        backup=scenario.backup("static-rust-backup")
        response=_rust_current_host_restore_matrix({
            "backup":str(backup),"anchor":str(anchor),"identity":record["id"],
            "record_wire":actual["record_wire"],"number_wires":actual["number_wires"],
            "raw_token_faults":actual["raw_token_faults"],"typed_matrix":True,"resource_fault":None,
        })
        self.assertEqual(response["isolated"],30)
        self.assertEqual(len(response["equivalents"]),1)
        self.assertTrue(Path(response["restored"]).is_dir())
        self.assertTrue(Path(response["equivalents"][0]).is_dir())

    def test_root_host_upgrade_metadata_without_predecessor_isolated(self):
        actual=json.loads(RUST_CURRENT_HOST_MARKER_BYTES)
        wire=actual["record_wire"];manifest_wire=actual["manifest_wire"]
        record=json.loads(wire)
        self.assertIsNone(record["predecessor_surface_id"])
        self.assertEqual(record["manifest"]["retired"],[])
        self.assertTrue(all(item["renamed_from"] is None for item in record["manifest"]["items"]))
        self.assertEqual(record["manifest"]["items"][0]["name"],"enabled")
        self.assertEqual(json.loads(manifest_wire),record["manifest"])
        self.assertEqual(wire.count(manifest_wire),1)
        self.assertEqual(manifest_wire.count('"retired":[]'),1)
        self.assertEqual(manifest_wire.count('"renamed_from":null'),9)
        self.assertEqual(record["manifest_digest"],sha256_text(manifest_wire))
        digest_token='"manifest_digest":"'+record["manifest_digest"]+'"'
        self.assertEqual(wire.count(digest_token),1)
        self.assertEqual(wire.count(actual["source_wire"]),1)
        scenario=Scenario(self,CURRENT)
        con=connect(scenario.history)
        try:
            con.execute("INSERT INTO revoke_watermark VALUES(?,?,?)",
                ("name-coverage",1,sha256_text("root-rust-current-host-anchor-1")))
            con.execute("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,?,?,?,?)",
                ("name-coverage","artifact",record["id"],"trusted-actor",wire))
        finally:
            con.close()
        anchor=scenario.anchor("root-host-live")
        backup=scenario.backup("root-host-backup")
        checks=RestoreTests()
        with _host_anchor_read_observer(self,anchor) as (anchor_image,observer):
            backup_image=_host_private_database_image(backup/"rsia.sqlite3")
            backup_files=_host_backup_image(backup)
            dest,result=scenario.restore(backup,anchor,"root-host-restored")
            checks.assert_restored(dest,result,0,"none")
            self.assertEqual(_host_private_database_image(dest/"rsia.sqlite3")[0],backup_image[0])
            self.assertEqual(_host_private_database_image(anchor),anchor_image)
            self.assertEqual(_host_private_database_image(backup/"rsia.sqlite3"),backup_image)
            self.assertEqual(_host_backup_image(backup),backup_files)
            for name,old,new in (
                ("retired",'"retired":[]','"retired":[{"name":"ghost","reason":"retired"}]'),
                ("renamed-from",'"renamed_from":null','"renamed_from":"ghost"'),
            ):
                with self.subTest(fault=name):
                    changed_manifest=manifest_wire.replace(old,new,1)
                    self.assertEqual(changed_manifest.replace(new,old,1),manifest_wire)
                    changed_digest=sha256_text(changed_manifest)
                    changed_token='"manifest_digest":"'+changed_digest+'"'
                    damaged=wire.replace(manifest_wire,changed_manifest,1).replace(digest_token,changed_token,1)
                    self.assertNotEqual(damaged,wire)
                    self.assertEqual(damaged.replace(changed_manifest,manifest_wire,1).replace(changed_token,digest_token,1),wire)
                    body=json.loads(damaged)
                    self.assertEqual(body["manifest_digest"],restore.host_digest(
                        restore.host_ordered(body["manifest"],restore.HOST_MANIFEST_FIELDS)))
                    self.assertEqual(damaged.count(actual["source_wire"]),1)
                    for field in ("id","schema_version","source","extracted","extracted_typed","predecessor_surface_id","upgrade_diff"):
                        self.assertEqual(body[field],record[field],name)
                    repaired=json.loads(changed_manifest)
                    if name=="retired":
                        self.assertEqual(repaired["retired"],[{"name":"ghost","reason":"retired"}])
                        repaired["retired"]=[]
                    else:
                        self.assertEqual(repaired["items"][0]["renamed_from"],"ghost")
                        repaired["items"][0]["renamed_from"]=None
                    self.assertEqual(repaired,record["manifest"])
                    copy=scenario.base/("root-damaged-"+name);shutil.copytree(backup,copy)
                    _host_rewrite_backup(copy,record["id"],damaged)
                    before=_host_private_database_image(copy/"rsia.sqlite3")
                    before_files=_host_backup_image(copy)
                    refused,result=scenario.restore(copy,anchor,"root-refused-"+name)
                    self.assertEqual(_host_private_database_image(copy/"rsia.sqlite3"),before)
                    self.assertEqual(_host_backup_image(copy),before_files)
                    self.assertEqual(_host_private_database_image(anchor),anchor_image)
                    self.assertEqual(_host_private_database_image(backup/"rsia.sqlite3"),backup_image)
                    self.assertEqual(_host_backup_image(backup),backup_files)
                    with self.assertRaisesRegex(restore.IsolationError,"^invalid current host raw JSON$",
                        msg=f"CLI rc={result.returncode} stdout={result.stdout!r} stderr={result.stderr!r}"):
                        restore.read_current_host_raw(damaged,"name-coverage","artifact",record["id"])
                    checks.assert_isolated(scenario,refused,result,"invalid current host raw JSON")


if __name__ == "__main__":
    unittest.main()
