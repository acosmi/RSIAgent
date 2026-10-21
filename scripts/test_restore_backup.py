"""Restore migration-prefix regressions (AG-104 / AG132).

LegacyMechanicalTests retains the original SQL fixtures and physical row,
ledger and image comparisons; it grants no qualified business-history credit.
RestoreTests requires closed real-producer facts materialized in the compiled
prefix schemas by evo-engine, then executes all fifteen real CLI calls.
The two direct-helper tests cover the native-only boundary; actual migration
commit/rollback and rogue-SQL admission live in migration_clock's file controls.
"""

import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
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
    con = readonly_fixture(path)
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


def readonly_fixture(path):
    # Only closed private test fixtures; this is not a production source reader.
    path = Path(path).resolve()
    for suffix in ("-wal", "-shm", "-journal"):
        sidecar = Path(str(path) + suffix)
        assert not sidecar.exists() or suffix == "-wal" and sidecar.stat().st_size == 0
    return sqlite3.connect(path.as_uri() + "?mode=ro&immutable=1", uri=True)


def fixture_copy(source, directory):
    source, directory = Path(source), Path(directory)
    directory.mkdir(mode=0o700)
    shutil.copyfile(source, directory / "rsia.sqlite3")
    if (source.parent / "blobs").exists():
        shutil.copytree(source.parent / "blobs", directory / "blobs")
    return directory / "rsia.sqlite3"


def fixture_fault(database, sql):
    import fcntl
    # No producer remains live; only this test-owned closed copy is writable.
    directory = os.open(database.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    lock = os.open(database.parent / ".rsia.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    fcntl.flock(directory, fcntl.LOCK_EX | fcntl.LOCK_NB)
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    con = sqlite3.connect(database.resolve().as_uri() + "?mode=rw", uri=True, isolation_level=None)
    try:
        con.executescript(sql)
    finally:
        con.close()
        os.unlink(database.parent / ".rsia.lock")
        os.close(lock)
        os.close(directory)


def fixture_snapshot(directory):
    return {p.relative_to(directory).as_posix(): None if p.is_dir() else p.read_bytes()
            for p in directory.rglob("*")}


class Scenario:
    """A historical live database on `versions`, its backups, and anchors made from it."""

    def __init__(self, test, versions):
        self.base = Path(tempfile.mkdtemp(prefix="ag104-")).resolve()
        test.addCleanup(shutil.rmtree, self.base, True)
        self.versions = list(versions)
        self.history = self.base / "history.sqlite3"
        self.qualified = isinstance(test, RestoreTests)
        if self.qualified:
            test._qualified_scenario = self
            root = Path(os.environ["RSIA_TEST_RESTORE_FIXTURE_ROOT"]).resolve()
            fixtures = json.loads((root / "python-fixtures.json").read_text())
            assert set(fixtures) == {"5", "6", "current", "late"}
            if versions == [1, 2, 4, 5]:
                label = "late" if test._testMethodName == "test_a_0005_backup_replays_a_later_revocation_after_the_upgrade" else "5"
            elif versions == [1, 2, 4, 5, 6]:
                label = "6"
            else:
                assert versions in (CURRENT, [1, 2, 4, 5, 7], [1, 2, 4])
                label = "current"
            self.fixture = fixtures[label]
            self.actual_versions = CURRENT if label == "current" else [1, 2, 4, 5] + ([6] if label == "6" else [])
            shutil.copyfile(self.fixture["prefix"], self.history)
            return
        con = connect(self.history)
        sqlx_migrate(con, self.versions)
        seed(con, self.versions)
        con.close()

    def anchor(self, name="live", upgrade=True):
        """The current control plane: the live database after the binary upgrade."""
        directory = self.base / name
        if self.qualified:
            source = self.fixture["anchor"] if upgrade else self.fixture["prefix"]
            return fixture_copy(source, directory)
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
        if self.qualified:
            source = Path(self.fixture["backup"])
            database = fixture_copy(source / "rsia.sqlite3", directory)
            document = json.loads((source / "backup-manifest.json").read_bytes())
            if self.versions != self.actual_versions:
                keep = ",".join(map(str, self.versions))
                fixture_fault(database, "DELETE FROM _sqlx_migrations WHERE version NOT IN (" + keep + ")")
                document["migrations"] = [m for m in document["migrations"] if m["version"] in self.versions]
            if db_sql:
                fixture_fault(database, db_sql)
            data = database.read_bytes()
            document["database"].update(sha256=hashlib.sha256(data).hexdigest(), bytes=len(data))
            if manifest:
                manifest(document)
            raw = (source / "backup-manifest.json").read_bytes()
            if document != json.loads(raw):
                raw = json.dumps(document).encode()
            (directory / "backup-manifest.json").write_bytes(raw)
            return directory
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
        if self.qualified:
            packet = json.loads(Path(self.fixture["delta"]).read_bytes())
            packet["base_manifest_sha256"] = hashlib.sha256(manifest).hexdigest()
            path = self.base / (backup.name + "-delta.json")
            path.write_text(json.dumps(packet))
            return path
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
        before = (fixture_snapshot(backup), fixture_snapshot(anchor.parent)) if self.qualified else None
        if self.qualified:
            self.last_backup = backup
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--native-reader", os.environ["RSIA_TEST_NATIVE_READER"],
                "--prepare-configs-file", os.environ["RSIA_TEST_PREPARE_CONFIGS_FILE"],
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
        if self.qualified:
            assert (fixture_snapshot(backup), fixture_snapshot(anchor.parent)) == before
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
        # Legacy commit/rollback obligation: Rust's committed_six_survives_seven_post_ddl_admission_failure.
        # This old Python entry now refuses both prefixes before opening the copy.
        for versions in ([1, 2, 4, 5], [1, 2, 4, 5, 6]):
            with self.subTest(versions=versions):
                scenario = Scenario(self, versions)
                database = scenario.base / "copy.sqlite3"
                shutil.copyfile(scenario.history, database)
                con = connect(database)
                con.execute("CREATE TABLE objects_formal_parent(x)")
                objects_sql = con.execute("SELECT sql FROM sqlite_master WHERE name='objects'").fetchone()
                con.close()
                before_bytes, before_image = database.read_bytes(), database_image(database)
                members = sorted(p.name for p in scenario.base.iterdir())
                with self.assertRaisesRegex(restore.IsolationError, "^SQLite migration requires the fixed native restore-copy operation$"):
                    restore.apply_missing_migrations(database, versions)
                self.assertEqual(database.read_bytes(), before_bytes)
                self.assertEqual(database_image(database), before_image)
                self.assertEqual(sorted(p.name for p in scenario.base.iterdir()), members)
                con = connect(database)
                if versions[-1] == 6:
                    self.assertEqual([row[0] for row in con.execute("SELECT version FROM _sqlx_migrations ORDER BY version")], [1, 2, 4, 5, 6])
                    self.assertIsNotNone(con.execute("SELECT 1 FROM sqlite_master WHERE name='revoke_cleanup_jobs'").fetchone())
                else:
                    self.assertEqual([row[0] for row in con.execute("SELECT version FROM _sqlx_migrations ORDER BY version")], versions)
                    self.assertIsNone(con.execute("SELECT 1 FROM sqlite_master WHERE name='revoke_cleanup_jobs'").fetchone())
                self.assertEqual(con.execute("SELECT sql FROM sqlite_master WHERE name='objects'").fetchone(), objects_sql)
                self.assertEqual(con.execute("SELECT count(*) FROM objects_formal_parent").fetchone(), (0,))
                self.assertFalse(Path(str(database) + "-journal").exists())
                con.close()

    def test_a_migration_that_ends_its_own_transaction_is_refused(self):
        # The same rogue SQL is rejected by Rust's compiled identity, before execution.
        scenario = Scenario(self, [1, 2, 4, 5, 6])
        database = scenario.base / "copy.sqlite3"
        shutil.copyfile(scenario.history, database)
        code = restore.code_migrations()
        rogue = b"CREATE TABLE rogue(x);\nCOMMIT;\n"
        code[7] = (code[7][0], rogue, hashlib.sha384(rogue).hexdigest())
        before_bytes, before_image = database.read_bytes(), database_image(database)
        members = sorted(p.name for p in scenario.base.iterdir())
        with mock.patch.object(restore, "code_migrations", return_value=code):
            with self.assertRaisesRegex(restore.IsolationError, "^SQLite migration requires the fixed native restore-copy operation$"):
                restore.apply_missing_migrations(database, [1, 2, 4, 5, 6])
        self.assertEqual(database.read_bytes(), before_bytes)
        self.assertEqual(database_image(database), before_image)
        self.assertEqual(sorted(p.name for p in scenario.base.iterdir()), members)
        con = connect(database)
        self.assertEqual([row[0] for row in con.execute("SELECT version FROM _sqlx_migrations ORDER BY version")], [1, 2, 4, 5, 6])
        self.assertFalse(Path(str(database) + "-journal").exists())
        con.close()

    def test_the_current_set_is_not_touched(self):
        scenario = Scenario(self, CURRENT)
        before = scenario.history.read_bytes()
        self.assertEqual(restore.apply_missing_migrations(scenario.history, CURRENT), [])
        self.assertEqual(scenario.history.read_bytes(), before)


class LegacyMechanicalTests(unittest.TestCase):
    def test_original_fixture_rows_survive_fixture_migrations(self):
        # Raw historical fixture mechanics only; no qualified business-origin equivalence.
        for versions in ([1, 2, 4, 5], [1, 2, 4, 5, 6], CURRENT):
            scenario = Scenario(self, versions)
            con = connect(scenario.history)
            utf8 = ' { "id" : "shared", "text" : "汉字" } '
            con.execute("INSERT INTO objects(rowid,namespace,kind,id,owner,body,revision) VALUES(901,'legacy-a','artifact','shared','unicode-owner',?,37)", (utf8,))
            con.close()
            anchor = scenario.anchor()
            database = scenario.base / "mechanical.sqlite3"
            shutil.copyfile(scenario.history, database)
            con = connect(database)
            sqlx_migrate(con, [v for v in CURRENT if v not in versions])
            con.close()
            self.assertEqual(database_image(database), database_image(anchor))
            old, upgraded = readonly_fixture(scenario.history), readonly_fixture(database)
            columns = "rowid,namespace,kind,id,owner,body,revision"
            rows = old.execute("SELECT " + columns + " FROM objects ORDER BY rowid").fetchall()
            self.assertEqual(upgraded.execute("SELECT " + columns + " FROM objects ORDER BY rowid").fetchall(), rows)
            self.assertEqual([r[0] for r in rows if r[0] != 901], [3, 8, 100, 117, 134, 151, 900])
            self.assertEqual(rows[-1][1:], ("legacy-a", "artifact", "shared", "unicode-owner", utf8, 37))
            self.assertEqual(rows[-1][5].encode(), utf8.encode())
            self.assertEqual(rows[0][1:4], ("legacy-b", "artifact", "shared"))
            self.assertEqual(rows[0][-1], 31)
            self.assertEqual(rows[1][5], ' { "id" : "first", "v" : 1 } ')
            ledger = "version,description,installed_on,success,checksum,execution_time"
            self.assertEqual(upgraded.execute("SELECT " + ledger + " FROM _sqlx_migrations WHERE version<=? ORDER BY version", (versions[-1],)).fetchall(),
                             old.execute("SELECT " + ledger + " FROM _sqlx_migrations ORDER BY version").fetchall())
            self.assertEqual(database_image(database)["integrity"], [("ok",)])
            self.assertEqual(database_image(database)["foreign_keys"], [])
            old.close()
            upgraded.close()


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
        backup = self._qualified_scenario.last_backup
        raw = (backup / "backup-manifest.json").read_bytes()
        self.assertEqual(receipt["backup_manifest_sha256"], hashlib.sha256(raw).hexdigest())
        for item in json.loads(raw)["blobs"]:
            expected = (backup / item["path"]).read_bytes()
            actual = (dest / item["path"]).read_bytes()
            self.assertEqual(actual, expected)
            self.assertEqual(len(actual), item["bytes"])
            self.assertEqual(hashlib.sha256(actual).hexdigest(), item["sha256"])

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
        self.assertEqual(sorted(path.name for path in dest.iterdir()), ["blobs", "restore.json", "restore.migrations.json", "rsia.sqlite3"])
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
        con = readonly_fixture(dest / "rsia.sqlite3")
        old = readonly_fixture(backup / "rsia.sqlite3")
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
        self.assertEqual(sorted(path.name for path in backup.iterdir()), ["backup-manifest.json", "blobs", "rsia.sqlite3"])

    def test_a_0005_backup_replays_a_later_revocation_after_the_upgrade(self):
        scenario = Scenario(self, [1, 2, 4, 5])
        backup = scenario.backup()
        anchor = scenario.anchor()
        packet = json.loads(Path(scenario.fixture["delta"]).read_bytes())
        self.assertEqual(len(packet["namespaces"]), 1)
        entry = packet["namespaces"][0]
        self.assertEqual(len(entry["events"]), 1)
        event = entry["events"][0]
        namespace, source = entry["namespace"], event["source_id"]
        base_seq, base_digest = entry["base_seq"], entry["base_digest"]
        self.assertEqual(event["seq"], base_seq + 1)
        self.assertEqual(event["previous_digest"], base_digest)
        latest_digest = event["digest"]
        body = json.dumps(tombstone(source, event["seq"], latest_digest, event["reason"], event["created_at"]))
        delta = scenario.delta(backup, [event], (base_seq + 1, latest_digest))
        dest, result = scenario.restore(backup, anchor, delta=delta)
        self.assert_restored(dest, result, 1, "6,7")
        self.assertEqual(json.loads((dest / "restore.migrations.json").read_text())["applied"], [
            {"version": 6, "checksum_sha384": hashlib.sha384(migration_file(6).read_bytes()).hexdigest()},
            {"version": 7, "checksum_sha384": hashlib.sha384(migration_file(7).read_bytes()).hexdigest()},
        ])
        con = readonly_fixture(dest / "rsia.sqlite3")
        self.assertEqual(con.execute("SELECT version FROM _sqlx_migrations ORDER BY version").fetchall(), [(v,) for v in CURRENT])
        self.assertEqual(con.execute("SELECT seq,digest FROM revoke_watermark WHERE namespace=?", (namespace,)).fetchone(), (base_seq + 1, latest_digest))
        self.assertEqual(
            json.loads(con.execute("SELECT body FROM objects WHERE namespace=? AND kind='tombstone' AND id=?", (namespace, source)).fetchone()[0]),
            json.loads(body),
        )
        self.assertEqual(con.execute("SELECT source_id,state FROM revoke_cleanup_jobs").fetchall(), [(source, "pending")])
        self.assertEqual(con.execute("SELECT count(*) FROM revoke_cleanup_frontier").fetchone(), (3,))
        self.assertEqual(con.execute("SELECT event_kind FROM revoke_cleanup_events").fetchall(), [("logical_block_restored",)])
        con.close()

    def test_a_current_set_backup_is_restored_as_before(self):
        scenario = Scenario(self, CURRENT)
        backup = scenario.backup()
        anchor = scenario.anchor()
        dest, result = scenario.restore(backup, anchor)
        self.assert_restored(dest, result, 0, "none")
        self.assertEqual(sorted(path.name for path in dest.iterdir()), ["blobs", "restore.json", "rsia.sqlite3"])
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
        self.assert_isolated(scenario, dest, result, "native restore refused: clock_regressed")

    def test_an_unsuccessful_migration_is_isolated(self):
        scenario = Scenario(self, [1, 2, 4, 5, 6])
        anchor = scenario.anchor()
        failed = "UPDATE _sqlx_migrations SET success=0 WHERE version=5"
        both = scenario.backup("both", db_sql=failed, manifest=lambda m: m["migrations"][3].update(success=False))
        dest, result = scenario.restore(both, anchor, "restored-both")
        self.assert_isolated(scenario, dest, result, "migration is not successful")
        database_only = scenario.backup("database-only", db_sql=failed)
        dest, result = scenario.restore(database_only, anchor, "restored-database-only")
        self.assert_isolated(scenario, dest, result, "native restore refused: clock_regressed")

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
        self.assert_isolated(scenario, dest, result, "native restore refused: clock_regressed")


if __name__ == "__main__":
    unittest.main()
