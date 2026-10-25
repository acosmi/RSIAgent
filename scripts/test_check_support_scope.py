#!/usr/bin/env python3
"""Stdlib-only protection and adversarial tests for check_support_scope.py."""
from __future__ import annotations

import contextlib
import copy
import hashlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_support_scope as css  # noqa: E402

REAL_ROOT = Path(__file__).resolve().parents[1]

def _write(path: Path, content: str = "stub\n") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")

def _valid_manifest_dict() -> dict:
    return copy.deepcopy(json.loads((REAL_ROOT / "reports/support-scope.json").read_text(encoding="utf-8")))

def _valid_execution(**overrides) -> dict:
    """A well-formed record-level `execution` object (AG-102): the literally executed command, not the declared target."""
    execution = {
        "runner": "ctrl_verify.sh",
        "command": "cargom test --workspace --locked --no-fail-fast",
        "scope": "workspace",
        "exit_code": 0,
        "evidence": "out/fixture/ctrl-fixture.log.test",
    }
    execution.update(overrides)
    return execution

def _fresh_record(base: dict, task: str, name: str, stem: str) -> dict:
    """Give a known-PR record its own evidence identity, source and local artifacts."""
    record = copy.deepcopy(base)
    record.pop("pr_state", None)  # retain the reviewed PR/merge, without adding a state claim
    record.update(
        {
            "id": f"{task}.{name}",
            "plan_version": css.PLAN_VERSION,
            "plan_sha256": css.PLAN_SHA256,
            "plan_recheck": "not_affected",  # a record verified against the current plan states it explicitly (plan §18.5)
            "source_sha": hashlib.sha1(f"{task}.{name}".encode()).hexdigest(),
            "log_path": f"out/fixture/{stem}.log",
            "input_ref": f"out/fixture/{stem}-input.json",
            "input_digest": hashlib.sha256(stem.encode()).hexdigest(),
            "record_source": f"out/fixture/{stem}-acceptance.json",
            "execution": _valid_execution(),  # retain an explicit literal execution record
        }
    )
    return record

@contextlib.contextmanager
def _repinned(manifest: dict, register: dict | None = None):
    """Re-pin the reviewed V080.d tables (historical record digests, withdrawn tuples, relation pairs and, with a register,
    its triples) to a deliberately changed fixture, so that a test isolates the rule it is about. The tables are computed
    when the context is entered; the reviewed pins themselves are exercised by V45RebindTests."""
    records = {record["id"]: record for scope in manifest["e_scopes"].values() for record in scope["verified_subscopes"]}
    patches = {
        "EXPECTED_RECORD_DIGESTS": {identifier: css.record_digest(records[identifier]) for identifier in css.EXPECTED_RECORD_DIGESTS if identifier in records},
        "EXPECTED_WITHDRAWN": {
            entry["id"]: (int(entry["withdrawn_in_pr"].rsplit("/", 1)[1]), entry["clause"], entry["owner_task"], entry["status"])
            for entry in manifest["withdrawn_records"]
        },
        "EXPECTED_RELATIONS": {(kind, record["id"], old) for record in records.values() for kind in ("supersedes", "completes") for old in record.get(kind, [])},
    }
    if register is not None:
        patches["EXPECTED_MISSING_TRIPLES"] = {(entry["record_id"], entry["field"], entry["path"]) for entry in register["entries"]}
    with mock.patch.multiple(css, **patches):
        yield

class FixtureRepo:
    def __init__(self, root: Path, manifest: dict | None = None):
        self.root = root
        self.manifest = manifest or _valid_manifest_dict()
        paths: set[str] = {"reports/support-scope.json"}
        for scope in self.manifest["e_scopes"].values():
            paths.update(scope["implementation_files"])
            paths.update(record["test_entry"] for record in scope["verified_subscopes"])
        for group in ("b_commitments", "u_commitments", "k_commitments"):
            for entry in self.manifest["traceability"][group]:
                paths.update(entry["local_artifacts"])
        for source in self.manifest["traceability"]["so_sources"]:
            paths.update(source["local_use"]["artifacts"])
        for path in paths:
            if path != "reports/support-scope.json":
                try:
                    css.safe_relative(path, "fixture path")
                except css.CheckerError:
                    continue
                _write(root / path)
        members = ["crates/evo-core", "crates/evo-storage", "crates/evo-engine", "crates/evo-http", "crates/evo-mcp", "apps/rsia"]
        _write(root / "Cargo.toml", "[workspace]\nmembers = [" + ",".join(json.dumps(x) for x in members) + ']\n[workspace.package]\npublish = false\nlicense-file = "LICENSE"\n')
        _write(root / "LICENSE", (REAL_ROOT / "LICENSE").read_text())
        _write(root / "README.md", "## License\n\n" + css.PROJECT_LICENSE_README + "\n")
        for member in members:
            name = member.rsplit("/", 1)[1]
            _write(root / member / "Cargo.toml", f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2024"\nlicense-file.workspace = true\npublish.workspace = true\n')
            _write(root / member / "src/lib.rs", "pub mod evaluator;\npub mod hosts;\n")
        # Runtime validation uses the same actual Rust fact table, never a Python copy.
        _write(root / "crates/evo-core/tests/support_scope.rs", (REAL_ROOT / "crates/evo-core/tests/support_scope.rs").read_text(encoding="utf-8"))
        # The pinned missing-evidence register and the tracked (non out/) record sources travel with the checkout.
        (root / css.DEFAULT_KNOWN_MISSING).parent.mkdir(parents=True, exist_ok=True)
        (root / css.DEFAULT_KNOWN_MISSING).write_bytes((REAL_ROOT / css.DEFAULT_KNOWN_MISSING).read_bytes())
        for scope in self.manifest["e_scopes"].values():
            for record in scope["verified_subscopes"]:
                source = record.get("record_source")
                if isinstance(source, str) and not css.in_local_evidence_store(source):
                    try:
                        css.safe_relative(source, "fixture record source")
                    except css.CheckerError:
                        continue
                    _write(root / source)
        self.write_manifest(self.manifest)
    def write_manifest(self, manifest: dict, rel: str = "reports/support-scope.json") -> None:
        _write(self.root / rel, json.dumps(manifest, ensure_ascii=False, indent=2))

def _run_cli(args: list[str]) -> tuple[int, dict]:
    output = io.StringIO()
    with contextlib.redirect_stdout(output):
        code = css.main(args)
    return code, json.loads(output.getvalue())

def _structure_check(repo_root: Path, manifest_rel: str = "reports/support-scope.json", source_of_truth: Path | None = None) -> css.CheckReport:
    """A fixture checkout carries no local evidence store, so structural tests state --structure-only explicitly (AG-200: the
    default mode fails closed on unjudged out/ evidence paths and is exercised by FailClosedEvidenceJudgmentTests)."""
    return css.run_check(repo_root, manifest_rel, source_of_truth, structure_only=True)

def _out_path_count(manifest: dict) -> int:
    """Evidence path references (input_ref, log_path, record_source) inside the local evidence store out/."""
    return sum(1 for scope in manifest["e_scopes"].values() for record in scope["verified_subscopes"] for field in css.EVIDENCE_PATH_FIELDS if css.in_local_evidence_store(record[field]))

class ValidManifestTests(unittest.TestCase):
    def test_valid_manifest_passes_structurally_and_exits_zero(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            report = _structure_check(repo.root)
            self.assertTrue(report.structure_valid, report.errors)
            code, payload = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--structure-only"])
            self.assertEqual(code, 0)
            self.assertTrue(payload["structure_valid"])
            self.assertFalse(payload["plan_binding_available"])
            self.assertFalse(payload["input_binding_available"])
            self.assertEqual(payload["input_binding_scope"], "local_input_manifest_hashes_only")
            self.assertEqual(payload["evidence_judgment"], "not_performed_structure_only")

    def test_missing_source_of_truth_is_reported_as_unverified_not_silently_ok(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            report = _structure_check(repo.root)
            self.assertTrue(report.structure_valid)
            self.assertFalse(report.plan_binding_available)
            self.assertFalse(report.input_binding_available)
            self.assertTrue(any("no --source-of-truth" in item for item in report.needs_verification))
            self.assertTrue(any("local input manifest is unavailable" in item for item in report.needs_verification))

    def test_matching_source_of_truth_binds_plan_but_not_missing_inputs(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            plan = Path(tmp) / "plan.md"
            plan.write_text("fixture plan bytes\n", encoding="utf-8")
            digest = css.sha256_of_file(plan)
            # Only the top level, the current lineage entry and the records verified against the
            # current version follow the plan bytes; historical records keep the v4.1 pair they were
            # verified against (plan §18.8).
            repo.manifest["plan_sha256"] = digest
            repo.manifest["plan_lineage"][-1]["sha256"] = digest
            for scope in repo.manifest["e_scopes"].values():
                for record in scope["verified_subscopes"]:
                    if record["plan_version"] == css.PLAN_VERSION:
                        record["plan_sha256"] = digest
            repo.write_manifest(repo.manifest)
            lineage = copy.deepcopy(css.PLAN_LINEAGE)
            lineage[css.PLAN_VERSION]["sha256"] = digest
            # The records verified against the current plan are digest-pinned since AG-200; the fixture re-binds them to the fixture
            # plan bytes, so the V080.d tables are re-pinned to isolate the plan binding.
            with mock.patch.object(css, "PLAN_SHA256", digest), mock.patch.object(css, "PLAN_LINEAGE", lineage), _repinned(repo.manifest):
                report = _structure_check(repo.root, source_of_truth=plan)
            self.assertTrue(report.structure_valid, report.errors)
            self.assertTrue(report.plan_binding_available)
            self.assertFalse(report.input_binding_available)
            self.assertTrue(
                any(record["plan_version"] == "v4.1" for scope in repo.manifest["e_scopes"].values() for record in scope["verified_subscopes"]),
                "fixture must exercise historical records that stay bound to v4.1",
            )

    def test_mismatched_source_of_truth_hash_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            plan = Path(tmp) / "plan.md"
            plan.write_text("wrong plan\n", encoding="utf-8")
            report = _structure_check(repo.root, source_of_truth=plan)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("source-of-truth SHA-256 mismatch" in item for item in report.errors))

    def test_legitimate_implementation_reorder_and_new_real_consumer_are_allowed(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            files = manifest["e_scopes"]["E03"]["implementation_files"]
            manifest["e_scopes"]["E03"]["implementation_files"] = list(reversed(files))
            manifest["e_scopes"]["E03"]["implementation_files"].append(
                "crates/evo-engine/src/compiler.rs"
            )
            repo = FixtureRepo(Path(tmp), manifest)
            report = _structure_check(repo.root)
            self.assertTrue(report.structure_valid, report.errors)

    def test_new_scoped_evidence_refs_and_compatible_status_progress_are_allowed(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            new_record = copy.deepcopy(
                manifest["e_scopes"]["E03"]["verified_subscopes"][0]
            )
            input_bytes = b"synthetic structural input only\n"
            new_record.update(
                {
                    "id": "E03.future_controller_subscope",
                    "source_sha": "a" * 40,
                    "pr_state": "merged",
                    "input_ref": "out/future/input.json",
                    "input_digest": hashlib.sha256(input_bytes).hexdigest(),
                    "log_path": "out/future/test.log",
                    "record_source": "out/future/acceptance.json",
                    "actual_result": "fixture record exercises schema extensibility",
                    "scope": "synthetic structural fixture only",
                    "execution": _valid_execution(evidence="out/future/test.log"),
                }
            )
            manifest["e_scopes"]["E03"]["verified_subscopes"].append(new_record)
            manifest["e_scopes"]["E03"]["status"] = "implemented_not_verified"
            manifest["traceability"]["k_commitments"][1][
                "related_evidence_refs"
            ].append(new_record["id"])
            manifest["traceability"]["k_commitments"][4][
                "implementation_status"
            ] = "implemented_not_verified"
            repo = FixtureRepo(Path(tmp), manifest)
            path = repo.root / new_record["input_ref"]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(input_bytes)
            report = _structure_check(repo.root)
            self.assertTrue(report.structure_valid, report.errors)

class RejectionTests(unittest.TestCase):
    def test_rejects_impl_file_that_does_not_exist_on_disk(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E00"]["implementation_files"][0] = "scripts/never_written.py"
            repo = FixtureRepo(Path(tmp), manifest)
            # Fixture creation wrote the declared fake; remove it to retain the original protection semantic.
            (repo.root / "scripts/never_written.py").unlink()
            report = _structure_check(repo.root)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("implementation_files differ" in item or "does not exist" in item for item in report.errors))

    def test_rejects_test_command_pointing_at_a_fake_target(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            record = manifest["e_scopes"]["E01"]["verified_subscopes"][0]
            record["test_entry"] = "crates/evo-core/tests/never_written.rs"
            record["command"] = "cargo test --locked --offline -p evo-core --test never_written"
            repo = FixtureRepo(Path(tmp), manifest)
            (repo.root / record["test_entry"]).unlink()
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_test_command_target_not_listed_in_test_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E01"]["verified_subscopes"][0]["command"] = "cargo test --locked --offline -p evo-core --test other"
            repo = FixtureRepo(Path(tmp), manifest)
            _write(repo.root / "crates/evo-core/tests/other.rs")
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_duplicate_id_in_a_commitment_set(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["traceability"]["b_commitments"][-1] = copy.deepcopy(manifest["traceability"]["b_commitments"][0])
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_id_set_with_correct_length_but_wrong_members(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["traceability"]["b_commitments"][-1]["id"] = "B11"
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_so_source_with_illegal_hash_format(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["traceability"]["so_sources"][0]["commit"] = "not-a-real-sha"
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_verified_status_with_no_test_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E17"]["status"] = "verified"
            manifest["e_scopes"]["E17"]["remaining"] = []
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E03"]["status"] = "verified"
            manifest["e_scopes"]["E03"]["remaining"] = []
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(
                _structure_check(repo.root).structure_valid,
                "an existing scoped record must not automatically verify the whole E task",
            )

    def test_planned_status_tolerates_empty_test_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            self.assertEqual(repo.manifest["e_scopes"]["E15"]["status"], "planned")
            self.assertEqual(repo.manifest["e_scopes"]["E15"]["verified_subscopes"], [])
            self.assertTrue(_structure_check(repo.root).structure_valid)

    def test_rejects_missing_e16_parent_e17_or_e18(self):
        for missing in ("E16", "E17", "E18"):
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as tmp:
                manifest = _valid_manifest_dict()
                del manifest["e_scopes"][missing]
                repo = FixtureRepo(Path(tmp), manifest)
                self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_well_formed_but_wrong_so_commit_and_blob(self):
        for field in ("commit", "blob_sha"):
            with self.subTest(field=field), tempfile.TemporaryDirectory() as tmp:
                manifest = _valid_manifest_dict()
                manifest["traceability"]["so_sources"][0][field] = "a" * 40
                repo = FixtureRepo(Path(tmp), manifest)
                self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_nonexistent_e99_trace_reference(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["traceability"]["so_sources"][0]["e_tasks"] = ["E99"]
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_unknown_status(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E01"]["status"] = "verified_subset"
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_command_with_extra_shell_tokens_or_multiple_targets(self):
        commands = [
            "cargo test --locked --offline -p evo-core --test evaluation_v41 && false",
            "cargo test --locked --offline -p evo-core --test evaluation_v41 --test statistics",
        ]
        for command in commands:
            with self.subTest(command=command), tempfile.TemporaryDirectory() as tmp:
                manifest = _valid_manifest_dict()
                manifest["e_scopes"]["E01"]["verified_subscopes"][0]["command"] = command
                repo = FixtureRepo(Path(tmp), manifest)
                self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_duplicate_json_object_key(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            raw = (repo.root / "reports/support-scope.json").read_text(encoding="utf-8")
            raw = raw.replace('{\n  "schema_version"', '{\n  "schema_version": "rsia.support_scope.v2",\n  "schema_version"', 1)
            (repo.root / "reports/support-scope.json").write_text(raw, encoding="utf-8")
            report = _structure_check(repo.root)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("duplicate JSON object key" in item for item in report.errors))

    def test_missing_input_is_needs_verification_but_wrong_hash_is_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            missing = _structure_check(repo.root)
            self.assertTrue(missing.structure_valid)
            self.assertFalse(missing.input_binding_available)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid, "the default mode fails closed on the absent inputs")
            first = repo.manifest["e_scopes"]["E00"]["verified_subscopes"][0]
            _write(repo.root / first["input_ref"], "wrong bytes")
            # The digest mismatch is reported first in the structure-only and in the fail-closed default mode alike.
            for mismatch in (_structure_check(repo.root), css.run_check(repo.root, "reports/support-scope.json", None)):
                self.assertFalse(mismatch.structure_valid)
                self.assertIn("input manifest SHA-256 mismatch", mismatch.errors[0])

    def test_rejects_nonzero_verified_exit_code(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E01"]["verified_subscopes"][0]["exit_code"] = 101
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_boolean_and_float_zero_exit_codes(self):
        for value in (False, 0.0):
            with self.subTest(value=value), tempfile.TemporaryDirectory() as tmp:
                manifest = _valid_manifest_dict()
                manifest["e_scopes"]["E01"]["verified_subscopes"][0][
                    "exit_code"
                ] = value
                repo = FixtureRepo(Path(tmp), manifest)
                self.assertFalse(
                    _structure_check(repo.root).structure_valid
                )

    def test_rejects_range_or_trace_chain_disconnect(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["trace_index_coverage"]["E16.6"]["ranges"].remove("V001–V123")
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["traceability"]["k_commitments"][4]["e_tasks"] = ["E18"]
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

class ControllerFalseAcceptanceTests(unittest.TestCase):
    """One rejection per false acceptance reproduced by the controller (2026-09-30), plus legitimate counterparts."""

    def _errors(self, manifest: dict) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            report = _structure_check(repo.root)
        self.assertFalse(report.structure_valid, "manipulated manifest must be rejected")
        return report.errors

    def _assert_valid(self, manifest: dict) -> css.CheckReport:
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            report = _structure_check(repo.root)
        self.assertTrue(report.structure_valid, report.errors)
        return report

    def test_rejects_planned_scope_carrying_verified_evidence(self):
        manifest = _valid_manifest_dict()
        e15 = manifest["e_scopes"]["E15"]
        self.assertEqual(e15["status"], "planned")
        e15["verified_subscopes"].append(
            _fresh_record(manifest["e_scopes"]["E13"]["verified_subscopes"][0], "E15", "controller_acceptance", "e15")
        )
        errors = self._errors(manifest)
        self.assertIn(
            "E15: status 'planned' is incompatible with verified evidence records ['E15.controller_acceptance']",
            errors[0],
        )

    def test_rejects_parent_scope_stronger_than_weakest_child(self):
        manifest = _valid_manifest_dict()
        e16 = manifest["e_scopes"]["E16"]
        e16["status"] = "verified"
        e16["remaining"] = []
        e16["implementation_files"].append("crates/evo-engine/src/hosts.rs")
        record = _fresh_record(
            manifest["e_scopes"]["E16.4"]["verified_subscopes"][0], "E16", "gate_controller_acceptance", "e16-gate"
        )
        e16["verified_subscopes"].append(record)
        e16["completion_evidence_refs"] = [record["id"]]
        errors = self._errors(manifest)
        self.assertIn(
            "E16: status 'verified' is stronger than sub-package E16.1 status",
            errors[0],
        )
        with self.subTest(variant="implemented_not_verified parent over a blocked child"):
            manifest = _valid_manifest_dict()
            for child in ("E16.1", "E16.2", "E16.3", "E16.5", "E16.6"):
                manifest["e_scopes"][child]["status"] = "implemented_not_verified"
            manifest["e_scopes"]["E16"]["status"] = "implemented_not_verified"
            errors = self._errors(manifest)
            self.assertIn(
                "E16: status 'implemented_not_verified' is stronger than sub-package E16.4 status 'blocked'",
                errors[0],
            )

    def test_rejects_verified_status_for_optional_disabled_scope(self):
        manifest = _valid_manifest_dict()
        self.assertIn("E17", manifest["optional_disabled"])
        e17 = manifest["e_scopes"]["E17"]
        e17["status"] = "verified"
        e17["remaining"] = []
        record = _fresh_record(
            manifest["e_scopes"]["E00"]["verified_subscopes"][0], "E17", "scorer_evolution_controller_acceptance", "e17"
        )
        e17["verified_subscopes"].append(record)
        e17["completion_evidence_refs"] = [record["id"]]
        errors = self._errors(manifest)
        self.assertIn(
            "E17: optional scope is listed in optional_disabled and no enablement is recorded; status 'verified' is not allowed",
            errors[0],
        )

    def test_rejects_evidence_duplicated_or_borrowed_from_another_task(self):
        manifest = _valid_manifest_dict()
        e13, e15 = manifest["e_scopes"]["E13"], manifest["e_scopes"]["E15"]
        e15["status"] = "in_progress"
        e15["implementation_files"] = list(e13["implementation_files"])
        borrowed = copy.deepcopy(e13["verified_subscopes"][0])
        borrowed["id"] = "E15.controller_acceptance"
        borrowed["plan_recheck"] = "rechecked_v4.3"  # a v4.2 record of E15 (outside the v4.5 union) keeps the v4.3 marker
        e15["verified_subscopes"].append(borrowed)
        errors = self._errors(manifest)
        self.assertIn(
            f"E15.controller_acceptance: log_path {borrowed['log_path']!r} duplicates evidence {e13['verified_subscopes'][0]['id']} of task E13",
            errors[0],
        )
        with self.subTest(variant="same test run re-logged under another task"):
            manifest = _valid_manifest_dict()
            e13, e15 = manifest["e_scopes"]["E13"], manifest["e_scopes"]["E15"]
            e15["status"] = "in_progress"
            rerun = copy.deepcopy(e13["verified_subscopes"][0])
            rerun.update({"id": "E15.controller_acceptance", "log_path": "out/fixture/e15-rerun.log", "plan_recheck": "rechecked_v4.3"})
            e15["verified_subscopes"].append(rerun)
            errors = self._errors(manifest)
            self.assertIn(
                f"the same test run ({rerun['test_entry']} at source_sha {rerun['source_sha']}) is already claimed by {e13['verified_subscopes'][0]['id']} of task E13",
                errors[0],
            )
        with self.subTest(variant="evidence id namespaced under a foreign task"):
            manifest = _valid_manifest_dict()
            e15 = manifest["e_scopes"]["E15"]
            e15["status"] = "in_progress"
            e15["verified_subscopes"].append(
                _fresh_record(manifest["e_scopes"]["E13"]["verified_subscopes"][0], "E13", "second_controller_acceptance", "e13-second")
            )
            errors = self._errors(manifest)
            self.assertIn("E15: evidence id 'E13.second_controller_acceptance' is not namespaced under its own task", errors[0])
        with self.subTest(variant="test target outside the task's declared crates"):
            manifest = _valid_manifest_dict()
            e15 = manifest["e_scopes"]["E15"]
            e15["status"] = "in_progress"
            e15["verified_subscopes"].append(
                _fresh_record(manifest["e_scopes"]["E01"]["verified_subscopes"][0], "E15", "controller_acceptance", "e15-foreign")
            )
            errors = self._errors(manifest)
            self.assertIn(
                "E15.controller_acceptance: test_entry 'crates/evo-core/tests/evaluation_v41.rs' (package evo-core) is not among E15's declared implementation_files nor inside a crate they declare",
                errors[0],
            )

    def test_rejects_unknown_keys_asserting_effect_or_verdict(self):
        manifest = _valid_manifest_dict()
        first_v = manifest["traceability"]["v_scenarios"][0]["id"]
        first_b = manifest["traceability"]["b_commitments"][0]["id"]
        first_so = manifest["traceability"]["so_sources"][0]["id"]
        cases = [
            ("scope entry", ("e_scopes", "E13"), "effect", "E13 scope entry: unknown key(s) ['effect']"),
            ("evidence record", ("e_scopes", "E13", "verified_subscopes", 0), "verdict", f"E13 evidence {_valid_manifest_dict()['e_scopes']['E13']['verified_subscopes'][0]['id']}: unknown key(s) ['verdict']"),
            ("v_scenario", ("traceability", "v_scenarios", 0), "benefit", f"{first_v}: unknown key(s) ['benefit']"),
            ("commitment", ("traceability", "b_commitments", 0), "effect", f"{first_b}: unknown key(s) ['effect']"),
            ("so_source", ("traceability", "so_sources", 0), "effect", f"{first_so}: unknown key(s) ['effect']"),
            ("local_use", ("traceability", "so_sources", 0, "local_use"), "effect", f"{first_so} local_use: unknown key(s) ['effect']"),
            ("traceability", ("traceability",), "effect", "traceability: unknown key(s) ['effect']"),
            ("trace_index_coverage", ("trace_index_coverage", "E00"), "effect", "E00 trace_index_coverage: unknown key(s) ['effect']"),
            ("dimensions", ("dimensions",), "benefit", "dimensions: unknown key(s) ['benefit']"),
            ("root", (), "effect", "manifest root: unknown key(s) ['effect']"),
        ]
        for name, path, key, expected in cases:
            with self.subTest(object=name):
                manifest = _valid_manifest_dict()
                target = manifest
                for step in path:
                    target = target[step]
                target[key] = "improved"
                errors = self._errors(manifest)
                self.assertIn(expected, errors[0])
                self.assertIn("not part of the rsia.support_scope.v2 schema", errors[0])

    def test_rejects_merged_sha_inconsistent_with_manifest_merge_facts(self):
        def pr47(manifest: dict) -> dict:
            record = manifest["e_scopes"]["E16.4"]["verified_subscopes"][0]
            self.assertEqual(record["pr_state"], "merged")
            return record

        merged_pr47 = pr47(_valid_manifest_dict())
        self.assertEqual(merged_pr47["pr_state"], "merged")
        self.assertTrue(merged_pr47["merged_sha"])
        pr47_id = merged_pr47["id"]
        target_pr = merged_pr47["pr"].rsplit("/", 1)[1]
        other = _valid_manifest_dict()["e_scopes"]["E13"]["verified_subscopes"][0]
        other_pr = other["pr"].rsplit("/", 1)[1]
        with self.subTest(variant="merged_sha copied from another PR's merge commit"):
            manifest = _valid_manifest_dict()
            record = pr47(manifest)
            record["merged_sha"] = manifest["e_scopes"]["E13"]["verified_subscopes"][0]["merged_sha"]
            errors = self._errors(manifest)
            self.assertIn(
                f"{pr47_id}: merged_sha {record['merged_sha']} is already recorded as the merge commit of PR #{other_pr} ({other['id']})",
                errors[0],
            )
        with self.subTest(variant="merged_sha equal to another PR's source_sha"):
            manifest = _valid_manifest_dict()
            pr47(manifest)["merged_sha"] = manifest["e_scopes"]["E13"]["verified_subscopes"][0]["source_sha"]
            errors = self._errors(manifest)
            self.assertIn(f"is the source_sha of PR #{other_pr} ({other['id']}), not a merge of PR #{target_pr}", errors[0])
        with self.subTest(variant="same PR merged in one record and unmerged in another"):
            manifest = _valid_manifest_dict()
            second = _fresh_record(pr47(manifest), "E16.4", "second_controller_acceptance", "pr47-second")
            second.update({"pr": pr47(manifest)["pr"], "merged_sha": None})
            manifest["e_scopes"]["E16.4"]["verified_subscopes"].append(second)
            errors = self._errors(manifest)
            self.assertIn(
                f"E16.4.second_controller_acceptance: PR #{target_pr} carries merged_sha None here but {merged_pr47['merged_sha']!r} in {pr47_id}",
                errors[0],
            )
        with self.subTest(variant="merged_sha on a record whose pr_state is draft"):
            manifest = _valid_manifest_dict()
            pr47(manifest).update({"pr_state": "draft", "merged_sha": "c" * 40})
            errors = self._errors(manifest)
            self.assertIn(
                f"{pr47_id}: merged_sha {'c' * 40} is set but the record's pr_state is 'draft', not 'merged'",
                errors[0],
            )
        with self.subTest(variant="pr_state merged without merged_sha"):
            manifest = _valid_manifest_dict()
            pr47(manifest).update({"pr_state": "merged", "merged_sha": None})
            errors = self._errors(manifest)
            self.assertIn(f"{pr47_id}: pr_state is 'merged' but merged_sha is null", errors[0])
        with self.subTest(variant="malformed merged_sha"):
            manifest = _valid_manifest_dict()
            pr47(manifest)["merged_sha"] = "2e209dc"
            errors = self._errors(manifest)
            self.assertIn("merged_sha must be null or lowercase 40-hex", errors[0])

    def test_unreviewed_merged_sha_is_rejected_but_reviewed_claim_stays_unverified(self):
        """Static fact agreement is required; it still does not prove git ancestry."""
        manifest = _valid_manifest_dict()
        record = manifest["e_scopes"]["E16.4"]["verified_subscopes"][0]
        record.pop("pr_state")
        record["merged_sha"] = hashlib.sha1(b"fabricated merge of pr47").hexdigest()
        self.assertIn("differs from the reviewed PR merge", self._errors(manifest)[0])
        record["merged_sha"] = css.reviewed_merge_facts(REAL_ROOT)[int(record["pr"].rsplit("/", 1)[1])]
        # Dropping pr_state rewrites a historical record: the reviewed digest pin rejects it, re-pinned it is structurally valid.
        self.assertIn(f"{record['id']}: historical record differs from its pinned digest", self._errors(manifest)[0])
        with _repinned(manifest):
            report = self._assert_valid(manifest)
        self.assertTrue(
            any(
                f"{record['id']}: merged_sha {record['merged_sha']} for PR #{record['pr'].rsplit('/', 1)[1]} (pr_state unrecorded) is a manifest assertion" in item
                for item in report.needs_verification
            ),
            report.needs_verification,
        )

    def test_new_task_with_fresh_evidence_is_still_registrable(self):
        manifest = _valid_manifest_dict()
        e15 = manifest["e_scopes"]["E15"]
        e15["status"] = "implemented_not_verified"
        record = _fresh_record(
            manifest["e_scopes"]["E13"]["verified_subscopes"][0], "E15", "inheritance_controller_acceptance", "e15-fresh"
        )
        record.update(
            {
                "test_entry": "crates/evo-engine/tests/meta.rs",
                "command": "cargo test --locked --offline -p evo-engine --test meta",
                "pr_state": "merged",
            }
        )
        e15["verified_subscopes"].append(record)
        self._assert_valid(manifest)

    def test_parent_verified_when_all_children_verified_is_allowed(self):
        manifest = _valid_manifest_dict()
        scopes = manifest["e_scopes"]
        base = scopes["E13"]["verified_subscopes"][0]
        plan = {
            "E16.1": ("evo-engine", "import_gate"), "E16.2": ("evo-engine", "packages_gate"), "E16.3": ("evo-engine", "seeds_gate"),
            "E16.4": ("evo-engine", "hosts_gate"), "E16.5": ("evo-engine", "capacity_gate"), "E16.6": ("evo-core", "support_scope"),
            "E16": ("evo-engine", "delivery_gate"),
        }
        scopes["E16"]["implementation_files"].append("crates/evo-engine/src/delivery_gate.rs")
        for task, (package, target) in plan.items():
            record = _fresh_record(base, task, "gate_controller_acceptance", task.lower().replace(".", "-"))
            record.update(
                {
                    "test_entry": f"crates/{package}/tests/{target}.rs",
                    "command": f"cargo test --locked --offline -p {package} --test {target}",
                }
            )
            entry = scopes[task]
            entry["verified_subscopes"].append(record)
            entry["status"] = "verified"
            entry["remaining"] = []
            entry["completion_evidence_refs"] = [record["id"]]
        self._assert_valid(manifest)

    def test_genuinely_merged_pr_with_consistent_state_is_allowed(self):
        manifest = _valid_manifest_dict()
        e10 = manifest["e_scopes"]["E10"]
        merged_record = e10["verified_subscopes"][0]
        self.assertEqual(merged_record["pr_state"], "merged")
        self.assertTrue(css.HEX40.fullmatch(merged_record["merged_sha"]))
        merged_record["pr_state"] = "merged"
        sibling = _fresh_record(merged_record, "E10", "replay_management_second_target", "e10-second")
        sibling.update(
            {
                "pr": merged_record["pr"],
                "source_sha": merged_record["source_sha"],
                "merged_sha": merged_record["merged_sha"],
                "pr_state": "merged",
                "test_entry": "crates/evo-engine/tests/replay_second.rs",
                "command": "cargo test --locked --offline -p evo-engine --test replay_second",
            }
        )
        e10["verified_subscopes"].append(sibling)
        report = self._assert_valid(manifest)
        self.assertTrue(any(f"for PR #{merged_record['pr'].rsplit('/', 1)[1]} (pr_state merged) is a manifest assertion" in item for item in report.needs_verification))

class PlanLineageTests(unittest.TestCase):
    """v4.1→v4.2→v4.3 lineage binding (plan §18.9): the top level binds the current plan, historical records
    keep the lineage pair they were verified against, digests outside the lineage are rejected."""

    V41_SHA = "45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150"
    V42_SHA = "70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455"

    V43_SHA = "73dd00bf5ef6e9757d0c391c9a8be4ca92b6e06a750aacb542817f82cbd208a7"
    V44_SHA = "45ae29d71396f77dffc3baab7fed50564334734623bcec15f267778ddebeb496"
    V45_SHA = "f6f9b1e44f3b1f364c0db8720f7eaec490ed7a03ddc053de8bfbab65326e6164"

    def _errors(self, manifest: dict) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            report = _structure_check(repo.root)
        self.assertFalse(report.structure_valid, "manipulated manifest must be rejected")
        return report.errors

    def _assert_valid(self, manifest: dict) -> css.CheckReport:
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            report = _structure_check(repo.root)
        self.assertTrue(report.structure_valid, report.errors)
        return report

    def test_constants_bind_the_current_plan_and_register_the_lineage(self):
        self.assertEqual(css.PLAN_VERSION, "v4.5")
        self.assertEqual(css.PLAN_SHA256, self.V45_SHA)
        self.assertEqual(list(css.PLAN_LINEAGE), ["v4.1", "v4.2", "v4.3", "v4.4", "v4.5"])
        self.assertEqual(css.PLAN_LINEAGE["v4.1"]["sha256"], self.V41_SHA)
        self.assertEqual(css.PLAN_LINEAGE["v4.2"]["sha256"], self.V42_SHA)
        self.assertEqual(css.PLAN_LINEAGE["v4.3"]["sha256"], self.V43_SHA)
        self.assertEqual(css.PLAN_LINEAGE["v4.4"]["sha256"], self.V44_SHA)
        self.assertEqual(css.PLAN_LINEAGE["v4.5"]["sha256"], css.PLAN_SHA256)
        self.assertEqual(css.PLAN_RECHECKED, "rechecked_v4.5")
        self.assertEqual(css.PLAN_RECHECK_VALUES, {"not_affected", "required", "rechecked_v4.2", "rechecked_v4.3", "rechecked_v4.5"})
        self.assertNotIn("rechecked_v4.4", css.PLAN_RECHECK_VALUES, "v4.4 was never bound by the index on its own")
        self.assertEqual(css.V43_RECHECK_TASKS, set(css.EXPECTED_E_IDS) - {"E17", "E18"})
        self.assertEqual(css.RELATION_MIN_VERSION, "v4.5")

    def test_rejects_top_level_still_bound_to_v41(self):
        manifest = _valid_manifest_dict()
        manifest["plan_version"], manifest["plan_sha256"] = "v4.1", self.V41_SHA
        errors = self._errors(manifest)
        self.assertIn("manifest top level is not bound to the current plan v4.5", errors[0])
        manifest = _valid_manifest_dict()
        manifest["plan_version"], manifest["plan_sha256"] = "v4.3", self.V43_SHA
        self.assertIn("manifest top level is not bound to the current plan v4.5", self._errors(manifest)[0])

    def test_records_bound_to_either_lineage_pair_are_accepted(self):
        manifest = _valid_manifest_dict()
        record = manifest["e_scopes"]["E01"]["verified_subscopes"][0]
        self.assertEqual((record["plan_version"], record["plan_sha256"]), ("v4.1", self.V41_SHA))
        self._assert_valid(manifest)
        record.update({"plan_version": "v4.2", "plan_sha256": self.V42_SHA})
        self.assertIn("E01.controller_acceptance: historical record differs from its pinned digest", self._errors(manifest)[0])
        with _repinned(manifest):
            self._assert_valid(manifest)
        # A backfilled first acceptance keeps the v4.3 pair it was verified against and carries the v4.5 recheck (R-04 option ii).
        backfill = next(r for r in manifest["e_scopes"]["E05"]["verified_subscopes"] if r["id"] == "E05.ag122_ga18_bound_cycle_validation")
        self.assertEqual((backfill["plan_version"], backfill["plan_sha256"], backfill["plan_recheck"]), ("v4.3", self.V43_SHA, "rechecked_v4.5"))

    def test_rejects_record_with_mismatched_or_unregistered_plan_digest(self):
        cases = [
            ("v4.2 version paired with the v4.1 digest", {"plan_version": "v4.2"}, f"plan_sha256 '{self.V41_SHA}' is not the registered digest of plan v4.2"),
            ("v4.1 version paired with the v4.2 digest", {"plan_sha256": self.V42_SHA}, f"plan_sha256 '{self.V42_SHA}' is not the registered digest of plan v4.1"),
            ("digest outside the lineage", {"plan_sha256": "a" * 64}, f"plan_sha256 '{'a' * 64}' is not the registered digest of plan v4.1"),
            ("unregistered version", {"plan_version": "v4.0"}, "plan_version 'v4.0' is outside the registered plan lineage ['v4.1', 'v4.2', 'v4.3', 'v4.4', 'v4.5']"),
        ]
        for name, patch, expected in cases:
            with self.subTest(case=name):
                manifest = _valid_manifest_dict()
                manifest["e_scopes"]["E01"]["verified_subscopes"][0].update(patch)
                errors = self._errors(manifest)
                self.assertIn(f"E01.controller_acceptance: {expected}", errors[0])

    def test_rejects_pending_or_unknown_plan_recheck(self):
        manifest = _valid_manifest_dict()
        manifest["e_scopes"]["E01"]["verified_subscopes"][0]["plan_recheck"] = "required"
        errors = self._errors(manifest)
        self.assertIn("E01.controller_acceptance: plan_recheck is 'required'", errors[0])
        for value in ("improved", "rechecked_v4.1", "rechecked_v4.4", "", None, True):
            with self.subTest(value=value):
                manifest = _valid_manifest_dict()
                manifest["e_scopes"]["E01"]["verified_subscopes"][0]["plan_recheck"] = value
                errors = self._errors(manifest)
                self.assertIn("E01.controller_acceptance: plan_recheck must be one of ['not_affected', 'rechecked_v4.2', 'rechecked_v4.3', 'rechecked_v4.5', 'required']", errors[0])

    def test_touched_tasks_require_recheck_marker_but_untouched_records_may_omit_it(self):
        baseline = _valid_manifest_dict()
        order = list(css.PLAN_LINEAGE)
        for task in sorted(css.PLAN_RECHECK_TASKS):
            if not baseline["e_scopes"][task]["verified_subscopes"]:
                continue
            for marker in (None, "not_affected", "rechecked_v4.2", "rechecked_v4.3", "rechecked_v4.4"):
                with self.subTest(task=task, marker=marker):
                    manifest = copy.deepcopy(baseline)
                    record = manifest["e_scopes"][task]["verified_subscopes"][0]
                    version = record["plan_version"]
                    self.assertEqual(record["plan_recheck"], "rechecked_v4.5")
                    self.assertNotEqual(version, css.PLAN_VERSION)
                    if marker is None:
                        del record["plan_recheck"]
                    else:
                        record["plan_recheck"] = marker
                    if marker == "rechecked_v4.4":
                        expected = "plan_recheck must be one of"
                    elif marker is not None and marker.startswith("rechecked_") and order.index(marker[len("rechecked_"):]) <= order.index(version):
                        expected = f"plan_recheck {marker!r} does not name a plan later than the record's own plan_version {version!r}"
                    else:
                        expected = f"task {task} is touched by the v4.5 revision; a record still bound to plan {version} must carry plan_recheck 'rechecked_v4.5', got {marker or 'not_affected'!r}"
                    self.assertIn(f"{record['id']}: {expected}", self._errors(manifest)[0])
        for task in ("E17", "E18"):
            manifest = copy.deepcopy(baseline)
            manifest["e_scopes"][task]["verified_subscopes"][0].pop("plan_recheck", None)
            self._assert_valid(manifest)
        # A record verified against the current plan states not_affected explicitly (plan §18.5, review R-06).
        manifest = copy.deepcopy(baseline)
        record = _fresh_record(manifest["e_scopes"]["E00"]["verified_subscopes"][0], "E00", "fixture_current_plan", "e00-current")
        manifest["e_scopes"]["E00"]["verified_subscopes"].append(record)
        self._assert_valid(manifest)
        del record["plan_recheck"]
        self.assertIn("E00.fixture_current_plan: a record bound to the current plan v4.5 must carry plan_recheck 'not_affected' explicitly (plan §18.5), got no marker", self._errors(manifest)[0])

    def test_recheck_tasks_are_the_literal_v45_landing_union(self):
        # plan §19.6 (as corrected by v4.5) ∪ §19.7 (plan §18.11); "E16" is the parent key only (plan §18.10).
        expected = {"E00", "E01", "E02", "E03", "E04", "E05", "E06", "E07", "E08", "E09", "E10", "E13", "E16", "E16.1", "E16.3", "E16.5", "E16.6"}
        self.assertEqual(css.PLAN_RECHECK_TASKS, expected)
        self.assertTrue({"E11", "E12", "E14", "E15", "E16.2", "E16.4", "E17", "E18"}.isdisjoint(css.PLAN_RECHECK_TASKS))

    def test_v43_scenarios_are_complete_and_not_execution_credit(self):
        manifest = _valid_manifest_dict()
        mapping = css.expected_v_to_e()
        self.assertEqual(set(mapping), {f"V{i:03}" for i in range(1, 124)})
        self.assertEqual(mapping["V107"], {"E02", "E16", "E16.4"})
        # Registering the new scenario family alone cannot confer execution credit.
        tampered = copy.deepcopy(manifest)
        unexecuted = tampered["traceability"]["v_scenarios"][-1]
        unexecuted.update({"status": "verified", "verification_status": "verified", "related_evidence_refs": [], "remaining": ""})
        self._errors(tampered)
        for variant in ("missing", "duplicate", "wrong owner"):
            tampered = copy.deepcopy(manifest)
            if variant == "missing":
                tampered["traceability"]["v_scenarios"].pop()
            elif variant == "duplicate":
                tampered["traceability"]["v_scenarios"].append(copy.deepcopy(tampered["traceability"]["v_scenarios"][-1]))
            else:
                tampered["traceability"]["v_scenarios"][-1]["e_tasks"] = ["E17"]
            self._errors(tampered)

    def test_rejects_missing_or_wrong_plan_lineage(self):
        cases = [
            ("missing", lambda m: m.pop("plan_lineage")),
            ("empty", lambda m: m.__setitem__("plan_lineage", [])),
            ("reversed order", lambda m: m["plan_lineage"].reverse()),
            ("current version only", lambda m: m["plan_lineage"].pop(0)),
            ("wrong current digest", lambda m: m["plan_lineage"][1].__setitem__("sha256", self.V41_SHA)),
            ("wrong historical file", lambda m: m["plan_lineage"][0].__setitem__("file", "RSIAgent-v4.1.md")),
            ("unknown key", lambda m: m["plan_lineage"][1].__setitem__("effect", "improved")),
            ("extra future version", lambda m: m["plan_lineage"].append({"version": "v4.4", "file": "future.md", "sha256": "b" * 64})),
            ("malformed digest", lambda m: m["plan_lineage"][0].__setitem__("sha256", "45F3")),
        ]
        for name, mutate in cases:
            with self.subTest(case=name):
                manifest = _valid_manifest_dict()
                mutate(manifest)
                errors = self._errors(manifest)
                self.assertTrue(errors[0].startswith("plan_lineage"), errors[0])

    def test_e09_owns_v097_after_the_v42_revision(self):
        self.assertIn("V097", css.EXPECTED_E_SCENARIOS["E09"])
        self.assertIn("E09", css.expected_v_to_e()["V097"])
        manifest = _valid_manifest_dict()
        self.assertIn("V097", manifest["e_scopes"]["E09"]["scenarios"])
        manifest["e_scopes"]["E09"]["scenarios"].remove("V097")
        self.assertIn("E09: scenario execution responsibility differs", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        v097 = next(entry for entry in manifest["traceability"]["v_scenarios"] if entry["id"] == "V097")
        v097["e_tasks"].remove("E09")
        self.assertIn("V097: E responsibility differs", self._errors(manifest)[0])

class HonestMachineFieldTests(unittest.TestCase):
    """AG-102 (plan E16.6 L1461, E00 L1185): machine fields must not read as stronger evidence than they are.

    command/exit_code/log_path describe the declared single target; the record-level `execution` object states what was
    literally run and is required from PR #142 on; production_consumer registers library-only modules; evidence refs must
    name real records."""

    CARD_COMMAND = "declared single-target checker command; syntax validated, not necessarily the literally executed command"
    CARD_EXIT_CODE = "exit code of the controller gate run that covered the declared target"
    CARD_LOG_PATH = "local evidence file containing that target's result (controller test log or acceptance receipt)"

    def _errors(self, manifest: dict) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            report = _structure_check(repo.root)
        self.assertFalse(report.structure_valid, "manipulated manifest must be rejected")
        return report.errors

    def _assert_valid(self, manifest: dict) -> css.CheckReport:
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            report = _structure_check(repo.root)
        self.assertTrue(report.structure_valid, report.errors)
        return report

    def _with_fresh_record(self, pr: int = 144, **overrides) -> tuple[dict, dict]:
        """A fresh E13 evidence identity for a reviewed PR (unknown PRs stay rejection fixtures)."""
        manifest = _valid_manifest_dict()
        e13 = manifest["e_scopes"]["E13"]
        record = _fresh_record(e13["verified_subscopes"][0], "E13", "ag102_fixture", "e13-ag102")
        record["pr"] = f"https://github.com/acosmi/RSIAgent/pull/{pr}"
        record["merged_sha"] = css.reviewed_merge_facts(REAL_ROOT).get(pr)
        record.update(overrides)
        e13["verified_subscopes"].append(record)
        return manifest, record

    def _v_refs(self, manifest: dict, vid: str) -> list:
        return next(entry for entry in manifest["traceability"]["v_scenarios"] if entry["id"] == vid)["related_evidence_refs"]

    # field_semantics

    def test_real_manifest_declares_the_pinned_field_semantics(self):
        semantics = _valid_manifest_dict()["field_semantics"]
        self.assertEqual(semantics, css.FIELD_SEMANTICS)
        self.assertEqual(set(semantics), {"command", "exit_code", "log_path", "execution", "production_consumer", "execution_runners", "supersedes", "completes", "note", "withdrawn_records", "evidence_missing_register", "plan_recheck", "record_pins", "evidence_judgment", "evidence_missing_register_pin", "pr_state", "merged_sha"})
        self.assertEqual(semantics["command"], self.CARD_COMMAND)
        self.assertEqual(semantics["exit_code"], self.CARD_EXIT_CODE)
        self.assertEqual(semantics["log_path"], self.CARD_LOG_PATH)
        self.assertIn("field_semantics", css.ROOT_KEYS)
        self.assertTrue({"execution", "production_consumer"} <= css.EVIDENCE_KEYS)

    def test_rejects_missing_field_semantics(self):
        manifest = _valid_manifest_dict()
        del manifest["field_semantics"]
        self.assertIn(
            "field_semantics: must be an object stating what command, exit_code, log_path, execution and production_consumer mean",
            self._errors(manifest)[0],
        )

    def test_cli_exits_nonzero_when_field_semantics_is_missing(self):
        manifest = _valid_manifest_dict()
        del manifest["field_semantics"]
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            code, payload = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--structure-only"])
        self.assertEqual(code, 1)
        self.assertFalse(payload["structure_valid"])
        self.assertIn("field_semantics", payload["errors"][0])

    def test_rejects_non_object_incomplete_redefined_or_extended_field_semantics(self):
        for value in (None, [], "command means executed", 7):
            with self.subTest(value=value):
                manifest = _valid_manifest_dict()
                manifest["field_semantics"] = value
                self.assertIn("field_semantics: must be an object", self._errors(manifest)[0])
        for key in css.FIELD_SEMANTICS:
            with self.subTest(missing=key):
                manifest = _valid_manifest_dict()
                del manifest["field_semantics"][key]
                self.assertIn(f"field_semantics: missing the definition of '{key}'", self._errors(manifest)[0])
            for label, replacement in (("redefined", "the command that was executed and passed"), ("null", None), ("empty", "")):
                with self.subTest(key=key, replaced=label):
                    manifest = _valid_manifest_dict()
                    manifest["field_semantics"][key] = replacement
                    self.assertIn(f"field_semantics.{key} differs from the definition pinned by the checker", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        manifest["field_semantics"]["effect"] = "improved"
        self.assertIn("field_semantics: unknown key(s) ['effect']", self._errors(manifest)[0])

    # record-level execution

    def test_rejects_record_above_pr_141_without_execution(self):
        for number in (142, 143, 999):
            with self.subTest(pr=number):
                manifest, record = self._with_fresh_record(number)
                del record["execution"]
                self.assertIn(
                    f"{record['id']}: PR #{number} is above #141, so the record must carry an 'execution' object",
                    self._errors(manifest)[0],
                )

    def test_execution_stays_optional_up_to_pr_141(self):
        self.assertEqual(css.EXECUTION_REQUIRED_ABOVE_PR, 141)
        css.validate_execution({}, "E13.boundary_only", 141)  # unit boundary, no manifest/PR acceptance claim
        manifest, record = self._with_fresh_record(140)
        del record["execution"]
        self._assert_valid(manifest)

    def test_older_record_with_a_malformed_execution_is_still_rejected(self):
        manifest, record = self._with_fresh_record(140, execution=_valid_execution(exit_code="0"))
        self.assertIn(f"{record['id']} execution: exit_code must be the integer exit code of the executed command", self._errors(manifest)[0])

    def test_valid_record_above_pr_141_with_execution_passes(self):
        manifest, record = self._with_fresh_record(144)
        self.assertEqual(record["execution"], _valid_execution())
        self.assertGreater(int(record["pr"].rsplit("/", 1)[1]), css.EXECUTION_REQUIRED_ABOVE_PR)
        self._assert_valid(manifest)
        for runner in sorted(css.EXECUTION_RUNNERS):
            for scope in sorted(css.EXECUTION_SCOPES):
                with self.subTest(runner=runner, scope=scope):
                    manifest, _ = self._with_fresh_record(144, execution=_valid_execution(runner=runner, scope=scope))
                    self._assert_valid(manifest)

    def test_rejects_execution_command_that_copies_the_declared_target(self):
        copies = [
            "cargo test --locked --offline -p evo-core --test evaluation_v41",
            "cargo test --locked --offline",
            "  cargo   test --locked   --offline -p evo-core --lib hosts::tests  ",
            "cargo test\t--locked --offline -p evo-core --test evaluation_v41",
        ]
        for command in copies:
            with self.subTest(command=command):
                manifest, record = self._with_fresh_record(144, execution=_valid_execution(command=command))
                self.assertIn(f"{record['id']} execution: command must be the literally executed command", self._errors(manifest)[0])
        for command in ("cargom test --workspace --locked --no-fail-fast", "cargo test --locked -p evo-core --test evaluation_v41", "cargo test --workspace --locked"):
            with self.subTest(command=command):
                manifest, _ = self._with_fresh_record(144, execution=_valid_execution(command=command))
                self._assert_valid(manifest)

    def test_rejects_non_integer_execution_exit_code(self):
        for value in ("0", 0.0, 1.5, True, False, None, [], {}):
            with self.subTest(value=value):
                manifest, record = self._with_fresh_record(144, execution=_valid_execution(exit_code=value))
                self.assertIn(f"{record['id']} execution: exit_code must be the integer exit code of the executed command", self._errors(manifest)[0])

    def test_execution_exit_code_records_the_real_value_even_when_nonzero(self):
        """E00: real exit codes. A workspace run that also hits unrelated failing binaries exits nonzero; recording that must stay possible."""
        for code in (0, 1, 101, 134, -1):
            with self.subTest(exit_code=code):
                manifest, _ = self._with_fresh_record(144, execution=_valid_execution(exit_code=code))
                self._assert_valid(manifest)

    def test_rejects_malformed_execution_objects(self):
        base = _valid_execution()
        # (case, execution object, error text that must follow "<record id> execution")
        cases = [
            ("null", None, ": must be an object"),
            ("list", [], ": must be an object"),
            ("string", "ctrl_verify.sh", ": must be an object"),
            ("unknown key", {**base, "verdict": "pass"}, ": unknown key(s) ['verdict']"),
            ("runner outside the set", _valid_execution(runner="manual"), ": runner must be one of ['codex_task', 'ctrl_verify.sh', 'ctrl_verify_v43.sh', 'ctrl_verify_v45.sh', 'other']"),
            ("runner null", _valid_execution(runner=None), ": runner must be one of"),
            ("runner unhashable", _valid_execution(runner=["ctrl_verify.sh"]), ": runner must be one of"),
            ("scope outside the set", _valid_execution(scope="all"), ": scope must be one of ['single_target', 'workspace']"),
            ("scope unhashable", _valid_execution(scope={}), ": scope must be one of"),
            ("empty command", _valid_execution(command=""), " command: must be a non-empty string"),
            ("blank command", _valid_execution(command="   "), " command: must be a non-empty string"),
            ("list command", _valid_execution(command=["cargom", "test"]), " command: must be a non-empty string"),
            ("empty evidence", _valid_execution(evidence=""), " evidence: must be a non-empty string"),
            ("null evidence", _valid_execution(evidence=None), " evidence: must be a non-empty string"),
        ]
        for key in sorted(css.EXECUTION_KEYS):
            cases.append((f"missing {key}", {k: v for k, v in base.items() if k != key}, f": missing key(s) ['{key}']"))
        for name, execution, expected in cases:
            with self.subTest(case=name):
                manifest, record = self._with_fresh_record(144, execution=execution)
                self.assertIn(f"{record['id']} execution{expected}", self._errors(manifest)[0])

    # production_consumer

    def test_rejects_invalid_production_consumer(self):
        for value in ("yes", "library_only", "", "WIRED", None, True, 1, ["wired"], {}):
            with self.subTest(value=value):
                manifest, record = self._with_fresh_record(144, production_consumer=value)
                self.assertIn(
                    f"{record['id']}: production_consumer must be one of ['none_library_only', 'wired'] when present",
                    self._errors(manifest)[0],
                )

    def test_accepts_both_production_consumer_values_and_their_absence(self):
        self.assertEqual(css.PRODUCTION_CONSUMER_VALUES, {"wired", "none_library_only"})
        for value in sorted(css.PRODUCTION_CONSUMER_VALUES):
            with self.subTest(value=value):
                manifest, _ = self._with_fresh_record(144, production_consumer=value)
                self._assert_valid(manifest)
        manifest, record = self._with_fresh_record(144)
        self.assertNotIn("production_consumer", record)
        self._assert_valid(manifest)

    def test_real_manifest_registers_the_four_library_only_modules(self):
        sentence = " Library-only as of e727d1c: no production caller in engine/http/mcp/apps."
        by_pr = {
            int(record["pr"].rsplit("/", 1)[1]): record
            for scope in _valid_manifest_dict()["e_scopes"].values()
            for record in scope["verified_subscopes"]
        }
        for number, record_id in {
            125: "E04.ag077_local_scoped_acceptance",
            128: "E16.1.ag076_local_scoped_acceptance",
            130: "E16.1.ag070_local_scoped_acceptance",
            131: "E08.ag075_local_scoped_acceptance",
        }.items():
            with self.subTest(pr=number):
                record = by_pr[number]
                self.assertEqual(record["id"], record_id)
                self.assertEqual(record["production_consumer"], "none_library_only")
                self.assertTrue(record["scope"].endswith(sentence), record["scope"])

    # related_evidence_refs

    def test_rejects_related_evidence_ref_naming_a_missing_record(self):
        def v_scenario(manifest: dict) -> str:
            self._v_refs(manifest, "V002").append("E02.no_such_record")
            return "V002 evidence refs: unknown evidence refs ['E02.no_such_record']"

        def commitment(manifest: dict) -> str:
            entry = manifest["traceability"]["b_commitments"][0]
            entry["related_evidence_refs"].append("E02.no_such_record")
            return f"{entry['id']} evidence refs: unknown evidence refs ['E02.no_such_record']"

        def so_source(manifest: dict) -> str:
            entry = manifest["traceability"]["so_sources"][0]
            entry.setdefault("verification_evidence_refs", []).append("E03.no_such_record")
            return f"{entry['id']} evidence refs: unknown evidence refs ['E03.no_such_record']"

        def near_miss(manifest: dict) -> str:
            self._v_refs(manifest, "V002").append("E02.ag055_v1_wire_golden")
            return "V002 evidence refs: unknown evidence refs ['E02.ag055_v1_wire_golden']"

        for name, mutate in (("v_scenario", v_scenario), ("commitment", commitment), ("so_source", so_source), ("near-miss id", near_miss)):
            with self.subTest(case=name):
                manifest = _valid_manifest_dict()
                expected = mutate(manifest)
                self.assertIn(expected, self._errors(manifest)[0])

    def test_rejects_duplicate_related_evidence_refs(self):
        manifest = _valid_manifest_dict()
        refs = self._v_refs(manifest, "V002")
        refs.append(refs[0])
        self.assertIn("V002 evidence refs: duplicate id", self._errors(manifest)[0])

    def test_rejects_ref_to_a_record_owned_by_a_task_the_scenario_does_not_map(self):
        """Pre-existing owner-task rule (validate_refs): the AG-058 record belongs to E14, so only V scenarios mapped to E14 may cite it."""
        ag058 = "E14.ag058_current_description_boundaries"
        for vid in ("V001", "V002", "V017", "V032", "V037"):
            with self.subTest(scenario=vid):
                manifest = _valid_manifest_dict()
                self._v_refs(manifest, vid).append(ag058)
                self.assertIn(f"{vid} evidence refs: evidence refs are outside the mapped E scope: ['{ag058}']", self._errors(manifest)[0])
        for vid in ("V033", "V035"):
            self.assertIn(ag058, self._v_refs(_valid_manifest_dict(), vid))

    def test_ag102_traceability_backfill_is_present(self):
        manifest = _valid_manifest_dict()
        backfill = {
            "E02.ag055_v1_wire_goldens": ["V002"],
            # The record's scope text also names V001/V002/V017/V032/V037, but those scenarios map to other E tasks and
            # the owner-task rule above rejects them; only the E14-mapped V033/V035 can carry it.
            "E14.ag058_current_description_boundaries": ["V033", "V035"],
            "E01.ag057_explicit_sample_registration": ["V010", "V011", "V012", "V013", "V084", "V085", "V096", "V097"],
            "E16.2.ag071_known_marker_utf8_blocking": ["V067"],
            "E16.2.ag085_utf8_privacy_boundary_regressions": ["V067"],
        }
        for record_id, scenarios in backfill.items():
            for vid in scenarios:
                with self.subTest(record=record_id, scenario=vid):
                    self.assertIn(record_id, self._v_refs(manifest, vid))

class V45RebindTests(unittest.TestCase):
    """AG-182 (plan §18.10 machine form and §18.11, v4.5 Q-50; §2.9/§16.8; §13.5): every new field and structure has a
    rejection branch, and the missing-evidence register decides the three local outcomes."""

    def _report(self, manifest: dict, evidence: dict[str, bytes] | None = None, *, store: bool = False, register: bytes | None = None, repo_files: dict[str, bytes] | None = None, repin: bool = False, repin_register: bool = False, default_mode: bool = False) -> css.CheckReport:
        """evidence goes into a separate --evidence-root store (used only when store=True); repo_files into the checkout;
        repin re-pins the V080.d tables to a fixture that deliberately rewrites historical records (_complete_store);
        repin_register treats the supplied register bytes as reviewed (both checkers pin the register SHA-256, AG-200);
        without a store the run states --structure-only unless default_mode exercises the fail-closed default."""
        reviewed_register = hashlib.sha256(register).hexdigest() if register is not None and repin_register else css.EXPECTED_MISSING_REGISTER_SHA256
        with tempfile.TemporaryDirectory() as tmp, (_repinned(manifest) if repin else contextlib.nullcontext()), mock.patch.object(css, "EXPECTED_MISSING_REGISTER_SHA256", reviewed_register):
            repo = FixtureRepo(Path(tmp) / "checkout", manifest)
            if register is not None:
                (repo.root / css.DEFAULT_KNOWN_MISSING).write_bytes(register)
            for base, files in ((Path(tmp) / "evidence-store", evidence), (repo.root, repo_files)):
                base.mkdir(parents=True, exist_ok=True)
                for rel, body in (files or {}).items():
                    path = base / rel
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(body)
            return css.run_check(repo.root, "reports/support-scope.json", None, None, (Path(tmp) / "evidence-store") if store else None, structure_only=not store and not default_mode)

    def _errors(self, manifest: dict, **kwargs) -> list[str]:
        report = self._report(manifest, **kwargs)
        self.assertFalse(report.structure_valid, "manipulated manifest must be rejected")
        return report.errors

    def _valid(self, manifest: dict, **kwargs) -> css.CheckReport:
        report = self._report(manifest, **kwargs)
        self.assertTrue(report.structure_valid, report.errors)
        return report

    @staticmethod
    def _record(manifest: dict, identifier: str) -> dict:
        return next(r for scope in manifest["e_scopes"].values() for r in scope["verified_subscopes"] if r["id"] == identifier)

    @staticmethod
    def _withdrawn(manifest: dict, identifier: str) -> dict:
        return next(entry for entry in manifest["withdrawn_records"] if entry["id"] == identifier)

    @staticmethod
    def _register() -> dict:
        return json.loads((REAL_ROOT / css.DEFAULT_KNOWN_MISSING).read_text(encoding="utf-8"))

    @staticmethod
    def _pin(manifest: dict, register: dict) -> bytes:
        raw = (json.dumps(register, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
        manifest["evidence_missing_register"]["sha256"] = hashlib.sha256(raw).hexdigest()
        return raw

    @staticmethod
    def _complete_store(manifest: dict, register: dict) -> dict[str, bytes]:
        """Every out/ evidence path that is not registered missing; unregistered inputs get fixture bytes and the
        records naming them get the matching input_digest (registered inputs keep their pinned digest)."""
        registered = {(entry["record_id"], entry["field"]) for entry in register["entries"]}
        files: dict[str, bytes] = {}
        for scope in manifest["e_scopes"].values():
            for record in scope["verified_subscopes"]:
                for field in css.EVIDENCE_PATH_FIELDS:
                    rel = record[field]
                    if not css.in_local_evidence_store(rel) or (record["id"], field) in registered:
                        continue
                    body = files.setdefault(rel, f"fixture evidence {rel}\n".encode())
                    if field == "input_ref":
                        record["input_digest"] = hashlib.sha256(body).hexdigest()
        return files

    def test_plan_recheck_markers_name_a_later_plan_and_union_records_carry_v45(self):
        manifest = _valid_manifest_dict()
        outside = [(task, r) for task, scope in manifest["e_scopes"].items() if task not in css.PLAN_RECHECK_TASKS for r in scope["verified_subscopes"] if r["plan_version"] != css.PLAN_VERSION]
        self.assertEqual(len(outside), 14)
        kept = [r["id"] for task, r in outside if r["plan_version"] in css.V43_RECHECK_FROM and task in css.V43_RECHECK_TASKS]
        self.assertEqual(len(kept), 11)
        self.assertTrue(all(r["plan_recheck"] == "rechecked_v4.3" for task, r in outside if r["id"] in kept), "records outside the union keep their historical marker")
        # Review R-01 / probe f1: deleting the v4.3 marker of any of the 11 records (E12.ag025 first) fails again.
        for identifier in ["E12.ag025_curriculum_cleanup"] + [i for i in kept if i != "E12.ag025_curriculum_cleanup"]:
            with self.subTest(deleted=identifier):
                manifest = _valid_manifest_dict()
                record = self._record(manifest, identifier)
                del record["plan_recheck"]
                task = identifier.rsplit(".", 1)[0]
                self.assertIn(f"{identifier}: task {task} is outside the v4.5 recheck union but was rechecked by the v4.3 revision; a record bound to plan {record['plan_version']} keeps plan_recheck 'rechecked_v4.3', got 'not_affected'", self._errors(manifest)[0])
        # Probe f2 and siblings: E11.controller cannot claim a v4.5, v4.4 or v4.2 recheck, nor drop to not_affected.
        cases = [
            ("rechecked_v4.5", "E11.controller_acceptance: plan_recheck 'rechecked_v4.5' is valid only inside the v4.5 recheck union; task E11 is outside it"),
            ("rechecked_v4.4", "E11.controller_acceptance: plan_recheck must be one of"),
            ("rechecked_v4.2", "E11.controller_acceptance: task E11 is outside the v4.5 recheck union but was rechecked by the v4.3 revision; a record bound to plan v4.1 keeps plan_recheck 'rechecked_v4.3', got 'rechecked_v4.2'"),
            ("not_affected", "got 'not_affected'"),
        ]
        for marker, expected in cases:
            with self.subTest(marker=marker):
                manifest = _valid_manifest_dict()
                self._record(manifest, "E11.controller_acceptance")["plan_recheck"] = marker
                self.assertIn(expected, self._errors(manifest)[0])
        # E17/E18 were in no recheck set: not_affected may be implicit, a rechecked_* claim fails.
        for task in ("E17", "E18"):
            for marker in ("rechecked_v4.2", "rechecked_v4.3", "rechecked_v4.5"):
                with self.subTest(task=task, marker=marker):
                    manifest = _valid_manifest_dict()
                    record = manifest["e_scopes"][task]["verified_subscopes"][0]
                    record["plan_recheck"] = marker
                    expected = "is valid only inside the v4.5 recheck union" if marker == "rechecked_v4.5" else f"no registered recheck applies to a plan v4.1 record of task {task}"
                    self.assertIn(f"{record['id']}: ", self._errors(manifest)[0])
                    self.assertIn(expected, self._errors(manifest)[0])
        # A v4.3 record outside the union keeps not_affected (absent or explicit); a non-later or v4.5 marker fails.
        for marker, expected in (("rechecked_v4.2", "does not name a plan later than the record's own plan_version 'v4.3'"), ("rechecked_v4.3", "does not name a plan later than the record's own plan_version 'v4.3'"), ("rechecked_v4.5", "is valid only inside the v4.5 recheck union")):
            with self.subTest(v43=marker):
                manifest = _valid_manifest_dict()
                self._record(manifest, "E16.4.ag126_v43_host_surface_record")["plan_recheck"] = marker
                self.assertIn(f"E16.4.ag126_v43_host_surface_record: plan_recheck {marker!r} ", self._errors(manifest)[0])
                self.assertIn(expected, self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        self._record(manifest, "E16.4.ag126_v43_host_surface_record")["plan_recheck"] = "not_affected"
        self._valid(manifest)
        manifest = _valid_manifest_dict()
        record = self._record(manifest, "E09.ag039_legal_actions")
        record["plan_recheck"] = "rechecked_v4.3"  # probe e
        self.assertIn("E09.ag039_legal_actions: task E09 is touched by the v4.5 revision", self._errors(manifest)[0])
        for value in ([], {}):
            with self.subTest(value=value):
                manifest = _valid_manifest_dict()
                self._record(manifest, "E09.ag039_legal_actions")["plan_recheck"] = value
                self.assertIn("plan_recheck must be one of", self._errors(manifest)[0])

    def test_current_records_state_not_affected_and_relations_need_v45(self):
        manifest = _valid_manifest_dict()
        records = [r for scope in manifest["e_scopes"].values() for r in scope["verified_subscopes"]]
        current = [r for r in records if r["plan_version"] == css.PLAN_VERSION]
        self.assertEqual(len(current), 33)
        self.assertTrue(all(r["plan_recheck"] == "not_affected" and r["pr"].endswith("/pull/200") for r in current))
        # AG-200 backfilled the merge of PR #200 (02a135aa, merged 2026-10-25T04:39:38Z) into its 33 records.
        self.assertTrue(all((r["merged_sha"], r["pr_state"]) == ("02a135aa918fce5dd2ea0c1c8eafc287318f10d2", "merged") for r in current))
        backfills = [r for r in records if r["pr"].rsplit("/", 1)[1] in {"178", "180", "187", "188", "189", "191", "192", "194", "195", "197", "198", "199"}]
        self.assertEqual(len(backfills), 12)
        self.assertTrue(all((r["plan_version"], r["plan_sha256"], r["plan_recheck"]) == ("v4.3", css.PLAN_LINEAGE["v4.3"]["sha256"], "rechecked_v4.5") for r in backfills))
        rerun = "E01.ag182_rerun_controller_acceptance"
        for marker, expected in ((None, "a record bound to the current plan v4.5 must carry plan_recheck 'not_affected' explicitly (plan §18.5), got no marker"), ("required", "plan_recheck is 'required'"), ("rechecked_v4.5", "plan_recheck 'rechecked_v4.5' does not name a plan later than the record's own plan_version 'v4.5'")):
            with self.subTest(marker=marker):
                manifest = _valid_manifest_dict()
                record = self._record(manifest, rerun)
                if marker is None:
                    del record["plan_recheck"]
                else:
                    record["plan_recheck"] = marker
                self.assertIn(f"{rerun}: {expected}", self._errors(manifest)[0])
        # Probe r1: a backfilled v4.3 record of the union without its v4.5 marker fails.
        manifest = _valid_manifest_dict()
        del self._record(manifest, "E05.ag122_ga18_bound_cycle_validation")["plan_recheck"]
        self.assertIn("E05.ag122_ga18_bound_cycle_validation: task E05 is touched by the v4.5 revision; a record still bound to plan v4.3 must carry plan_recheck 'rechecked_v4.5', got 'not_affected'", self._errors(manifest)[0])
        # Review R-07: supersedes/completes exist since v4.5; a record carrying them cannot be rebound to an earlier plan.
        for identifier in (rerun, "E10.ag182_replay_v3_controller_acceptance", "E16.6.ag182_plan_v45_index_binding"):
            with self.subTest(relation_record=identifier):
                manifest = _valid_manifest_dict()
                record = self._record(manifest, identifier)
                record.update({"plan_version": "v4.3", "plan_sha256": css.PLAN_LINEAGE["v4.3"]["sha256"], "plan_recheck": "rechecked_v4.5"})
                self.assertIn(f"{identifier}: supersedes and completes exist since plan v4.5; a record carrying them must be bound to plan v4.5 or later, got 'v4.3'", self._errors(manifest)[0])
        # Review R-08: completes, like supersedes, stays inside the record's own E task.
        manifest = _valid_manifest_dict()
        self._record(manifest, rerun)["completes"] = ["E00.controller_acceptance"]
        self.assertIn(f"{rerun}: completes 'E00.controller_acceptance' of task E00; a re-run belongs to the completed record's own E task", self._errors(manifest)[0])

    def test_withdrawn_tuples_relation_pairs_and_historical_records_are_pinned(self):
        manifest = _valid_manifest_dict()
        tuples = {e["id"]: (int(e["withdrawn_in_pr"].rsplit("/", 1)[1]), e["clause"], e["owner_task"], e["status"]) for e in manifest["withdrawn_records"]}
        self.assertEqual(tuples, css.EXPECTED_WITHDRAWN)
        self.assertEqual(len(css.EXPECTED_WITHDRAWN), 24)
        records = {r["id"]: r for scope in manifest["e_scopes"].values() for r in scope["verified_subscopes"]}
        pairs = {(kind, r["id"], old) for r in records.values() for kind in ("supersedes", "completes") for old in r.get(kind, [])}
        self.assertEqual(pairs, css.EXPECTED_RELATIONS)
        self.assertEqual(sorted(kind for kind, _, _ in css.EXPECTED_RELATIONS).count("supersedes"), 7)
        self.assertEqual(sorted(kind for kind, _, _ in css.EXPECTED_RELATIONS).count("completes"), 25)
        # AG-200 pins every current record: the 93 retained historical records and the 45 records added by AG-182 (12 backfills
        # and the 33 PR #200 records, which carry the backfilled merge 02a135aa).
        self.assertEqual(len(css.EXPECTED_RECORD_DIGESTS), 138)
        self.assertEqual(set(css.EXPECTED_RECORD_DIGESTS), set(records))
        self.assertEqual({i: css.record_digest(records[i]) for i in css.EXPECTED_RECORD_DIGESTS}, css.EXPECTED_RECORD_DIGESTS)
        added = {i for i in records if records[i]["pr"].rsplit("/", 1)[1] in {"178", "180", "187", "188", "189", "191", "192", "194", "195", "197", "198", "199", "200"}}
        self.assertEqual(len(added), 45)
        # The digest is the canonical JSON without plan_recheck: a recheck marker change is judged by the recheck rule only.
        self.assertEqual(css.record_digest({"b": 1, "a": "§", "plan_recheck": "x"}), hashlib.sha256('{"a":"§","b":1}'.encode()).hexdigest())
        # Probe g1 and siblings: a reviewed withdrawn tuple cannot be rewritten field by field.
        for field, value in (("owner_task", "AG-999"), ("clause", "§9.9"), ("status", "replaced_by_binding"), ("withdrawn_in_pr", "https://github.com/acosmi/RSIAgent/pull/165")):
            with self.subTest(withdrawn_field=field):
                manifest = _valid_manifest_dict()
                self._withdrawn(manifest, "E05.ag052_versioned_batch_verdicts")[field] = value
                self.assertIn("E05.ag052_versioned_batch_verdicts: withdrawn entry (withdrawn_in_pr, clause, owner_task, status)", self._errors(manifest)[0])
                self.assertIn("differs from the reviewed tuple (200, '§3.4', 'AG-192', 'pending_recheck')", self._errors(manifest)[0])
        # Review h3: dropping a supersedes and reverting its withdrawn entry to pending_recheck is a rewritten tuple.
        manifest = _valid_manifest_dict()
        del self._record(manifest, "E10.ag182_replay_v3_controller_acceptance")["supersedes"]
        self._withdrawn(manifest, "E10.controller_acceptance")["status"] = "pending_recheck"
        self.assertIn("E10.controller_acceptance: withdrawn entry (withdrawn_in_pr, clause, owner_task, status)", self._errors(manifest)[0])
        # Probe h1: a re-run that drops its completes is a missing relation pair; an unreviewed pair is extra.
        manifest = _valid_manifest_dict()
        del self._record(manifest, "E01.ag182_rerun_controller_acceptance")["completes"]
        self.assertIn("supersedes/completes pairs differ from the reviewed relation pairs: missing=[('completes', 'E01.ag182_rerun_controller_acceptance', 'E01.controller_acceptance')] extra=[]", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        self._record(manifest, "E01.ag182_rerun_controller_acceptance")["completes"].append("E01.ag057_explicit_sample_registration")
        self.assertIn("missing=[] extra=[('completes', 'E01.ag182_rerun_controller_acceptance', 'E01.ag057_explicit_sample_registration')]", self._errors(manifest)[0])
        # Probe k1: one character of a historical record's actual_result.
        manifest = _valid_manifest_dict()
        record = self._record(manifest, "E12.ag050_curriculum_priority")
        record["actual_result"] = record["actual_result"][:-1] + ("." if record["actual_result"][-1] != "." else ";")
        self.assertIn("E12.ag050_curriculum_priority: historical record differs from its pinned digest (canonical JSON without plan_recheck)", self._errors(manifest)[0])
        # A pinned historical record that silently leaves the index fails (choose one nothing else references).
        manifest = _valid_manifest_dict()
        text = json.dumps(manifest, ensure_ascii=False)
        quiet = next(i for i in css.EXPECTED_RECORD_DIGESTS if text.count(json.dumps(i)) == 1 and not any(entry["record_id"] == i for entry in self._register()["entries"]) and not ({"supersedes", "completes"} & set(self._record(manifest, i))))
        scope = manifest["e_scopes"][css.task_of(quiet)]
        scope["verified_subscopes"] = [r for r in scope["verified_subscopes"] if r["id"] != quiet]
        if not scope["verified_subscopes"]:
            self.skipTest("no unreferenced record in a task with other records")
        self.assertIn(f"{quiet}: pinned historical record is missing from verified_subscopes", self._errors(manifest)[0])

    def test_withdrawn_records_are_the_reviewed_list_and_ids_never_revive(self):
        manifest = _valid_manifest_dict()
        self.assertEqual({entry["id"] for entry in manifest["withdrawn_records"]}, css.EXPECTED_WITHDRAWN_IDS)
        self.assertEqual(len(manifest["withdrawn_records"]), 24)
        revived = copy.deepcopy(manifest["e_scopes"]["E09"]["verified_subscopes"][0])
        revived.update({"id": "E09.ag033_run_next_trusted_gate", "log_path": "out/fixture/revived.log"})
        manifest["e_scopes"]["E09"]["verified_subscopes"].append(revived)  # probe a
        self.assertIn("E09.ag033_run_next_trusted_gate: withdrawn record reappears in verified_subscopes", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        revived = copy.deepcopy(manifest["e_scopes"]["E09"]["verified_subscopes"][0])
        revived["id"] = "E09.ag033_run_next_trusted_gate"  # the controller probe keeps the cloned log_path
        manifest["e_scopes"]["E09"]["verified_subscopes"].append(revived)
        self.assertIn("withdrawn record reappears in verified_subscopes", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        manifest["withdrawn_records"] = [entry for entry in manifest["withdrawn_records"] if entry["id"] != "E05.ag080_local_scoped_acceptance"]
        self.assertIn("missing=['E05.ag080_local_scoped_acceptance']", self._errors(manifest)[0])
        cases = [
            ("status", "withdrawn", "withdrawn status must be one of"),
            ("status", None, "withdrawn status must be one of"),
            ("owner_task", "AG187", "owner_task must be a registered task number"),
            ("clause", "3.4", "clause must cite the plan section"),
            ("withdrawn_in_pr", "#165", "withdrawn_in_pr must be a pull request URL"),
            ("withdrawn_in_pr", "https://github.com/acosmi/RSIAgent/pull/999", "no reviewed merge status"),
        ]
        for field, value, expected in cases:
            with self.subTest(field=field, value=value):
                manifest = _valid_manifest_dict()
                self._withdrawn(manifest, "E05.ag052_versioned_batch_verdicts")[field] = value
                self.assertIn(expected, self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        self._withdrawn(manifest, "E05.ag052_versioned_batch_verdicts")["verdict"] = "improved"
        self.assertIn("unknown key(s) ['verdict']", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        del self._withdrawn(manifest, "E05.ag052_versioned_batch_verdicts")["owner_task"]
        self.assertIn("missing key(s) ['owner_task']", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        manifest["withdrawn_records"].append(copy.deepcopy(manifest["withdrawn_records"][0]))
        self.assertIn("duplicate id", self._errors(manifest)[0])

    def _with_reverification(self) -> tuple[dict, dict]:
        """A synthetic re-verification of the withdrawn E05.ag101 (status superseded); callers re-pin it with _repinned."""
        manifest = _valid_manifest_dict()
        self._withdrawn(manifest, "E05.ag101_streaming_verdicts_v42")["status"] = "superseded"
        record = _fresh_record(manifest["e_scopes"]["E05"]["verified_subscopes"][0], "E05", "fixture_reverification", "e05-reverification")
        record["supersedes"] = ["E05.ag101_streaming_verdicts_v42"]
        manifest["e_scopes"]["E05"]["verified_subscopes"].append(record)
        return manifest, record

    def test_supersedes_points_at_a_superseded_withdrawn_record_of_the_same_task(self):
        manifest, _ = self._with_reverification()
        # Unreviewed, the synthetic supersession changes a pinned withdrawn tuple; re-pinned it is structurally valid.
        self.assertIn("E05.ag101_streaming_verdicts_v42: withdrawn entry (withdrawn_in_pr, clause, owner_task, status)", self._errors(manifest)[0])
        with _repinned(manifest):
            self._valid(manifest)
        cases = [
            ("E99.does_not_exist", "which is not a withdrawn record"),  # probe b
            ("E05.controller_acceptance", "which is not a withdrawn record"),
            ("E05.ag052_versioned_batch_verdicts", "its withdrawn status is 'pending_recheck', not 'superseded'"),
            ("E04.ag054_typed_predispatch", "its withdrawn status is 'pending_recheck'"),
        ]
        for target, expected in cases:
            with self.subTest(target=target):
                manifest, record = self._with_reverification()
                with _repinned(manifest):
                    record["supersedes"] = [target]
                    self.assertIn(expected, self._errors(manifest)[0])
        manifest, record = self._with_reverification()
        self._withdrawn(manifest, "E04.ag054_typed_predispatch")["status"] = "superseded"
        with _repinned(manifest):
            record["supersedes"] = ["E04.ag054_typed_predispatch"]
            self.assertIn("the re-verification belongs to the withdrawn record's own E task", self._errors(manifest)[0])
        manifest, record = self._with_reverification()
        with _repinned(manifest):
            record["supersedes"] = []
            self.assertIn("E05.ag101_streaming_verdicts_v42: withdrawn status is 'superseded' but no verified record supersedes it", self._errors(manifest)[0])

    def test_completes_points_at_a_current_valid_record_and_relation_fields_are_shaped(self):
        rerun = "E01.ag182_rerun_controller_acceptance"
        manifest = _valid_manifest_dict()
        self.assertEqual(self._record(manifest, rerun)["completes"], ["E01.controller_acceptance"])
        record = _fresh_record(self._record(manifest, rerun), "E01", "fixture_completion", "e01-completion")
        record.update({"completes": ["E01.ag057_explicit_sample_registration"], "note": "completion fixture"})
        manifest["e_scopes"]["E01"]["verified_subscopes"].append(record)
        with _repinned(manifest):
            self._valid(manifest)
        cases = [
            ("completes", ["E09.ag033_run_next_trusted_gate"], "completes the withdrawn record"),
            ("completes", ["E01.not_a_record"], "which is not a current verified record"),
            ("completes", [rerun], "names the record itself"),
            ("completes", "E01.controller_acceptance", "must be an array of evidence record ids"),
            ("completes", ["bad id"], "must be an array of evidence record ids"),
            ("completes", ["E01.controller_acceptance"] * 2, "lists an id twice"),
            ("supersedes", [rerun], "names the record itself"),
            ("note", "", "note: must be a non-empty string"),
            ("note", ["x"], "note: must be a non-empty string"),
        ]
        for field, value, expected in cases:
            with self.subTest(field=field, value=value):
                manifest = _valid_manifest_dict()
                self._record(manifest, rerun)[field] = value
                self.assertIn(expected, self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        record = self._record(manifest, rerun)
        record["supersedes"] = ["E01.controller_acceptance"]
        self.assertIn("both superseded and completed", self._errors(manifest)[0])

    def test_missing_evidence_outside_the_register_fails_when_the_store_is_checked(self):
        register = self._register()
        self.assertEqual(len(register["entries"]), 42)
        # Complete store: every unregistered path present, registered paths absent -> valid and all 42 reported.
        manifest = _valid_manifest_dict()
        store = self._complete_store(manifest, register)
        report = self._valid(manifest, evidence=store, store=True, repin=True)
        self.assertTrue(report.evidence_store_checked)
        self.assertEqual((report.evidence_judgment, report.evidence_unjudged), ("judged", 0))
        self.assertEqual(len(report.evidence_missing), 42)
        self.assertEqual(report.evidence_registered_not_checked, [])
        self.assertIn("E00.controller_acceptance log_path out/implementation-v4.1-20260919/controller-e00-directed-test.log", report.evidence_missing)
        # One unregistered path removed from the store -> failure naming it.
        victim = next(rel for rel in sorted(store) if "/inputs/" not in rel)
        errors = self._errors(manifest, evidence={rel: body for rel, body in store.items() if rel != victim}, store=True, repin=True)
        self.assertIn(f"{victim!r} is missing locally and is not listed in the missing-evidence register", errors[0])
        # With --structure-only and no --evidence-root the same absences are reported, not judged; registered paths are registered,
        # not checked (R-12); without either option the check fails closed on them (AG-200).
        report = self._valid(_valid_manifest_dict())
        self.assertFalse(report.evidence_store_checked)
        unjudged = _out_path_count(_valid_manifest_dict())
        self.assertEqual((report.evidence_judgment, report.evidence_unjudged), ("not_performed_structure_only", unjudged))
        errors = self._errors(_valid_manifest_dict(), default_mode=True)
        self.assertEqual(errors, [f"evidence store not supplied: {unjudged} out/ evidence paths unjudged; pass --evidence-root <primary checkout> or --structure-only"])
        self.assertTrue(any("local evidence store not supplied (--evidence-root)" in item and "42 registered path(s) are listed as registered, not checked" in item for item in report.needs_verification))
        self.assertEqual(report.evidence_missing, [])
        self.assertEqual(len(report.evidence_registered_not_checked), 42)
        self.assertIn("E00.controller_acceptance log_path out/implementation-v4.1-20260919/controller-e00-directed-test.log", report.evidence_registered_not_checked)
        self.assertEqual(report.to_dict()["evidence_registered_not_checked"], report.evidence_registered_not_checked)
        # A tracked (non out/) evidence path is always checked, and in the default mode before the fail-closed verdict. The record is
        # digest-pinned since AG-200, so the V080.d tables are re-pinned to isolate the location rule.
        manifest = _valid_manifest_dict()
        self._record(manifest, "E01.ag182_rerun_controller_acceptance")["record_source"] = "docs/never_written.md"
        with _repinned(manifest), tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            (repo.root / "docs/never_written.md").unlink()
            reports = (_structure_check(repo.root), css.run_check(repo.root, "reports/support-scope.json", None))
        for report in reports:
            self.assertFalse(report.structure_valid)
            self.assertIn("E01.ag182_rerun_controller_acceptance: record_source 'docs/never_written.md' is missing locally and is not listed in the missing-evidence register", report.errors[0])

    def test_registered_missing_paths_are_listed_and_reappearance_is_checked(self):
        register = self._register()
        unknown = next(entry for entry in register["entries"] if entry["expected_sha256"] == "unknown")
        self.assertTrue(unknown["unknown_reason"])
        # A reappearance is judged before the fail-closed verdict of the default mode (AG-200), so both modes report it first.
        for default_mode in (False, True):
            with self.subTest(default_mode=default_mode):
                errors = self._errors(_valid_manifest_dict(), repo_files={unknown["path"]: b"recovered elsewhere\n"}, default_mode=default_mode)
                self.assertIn("is registered missing with unknown identity and has reappeared", errors[0])
        # The same reappearance inside a supplied evidence store fails as well.
        manifest = _valid_manifest_dict()
        store = self._complete_store(manifest, register)
        store[unknown["path"]] = b"recovered elsewhere\n"
        self.assertIn("unknown identity", self._errors(manifest, evidence=store, store=True, repin=True)[0])
        known = next(entry for entry in register["entries"] if entry["expected_sha256"] != "unknown" and entry["field"] == "log_path")
        for default_mode in (False, True):
            with self.subTest(default_mode=default_mode):
                errors = self._errors(_valid_manifest_dict(), repo_files={known["path"]: b"different bytes\n"}, default_mode=default_mode)
                self.assertIn("has reappeared with bytes or SHA-256 that differ from the register", errors[0])
        # Rewriting a registered identity to match a restored file and refreshing the pointer fails on the reviewed register SHA-256
        # (AG-200, review S-1). Only a reviewed register update, re-pinned in both checkers, lets a path reappear with exactly the
        # registered identity; it then passes and is flagged for removal.
        body = b"exactly the registered bytes\n"
        manifest = _valid_manifest_dict()
        changed = copy.deepcopy(register)
        for entry in changed["entries"]:
            if entry["path"] == known["path"]:
                entry["expected_sha256"], entry["expected_bytes"] = hashlib.sha256(body).hexdigest(), len(body)
        raw = self._pin(manifest, changed)
        self.assertIn("is not the reviewed register SHA-256 pinned in both checkers", self._errors(manifest, repo_files={known["path"]: body}, register=raw)[0])
        report = self._valid(manifest, repo_files={known["path"]: body}, register=raw, repin_register=True)
        self.assertTrue(any("has reappeared with its registered identity" in item for item in report.needs_verification))
        self.assertNotIn(f"{known['record_id']} log_path {known['path']}", report.evidence_registered_not_checked)
        self.assertEqual(report.evidence_missing, [])
        self.assertEqual(len(report.evidence_registered_not_checked), 41)

    def test_register_triples_are_pinned_in_both_checkers(self):
        register = self._register()
        triples = {(entry["record_id"], entry["field"], entry["path"]) for entry in register["entries"]}
        self.assertEqual(triples, css.EXPECTED_MISSING_TRIPLES)
        self.assertEqual(len(css.EXPECTED_MISSING_TRIPLES), 42)
        # Probe c2: drop one entry and re-pin the register SHA-256.
        manifest = _valid_manifest_dict()
        dropped = copy.deepcopy(register)
        gone = dropped["entries"].pop(0)
        raw = self._pin(manifest, dropped)
        self.assertIn("is not the reviewed register SHA-256 pinned in both checkers", self._errors(manifest, register=raw)[0])
        self.assertIn(f"missing-evidence register entries differ from the reviewed register: missing=[{(gone['record_id'], gone['field'], gone['path'])!r}] extra=[]", self._errors(manifest, register=raw, repin_register=True)[0])
        # Probe c3: drop one entry and register another real path, keeping 42 entries and a re-pinned SHA-256.
        manifest = _valid_manifest_dict()
        swapped = copy.deepcopy(register)
        gone = swapped["entries"].pop(0)
        other = self._record(manifest, "E01.ag182_rerun_controller_acceptance")
        swapped["entries"].append({"path": other["log_path"], "record_id": other["id"], "field": "log_path", "expected_bytes": "unknown", "expected_sha256": "unknown", "unknown_reason": "probe c3", "missing_window": "unknown", "group": "probe_c3"})
        self.assertEqual(len(swapped["entries"]), 42)
        raw = self._pin(manifest, swapped)
        for store in (False, True):
            with self.subTest(store=store):
                self.assertIn("is not the reviewed register SHA-256 pinned in both checkers", self._errors(manifest, register=raw, store=store)[0])
                errors = self._errors(manifest, register=raw, store=store, repin_register=True)
                self.assertIn("missing-evidence register entries differ from the reviewed register", errors[0])
                self.assertIn(f"extra=[('E01.ag182_rerun_controller_acceptance', 'log_path', {other['log_path']!r})]", errors[0])

    def test_register_is_pinned_and_names_real_record_fields(self):
        manifest = _valid_manifest_dict()
        register = self._register()
        dropped = copy.deepcopy(register)
        dropped["entries"].pop(0)  # probe c
        raw = (json.dumps(dropped, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
        self.assertIn("SHA-256 differs from evidence_missing_register.sha256", self._errors(manifest, register=raw)[0])
        mutations = [
            ("unknown record", lambda r: r["entries"][0].__setitem__("record_id", "E01.nothing"), "is not a current verified record"),
            ("wrong path", lambda r: r["entries"][0].__setitem__("path", "out/fixture/other.log"), "not the registered path"),
            ("bad field", lambda r: r["entries"][0].__setitem__("field", "scope"), "field must be one of"),
            ("duplicate", lambda r: r["entries"].append(copy.deepcopy(r["entries"][0])), "is registered twice"),
            ("unknown without reason", lambda r: next(e for e in r["entries"] if e["expected_sha256"] == "unknown").pop("unknown_reason"), "unknown_reason: must be a non-empty string"),
            ("reason with a known identity", lambda r: next(e for e in r["entries"] if _identity_known(e)).__setitem__("unknown_reason", "x"), "unknown_reason is only allowed"),
            ("bad window", lambda r: r["entries"][0].__setitem__("missing_window", {"after": "yesterday", "before": "unknown"}), "missing_window.after must be"),
            ("bool bytes", lambda r: r["entries"][0].__setitem__("expected_bytes", True), "expected_bytes must be a non-negative integer"),
            ("input digest", lambda r: next(e for e in r["entries"] if e["field"] == "input_ref" and e["expected_sha256"] != "unknown").__setitem__("expected_sha256", "0" * 64), "must equal the record's input_digest"),
            ("schema", lambda r: r.__setitem__("schema_version", "rsia.evidence_missing.v0"), "schema_version must be"),
            ("extra key", lambda r: r["entries"][0].__setitem__("restored", True), "unknown key(s) ['restored']"),
        ]
        for label, mutate, expected in mutations:
            with self.subTest(case=label):
                manifest = _valid_manifest_dict()
                changed = copy.deepcopy(register)
                mutate(changed)
                raw = self._pin(manifest, changed)
                self.assertIn(expected, self._errors(manifest, register=raw, repin_register=True)[0])
        manifest = _valid_manifest_dict()
        manifest["evidence_missing_register"]["path"] = "reports/other.json"
        self.assertIn("differs from the register in use", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        del manifest["evidence_missing_register"]
        self.assertIn("evidence_missing_register", self._errors(manifest)[0])
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            code, payload = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--known-missing", "reports/other.json", "--structure-only"])
        self.assertEqual(code, 1)
        self.assertIn("differs from the register in use", payload["errors"][0])

    def test_register_file_sha256_is_pinned_in_both_checkers(self):
        """AG-200 (review S-1, probes c4/c5): the manifest pointer, the reviewed constant and the register file agree, so an edited
        identity with a refreshed pointer fails; the register path is pinned as in the Rust checker."""
        raw = (REAL_ROOT / css.DEFAULT_KNOWN_MISSING).read_bytes()
        self.assertEqual(hashlib.sha256(raw).hexdigest(), css.EXPECTED_MISSING_REGISTER_SHA256)
        self.assertEqual(css.EXPECTED_MISSING_REGISTER_SHA256, "b843e3f9692c5ba18ce9148278b3267584222461406fdccc17162ed84c115337")
        self.assertEqual(_valid_manifest_dict()["evidence_missing_register"], {"path": css.DEFAULT_KNOWN_MISSING, "sha256": css.EXPECTED_MISSING_REGISTER_SHA256})
        register = self._register()
        expected = "is not the reviewed register SHA-256 pinned in both checkers"
        # Probe c4: an unknown identity replaced by a forged SHA-256 and size, the forged file restored, the pointer refreshed.
        forged_body = b"forged evidence\n"
        forged = copy.deepcopy(register)
        target = next(entry for entry in forged["entries"] if entry["expected_sha256"] == "unknown" and entry["expected_bytes"] == "unknown")
        target.update({"expected_sha256": hashlib.sha256(forged_body).hexdigest(), "expected_bytes": len(forged_body)})
        del target["unknown_reason"]
        for store in (False, True):
            with self.subTest(probe="c4", store=store):
                manifest = _valid_manifest_dict()
                raw_forged = self._pin(manifest, forged)
                files = {target["path"]: forged_body}
                errors = self._errors(manifest, evidence=files if store else None, store=store, register=raw_forged, repo_files=None if store else files)
                self.assertIn(expected, errors[0])
                # Re-pinned as a reviewed register, the forged identity would be accepted: the constant is what rejects it.
                self._valid(manifest, evidence=files if store else None, store=False, register=raw_forged, repo_files=files, repin_register=True)
        # Probe c5: a known identity downgraded to unknown (with a reason), the pointer refreshed.
        downgraded = copy.deepcopy(register)
        known = next(entry for entry in downgraded["entries"] if _identity_known(entry))
        known.update({"expected_sha256": "unknown", "expected_bytes": "unknown", "unknown_reason": "probe c5"})
        manifest = _valid_manifest_dict()
        raw_down = self._pin(manifest, downgraded)
        self.assertIn(expected, self._errors(manifest, register=raw_down)[0])
        self._valid(manifest, register=raw_down, repin_register=True)
        # The register path is pinned: a consistent pointer, option and copy under another path fail.
        manifest = _valid_manifest_dict()
        manifest["evidence_missing_register"]["path"] = "reports/other.json"
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            (repo.root / "reports/other.json").write_bytes(raw)
            code, payload = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--known-missing", "reports/other.json", "--structure-only"])
        self.assertEqual(code, 1)
        self.assertIn("missing-evidence register 'reports/other.json' is not the reviewed register 'reports/evidence-missing.json'; its path is pinned in both checkers", payload["errors"][0])

    def test_harness2_borrowings_and_sources_follow_the_plan_tables(self):
        manifest = _valid_manifest_dict()
        self.assertEqual([entry["id"] for entry in manifest["traceability"]["hsf_sources"]], [f"HSF{i:02}" for i in range(1, 17)] + ["HSP01"])
        self.assertEqual([entry["id"] for entry in manifest["traceability"]["hs_borrowings"]], [f"HS{i:02}" for i in range(1, 9)])
        hsf_cases = [
            (0, "commit", "a" * 40), (0, "tree", "b" * 40), (3, "blob_sha", "c" * 40), (3, "path", "harness2/core/other.py"),
            (1, "license", "MIT"), (1, "repository", "https://github.com/google/harness2"), (2, "read_range", "全文"),
            (16, "blob_sha", "d" * 40), (16, "license", "Apache-2.0"),
        ]
        for index, field, value in hsf_cases:
            with self.subTest(hsf=index, field=field):
                manifest = _valid_manifest_dict()
                manifest["traceability"]["hsf_sources"][index][field] = value
                self.assertIn("fixed Harness² source pin differs from plan §16.8", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        manifest["traceability"]["hsf_sources"][2]["e_tasks"] = ["E00"]
        self.assertIn("E mapping differs", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        manifest["traceability"]["hsf_sources"].pop()
        self.assertIn("hsf_sources must be exactly HSF01–HSF16 and HSP01", self._errors(manifest)[0])
        hs_cases = [
            (0, "section", "§6.7.5", "borrowing trace chain differs"), (0, "hsf_refs", ["HSF04"], "borrowing trace chain differs"),
            (2, "e_tasks", ["E09"], "borrowing trace chain differs"), (6, "v_scenarios", ["V052"], "borrowing trace chain differs"),
            (4, "adoption", "adopted", "borrowing trace chain differs"),
            (4, "implementation_status", "planned", "registered-only borrowing is not implemented"),
            (0, "verification_status", "verified", "incompatible verification"),
            (0, "effect_status", "improved", "specification/effect axes differ"),
            (0, "remaining", "", "remaining: must be a non-empty string"),
            (0, "related_evidence_refs", ["E01.controller_acceptance"], "outside the mapped E scope"),
            (0, "benefit", "improved", "unknown key(s) ['benefit']"),
        ]
        for index, field, value, expected in hs_cases:
            with self.subTest(hs=index, field=field):
                manifest = _valid_manifest_dict()
                manifest["traceability"]["hs_borrowings"][index][field] = value
                self.assertIn(expected, self._errors(manifest)[0])

    def test_v121_to_v123_v048_and_v081_follow_the_v45_responsibility(self):
        mapping = css.expected_v_to_e()
        self.assertEqual(mapping["V121"], {"E03", "E07", "E09", "E13", "E16"})
        self.assertEqual(mapping["V122"], {"E01", "E07", "E09", "E16"})
        self.assertEqual(mapping["V123"], {"E00", "E16", "E16.5", "E16.6"})
        self.assertEqual(mapping["V048"], {"E02", "E05"})
        self.assertNotIn("E06", mapping["V081"], "E06 only touches V081 (v4.5 Q-49)")
        for vid, task in (("V121", "E13"), ("V122", "E01"), ("V123", "E16.5"), ("V048", "E05")):
            with self.subTest(scenario=vid):
                manifest = _valid_manifest_dict()
                entry = next(entry for entry in manifest["traceability"]["v_scenarios"] if entry["id"] == vid)
                entry["e_tasks"].remove(task)  # probe d for V121/E13
                self.assertIn(f"{vid}: E responsibility differs", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        manifest["e_scopes"]["E06"]["scenarios"].append("V081")
        self.assertIn("E06: scenario execution responsibility differs", self._errors(manifest)[0])
        manifest = _valid_manifest_dict()
        for vid in ("V121", "V122", "V123"):
            entry = next(entry for entry in manifest["traceability"]["v_scenarios"] if entry["id"] == vid)
            self.assertEqual((entry["status"], entry["verification_status"], entry["related_evidence_refs"]), ("planned", "planned", []))

    def test_stage8_gate_scripts_are_registered_runners_and_coverage_spans_v123(self):
        for runner in ("ctrl_verify_v43.sh", "ctrl_verify_v45.sh"):
            css.validate_execution({"execution": _valid_execution(runner=runner)}, "E13.runner_fixture", 200)
        for runner in ("ctrl_verify_v44.sh", "codex-r7-ctrl_verify.sh"):
            with self.subTest(runner=runner), self.assertRaises(css.CheckerError):
                css.validate_execution({"execution": _valid_execution(runner=runner)}, "E13.runner_fixture", 200)
        for task, rng in (("E16.6", "HSP01"), ("E16.6", "HS01–HS08"), ("E00", "V087–V123"), ("E00", "HSF01–HSF16")):
            with self.subTest(task=task, range=rng):
                manifest = _valid_manifest_dict()
                manifest["trace_index_coverage"][task]["ranges"].remove(rng)
                self.assertIn(f"{task} trace index responsibility differs", self._errors(manifest)[0])

    def test_cli_options_for_the_missing_register_and_evidence_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            code, payload = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--evidence-root", str(Path(tmp) / "absent")])
            self.assertEqual(code, 1)
            self.assertIn("evidence root does not exist", payload["errors"][0])
            for extra in (["--known-missing", "reports/evidence-missing.json"], ["--evidence-root", tmp], ["--structure-only"]):
                for mode in ("--workspace-packages-only", "--project-license-metadata-only"):
                    with self.subTest(mode=mode, extra=extra), self.assertRaises(SystemExit) as caught:
                        css.main(["--repo-root", str(repo.root), mode] + extra)
                    self.assertEqual(caught.exception.code, 2)


def _identity_known(entry: dict) -> bool:
    return entry["expected_sha256"] != "unknown" and entry["expected_bytes"] != "unknown"


class FailClosedEvidenceJudgmentTests(unittest.TestCase):
    """AG-200 (plan §18.10 "本地缺失的证据路径", V080.d): without the local evidence store the checker fails closed unless
    --structure-only states that only the structure was checked; --evidence-root judges every out/ evidence path; the report
    states which judgment was made (evidence_judgment, evidence_unjudged)."""

    REPORT_KEYS = {
        "structure_valid", "plan_binding_available", "input_binding_available", "input_binding_scope", "needs_verification", "errors",
        "plan_version", "plan_sha256", "evidence_store_checked", "evidence_missing", "evidence_registered_not_checked", "evidence_judgment",
        "evidence_unjudged", "disclaimer",
    }

    def test_default_mode_fails_closed_on_unjudged_out_paths(self):
        manifest = _valid_manifest_dict()
        unjudged = _out_path_count(manifest)
        self.assertGreater(unjudged, 42)
        expected = f"evidence store not supplied: {unjudged} out/ evidence paths unjudged; pass --evidence-root <primary checkout> or --structure-only"
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            code, payload = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json"])
        self.assertFalse(report.structure_valid)
        self.assertEqual(report.errors, [expected])
        self.assertEqual((report.evidence_judgment, report.evidence_unjudged, report.evidence_store_checked), ("not_performed", unjudged, False))
        self.assertEqual(len(report.evidence_registered_not_checked), 42, "registered paths count as unjudged without the store")
        self.assertEqual(code, 1)
        self.assertEqual(payload["errors"], [expected])
        self.assertEqual(set(payload), self.REPORT_KEYS)

    def test_default_mode_reports_a_structural_defect_before_the_fail_closed_verdict(self):
        manifest = _valid_manifest_dict()
        del manifest["field_semantics"]
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            code, payload = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json"])
        self.assertEqual(code, 1)
        self.assertEqual(len(payload["errors"]), 1)
        self.assertIn("field_semantics: must be an object", payload["errors"][0])
        self.assertEqual(payload["evidence_judgment"], "not_performed")

    def test_structure_only_passes_and_states_that_the_judgment_was_not_performed(self):
        manifest = _valid_manifest_dict()
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            code, payload = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--structure-only"])
        self.assertEqual(code, 0, payload["errors"])
        self.assertTrue(payload["structure_valid"])
        self.assertEqual(payload["errors"], [])
        self.assertEqual(payload["evidence_judgment"], "not_performed_structure_only")
        self.assertEqual(payload["evidence_unjudged"], _out_path_count(manifest))
        self.assertFalse(payload["evidence_store_checked"])
        self.assertEqual(len(payload["evidence_registered_not_checked"]), 42)
        self.assertEqual(payload["evidence_missing"], [])
        self.assertEqual(set(payload), self.REPORT_KEYS)

    def test_structure_only_and_evidence_root_are_mutually_exclusive(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            for order in (["--structure-only", "--evidence-root", tmp], ["--evidence-root", tmp, "--structure-only"]):
                with self.subTest(order=order), self.assertRaises(SystemExit) as caught, contextlib.redirect_stderr(io.StringIO()):
                    css.main(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json"] + order)
                self.assertEqual(caught.exception.code, 2)
            report = css.run_check(repo.root, "reports/support-scope.json", None, None, Path(tmp), structure_only=True)
        self.assertFalse(report.structure_valid)
        self.assertIn("--structure-only and --evidence-root are mutually exclusive", report.errors[0])
        self.assertEqual(report.evidence_judgment, "not_performed_structure_only")

    def test_evidence_root_judges_every_out_path(self):
        manifest = _valid_manifest_dict()
        register = json.loads((REAL_ROOT / css.DEFAULT_KNOWN_MISSING).read_text(encoding="utf-8"))
        store = V45RebindTests._complete_store(manifest, register)
        with _repinned(manifest), tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp) / "checkout", manifest)
            evidence = Path(tmp) / "evidence-store"
            for rel, body in store.items():
                _write(evidence / rel)
                (evidence / rel).write_bytes(body)
            code, payload = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--evidence-root", str(evidence)])
        self.assertEqual(code, 0, payload["errors"])
        self.assertEqual(payload["errors"], [])
        self.assertEqual((payload["evidence_judgment"], payload["evidence_unjudged"], payload["evidence_store_checked"]), ("judged", 0, True))
        self.assertEqual(len(payload["evidence_missing"]), 42)
        self.assertEqual(payload["evidence_registered_not_checked"], [])
        self.assertEqual(set(payload), self.REPORT_KEYS)


class PathSafetyTests(unittest.TestCase):
    def test_rejects_dot_dot_path_escape(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E00"]["implementation_files"][0] = "../../etc/passwd"
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_absolute_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E00"]["implementation_files"][0] = "/etc/passwd"
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

    @unittest.skipUnless(hasattr(Path, "symlink_to"), "symlink unsupported")
    def test_rejects_symlink_component(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            target = repo.root / "scripts/smoke_workspace.py"
            target.unlink()
            target.symlink_to(repo.root / "scripts/inventory_baseline_tests.py")
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_directory_symlink_even_when_target_stays_inside_repo(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            real = repo.root / "real-scripts"
            real.mkdir()
            _write(real / "smoke_workspace.py")
            link = repo.root / "scripts-link"
            try:
                link.symlink_to(real, target_is_directory=True)
            except OSError as exc:
                self.skipTest(str(exc))
            manifest = repo.manifest
            manifest["e_scopes"]["E00"]["implementation_files"][0] = "scripts-link/smoke_workspace.py"
            repo.write_manifest(manifest)
            self.assertFalse(_structure_check(repo.root).structure_valid)

    def test_rejects_manifest_path_itself_escaping_repo_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            self.assertFalse(_structure_check(repo.root, "../support-scope.json").structure_valid)



class ProjectLicenseTests(unittest.TestCase):
    def with_repo(self, check):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = FixtureRepo(Path(tmp))
            check(fixture)

    def assert_refused(self, fixture):
        with self.assertRaises(css.CheckerError):
            css.validate_project_license(fixture.manifest, fixture.root)

    def test_healthy_metadata_only_and_full_manifest(self):
        def check(f):
            metadata = css.validate_project_license_metadata(f.root)
            self.assertEqual(metadata["sha256"], css.PROJECT_LICENSE_SHA256)
            self.assertEqual(metadata["members"], css.PROJECT_LICENSE_MEMBERS)
            self.assertEqual(set(metadata), {"schema_version", "status", "path", "sha256", "members"})
            css.validate_manifest(f.manifest, f.root)
            code, result = _run_cli(["--repo-root", str(f.root), "--project-license-metadata-only"])
            self.assertEqual((code, result), (0, metadata))
        self.with_repo(check)

    def test_missing_license_is_refused(self):
        def check(f):
            (f.root / "LICENSE").unlink()
            self.assert_refused(f)
        self.with_repo(check)

    def test_changed_license_even_with_coordinated_manifest_hash_is_refused(self):
        def check(f):
            (f.root / "LICENSE").write_text("MIT\n")
            f.manifest["project_license"]["sha256"] = hashlib.sha256(b"MIT\n").hexdigest()
            self.assert_refused(f)
        self.with_repo(check)

    def test_manifest_stale_hash_and_closed_fields_are_refused(self):
        def check(f):
            original = copy.deepcopy(f.manifest["project_license"])
            for altered in [None, {}, {**original, "extra":False}, {**original, "status":"MIT"},
                            {**original, "path":"../LICENSE"}, {**original, "sha256":True},
                            {**original, "sha256":original["sha256"].upper()}, {**original, "sha256":"0"*64}]:
                with self.subTest(altered=altered):
                    f.manifest["project_license"] = altered
                    self.assert_refused(f)
            f.manifest["project_license"] = original
            css.validate_project_license(f.manifest, f.root)
        self.with_repo(check)

    def test_member_direct_license_and_publish_override_are_refused(self):
        def check(f):
            for member in css.PROJECT_LICENSE_MEMBERS:
                p = f.root / member / "Cargo.toml"
                healthy = p.read_text()
                for before, after in [('license-file.workspace = true','license = "MIT"'),
                                      ('publish.workspace = true','publish = true')]:
                    with self.subTest(member=member, after=after):
                        p.write_text(healthy.replace(before, after))
                        self.assert_refused(f)
                p.write_text(healthy)
            css.validate_project_license(f.manifest, f.root)
        self.with_repo(check)

    def test_member_inheritance_requires_exact_bool_table(self):
        def check(f):
            p = f.root / "crates/evo-core/Cargo.toml"
            healthy = p.read_text()
            for field in ("publish", "license-file"):
                for bad in ('1', '0', 'false', '"true"', '[true]'):
                    with self.subTest(field=field, bad=bad):
                        p.write_text(healthy.replace(f'{field}.workspace = true', f'{field}.workspace = {bad}'))
                        self.assert_refused(f)
                p.write_text(healthy.replace(f'{field}.workspace = true', f'{field} = {{workspace=true, extra=false}}'))
                self.assert_refused(f)
            p.write_text(healthy)
            css.validate_project_license(f.manifest, f.root)
        self.with_repo(check)

    def test_root_requires_false_publish_and_no_parallel_license(self):
        def check(f):
            p = f.root / "Cargo.toml"
            healthy = p.read_text()
            for bad in ('0', 'true', '"false"'):
                p.write_text(healthy.replace('publish = false', f'publish = {bad}'))
                self.assert_refused(f)
            p.write_text(healthy + 'license = "MIT"\n')
            self.assert_refused(f)
            p.write_text(healthy.replace('license-file = "LICENSE"', 'license-file = "other"'))
            self.assert_refused(f)
        self.with_repo(check)

    def test_workspace_exact_members_and_duplicate_toml_keys(self):
        def check(f):
            p = f.root / "Cargo.toml"
            healthy = p.read_text()
            for bad in [healthy.replace('"apps/rsia"', '"apps/rsia", "apps/rsia"'),
                        healthy.replace('"apps/rsia"', '"extra"'), healthy + 'publish = false\n']:
                p.write_text(bad)
                self.assert_refused(f)
        self.with_repo(check)

    def test_readme_unset_duplicate_and_extra_license_text_are_refused(self):
        def check(f):
            p = f.root / "README.md"
            healthy = p.read_text()
            for bad in ['## License\n\nunset\n', healthy + '\n## License\n\nMIT\n',
                        healthy + '\nYou may redistribute this project.\n',
                        healthy + '\n## Project redistribution grant\n\nThis project is released under the MIT License.\n',
                        healthy + '\n# Project open-source release\n\nMIT\n']:
                p.write_text(bad)
                self.assert_refused(f)
            p.write_text('## Third-party notices\n\nUpstream MIT notice remains applicable.\n\n' + healthy)
            css.validate_project_license(f.manifest, f.root)
        self.with_repo(check)

    def test_license_readme_and_cargo_symlinks_are_refused(self):
        def check(f):
            for relative in ('LICENSE', 'README.md', 'Cargo.toml', 'crates/evo-core/Cargo.toml'):
                path = f.root / relative
                saved = path.read_bytes()
                target = path.with_name(path.name + '.real')
                path.rename(target)
                path.symlink_to(target.name)
                self.assert_refused(f)
                path.unlink()
                target.rename(path)
                self.assertEqual(path.read_bytes(), saved)
        self.with_repo(check)

    def test_raw_duplicate_license_keys_are_not_collapsed(self):
        for raw in ['{"project_license":{},"project_license":{}}',
                    '{"project_license":{"status":"MIT","status":"source_available_personal_noncommercial_free"}}']:
            with self.assertRaises(css.CheckerError):
                css.load_json_strict(raw)

    def test_metadata_only_forbids_manifest_and_truth_arguments(self):
        def check(f):
            for extra in (['--manifest', 'reports/support-scope.json'], ['--source-of-truth', 'truth.md']):
                with self.assertRaises(SystemExit) as caught:
                    css.main(['--repo-root', str(f.root), '--project-license-metadata-only'] + extra)
                self.assertEqual(caught.exception.code, 2)
        self.with_repo(check)

class RealRepositoryTests(unittest.TestCase):
    def test_real_manifest_report(self):
        code, payload = _run_cli(["--repo-root", str(REAL_ROOT), "--manifest", "reports/support-scope.json", "--structure-only"])
        print(f"[real] exit={code} structure={payload.get('structure_valid')} plan={payload.get('plan_binding_available')} input={payload.get('input_binding_available')} errors={payload.get('errors')}", file=sys.stderr)
        self.assertEqual(code, 0)
        self.assertTrue(payload["structure_valid"])
        self.assertFalse(payload["plan_binding_available"])
        self.assertFalse(payload["input_binding_available"])
        self.assertEqual((payload["plan_version"], payload["plan_sha256"]), (css.PLAN_VERSION, css.PLAN_SHA256))
        self.assertFalse(payload["evidence_store_checked"])
        self.assertEqual(payload["evidence_judgment"], "not_performed_structure_only")
        self.assertGreater(payload["evidence_unjudged"], 0)


class IndexOwnershipAndMergeFactsTests(unittest.TestCase):
    def _report(self, manifest: dict) -> css.CheckReport:
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            return _structure_check(repo.root)

    def test_runtime_reads_the_single_reviewed_rust_table(self):
        facts = css.reviewed_merge_facts(REAL_ROOT)
        self.assertEqual(len(facts), 132)
        # The AG-182 rebind PR, unmerged when its records were registered; AG-200 backfilled its merge.
        self.assertEqual(facts[200], "02a135aa918fce5dd2ea0c1c8eafc287318f10d2")
        self.assertNotIn(None, facts.values())
        records = [record for scope in _valid_manifest_dict()["e_scopes"].values() for record in scope["verified_subscopes"]]
        self.assertEqual(len(records), 138)
        for record in records:
            self.assertEqual(record["merged_sha"], facts[int(record["pr"].rsplit("/", 1)[1])])
            self.assertNotEqual(record["source_sha"], record["merged_sha"])

    def test_cli_rejects_missing_duplicate_or_unsupported_rust_fact_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            path = repo.root / "crates/evo-core/tests/support_scope.rs"
            original = path.read_text(encoding="utf-8")
            arm = next(line for line in original.splitlines() if "=> Some(" in line)
            cases = {
                "missing function": "fn other() {}\n",
                "comment-only function": "/*\n" + original + "\n*/",
                "raw-string-only function": 'const TABLE: &str = r####"\n' + original + '\n"####;\n',
                "duplicate function": original + "\nfn expected_merge_for_pr() {}\n",
                "duplicate arm": original.replace(arm, arm + "\n" + arm, 1),
                "unknown arm": original.replace(arm, '        29 => derive_merge(),', 1),
                "unknown fallback": original.replace('no reviewed merge status for PR {pr}', 'unknown PR {pr}', 1),
                "empty table": original[:original.index("fn expected_merge_for_pr")] + "fn expected_merge_for_pr(pr: u64) -> Result<Option<&'static str>, String> {\n    Ok(match pr {\n        _ => return Err(format!(\"no reviewed merge status for PR {pr}\")),\n    })\n}\n",
            }
            for label, source in cases.items():
                with self.subTest(case=label):
                    path.write_text(source, encoding="utf-8")
                    code, report = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--structure-only"])
                    self.assertEqual(code, 1)
                    self.assertFalse(report["structure_valid"])
                    self.assertTrue(any("reviewed merge facts" in error for error in report["errors"]), report)
            path.unlink()
            code, report = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--structure-only"])
            self.assertEqual(code, 1)
            self.assertFalse(report["structure_valid"])

    def test_self_merge_and_same_pr_shared_forgery_are_rejected(self):
        manifest = _valid_manifest_dict()
        record = manifest["e_scopes"]["E01"]["verified_subscopes"][0]
        record["source_sha"] = record["merged_sha"]
        report = self._report(manifest)
        self.assertFalse(report.structure_valid)
        self.assertIn("must differ from its own source_sha", report.errors[0])
        manifest = _valid_manifest_dict()
        record = manifest["e_scopes"]["E01"]["verified_subscopes"][0]
        sibling = _fresh_record(record, "E01", "second_reviewed_target", "same-pr")
        record["merged_sha"] = sibling["merged_sha"] = "a" * 40
        manifest["e_scopes"]["E01"]["verified_subscopes"].append(sibling)
        report = self._report(manifest)
        self.assertFalse(report.structure_valid)
        self.assertIn("differs from the reviewed PR merge", report.errors[0])

    def test_unknown_pr_is_not_an_implicit_unmerged_fact(self):
        manifest = _valid_manifest_dict()
        record = _fresh_record(manifest["e_scopes"]["E03"]["verified_subscopes"][0], "E03", "unreviewed_pr", "unknown-pr")
        record.update(pr="https://github.com/acosmi/RSIAgent/pull/999", merged_sha=None, pr_state="draft")
        manifest["e_scopes"]["E03"]["verified_subscopes"].append(record)
        report = self._report(manifest)
        self.assertFalse(report.structure_valid)
        self.assertIn("no reviewed merge status for PR 999", report.errors[0])
        # Synthetic parser control only: None is legal solely when it has an explicit Rust arm.
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            path = repo.root / "crates/evo-core/tests/support_scope.rs"
            original = path.read_text(encoding="utf-8")
            path.write_text(original.replace('        _ => return Err(format!("no reviewed merge status for PR {pr}")),', '        999 => None,\n        _ => return Err(format!("no reviewed merge status for PR {pr}")),', 1), encoding="utf-8")
            report = _structure_check(repo.root)
            self.assertTrue(report.structure_valid, report.errors)
            record["merged_sha"] = "b" * 40
            record["pr_state"] = "merged"
            repo.write_manifest(manifest)
            report = _structure_check(repo.root)
            self.assertFalse(report.structure_valid)
            self.assertIn("differs from the reviewed PR merge", report.errors[0])

    def test_namespace_characters_and_e16_subtask_ownership_are_precise(self):
        for task, identifier in [("E01", "E02.borrowed"), ("E01", "E01.bad/name"), ("E16.5", "E16.50.borrowed")]:
            with self.subTest(task=task, identifier=identifier):
                manifest = _valid_manifest_dict()
                manifest["e_scopes"][task]["verified_subscopes"][0]["id"] = identifier
                report = self._report(manifest)
                self.assertFalse(report.structure_valid)
                self.assertTrue("evidence id is invalid" in report.errors[0] or "not namespaced under its own task" in report.errors[0])
        manifest = _valid_manifest_dict()
        parent = manifest["e_scopes"]["E16"]
        parent["status"] = "in_progress"
        record = _fresh_record(manifest["e_scopes"]["E16.5"]["verified_subscopes"][0], "E16.5", "parent_borrow", "parent-borrow")
        parent["verified_subscopes"].append(record)
        report = self._report(manifest)
        self.assertFalse(report.structure_valid)
        self.assertIn("not namespaced under its own task", report.errors[0])

    def test_all_trace_groups_enforce_evidence_owner_and_empty_so_is_healthy(self):
        for group in ("v_scenarios", "b_commitments", "u_commitments", "k_commitments", "so_sources"):
            with self.subTest(group=group):
                manifest = _valid_manifest_dict()
                entry = next(entry for entry in manifest["traceability"][group] if "E01" not in entry["e_tasks"])
                field = "verification_evidence_refs" if group == "so_sources" else "related_evidence_refs"
                entry.setdefault(field, []).append("E01.controller_acceptance")
                report = self._report(manifest)
                self.assertFalse(report.structure_valid)
                self.assertIn("outside the mapped E scope", report.errors[0])
        manifest = _valid_manifest_dict()
        for entry in manifest["traceability"]["so_sources"]:
            entry["verification_evidence_refs"] = []
        report = self._report(manifest)
        self.assertTrue(report.structure_valid, report.errors)

    def test_test_entry_requires_its_own_task_crate_or_exact_file(self):
        manifest = _valid_manifest_dict()
        record = manifest["e_scopes"]["E01"]["verified_subscopes"][0]
        other = manifest["e_scopes"]["E03"]["verified_subscopes"][0]
        record.update(test_entry=other["test_entry"], command=other["command"])
        report = self._report(manifest)
        self.assertFalse(report.structure_valid)
        self.assertIn("not among E01's declared implementation_files nor inside a crate they declare", report.errors[0])
        for files in (["crates/evo-core/src/holdout.rs"], ["crates/evo-core/tests/another_target.rs"], ["crates/evo-core-extra/src/holdout.rs"]):
            with self.subTest(files=files):
                manifest = _valid_manifest_dict()
                scope = manifest["e_scopes"]["E01"]
                scope["implementation_files"] = files
                record = scope["verified_subscopes"][0]
                record.update(test_entry="crates/evo-core/tests/another_target.rs", command="cargo test --locked --offline -p evo-core --test another_target")
                with _repinned(manifest):  # the ownership rule under test, not the historical digest pin
                    report = self._report(manifest)
                self.assertEqual(report.structure_valid, not files[0].startswith("crates/evo-core-extra/"), report.errors)


class SharedWorkspaceAndRawFunctionTests(unittest.TestCase):
    def test_raw_fact_function_declarations_are_ambiguous_or_unsupported(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            path = repo.root / "crates/evo-core/tests/support_scope.rs"
            original = path.read_text(encoding="utf-8")
            for label, source in (
                ("ordinary plus raw", original + "\nfn r#expected_merge_for_pr() {}\n"),
                ("raw only", original.replace("fn expected_merge_for_pr(", "fn r#expected_merge_for_pr(", 1)),
            ):
                with self.subTest(case=label):
                    path.write_text(source, encoding="utf-8")
                    code, report = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--structure-only"])
                    self.assertEqual(code, 1)
                    self.assertFalse(report["structure_valid"])
                    self.assertTrue(any("reviewed merge facts" in error for error in report["errors"]), report)
            path.write_text(original + '\n// fn r#expected_merge_for_pr() {}\nconst DOC: &str = "fn r#expected_merge_for_pr() {}";\n', encoding="utf-8")
            self.assertEqual(css.reviewed_merge_facts(repo.root), css.reviewed_merge_facts(REAL_ROOT))

    def test_workspace_metadata_uses_actual_member_and_never_recurses_into_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            member = "components/core-implementation"
            _write(root / "Cargo.toml", f'[workspace]\nmembers = ["{member}"]\n')
            _write(root / member / "Cargo.toml", '[package]\nname = "evo-core"\n')
            _write(root / member / "src/lib.rs", "pub mod evaluator;\n")
            _write(root / member / "src/evaluator.rs")
            _write(root / member / "tests/target.rs")
            _write(root / "scripts/check_support_scope.py", "raise AssertionError('untrusted fixture helper')\n")
            with mock.patch.object(css, "validate_manifest", side_effect=AssertionError("manifest recursion")), mock.patch.object(css, "reviewed_merge_facts", side_effect=AssertionError("fact recursion")):
                code, mapping = _run_cli(["--repo-root", str(root), "--workspace-packages-only"])
            self.assertEqual(code, 0)
            self.assertEqual(mapping, {"evo-core": member})
            packages = css.workspace_packages(root)
            self.assertEqual(css.resolve_cargo_target(root, "cargo test --locked --offline -p evo-core --test target", packages), member + "/tests/target.rs")
            self.assertEqual(css.resolve_cargo_target(root, "cargo test --locked --offline -p evo-core --lib evaluator::tests", packages), member + "/src/evaluator.rs")
            # A conventional package directory remaining on disk is not a workspace declaration.
            _write(root / "crates/evo-core/Cargo.toml", '[package]\nname = "evo-core"\n')
            _write(root / "crates/evo-core/tests/target.rs")
            _write(root / "other/Cargo.toml", '[package]\nname = "other"\n')
            _write(root / "Cargo.toml", '[workspace]\nmembers = ["other"]\n')
            with self.assertRaisesRegex(css.CheckerError, "undeclared workspace package"):
                css.resolve_cargo_target(root, "cargo test --locked --offline -p evo-core --test target", css.workspace_packages(root))
            for label, cargo in (
                ("duplicate member", f'[workspace]\nmembers = ["{member}", "{member}"]\n'),
                ("duplicate package", f'[workspace]\nmembers = ["{member}", "crates/evo-core"]\n'),
                ("escape", '[workspace]\nmembers = ["../outside"]\n'),
                ("glob", '[workspace]\nmembers = ["components/*"]\n'),
                ("unknown workspace shape", 'workspace = []\n'),
                ("unsupported members shape", '[workspace]\nmembers = "other"\n'),
                ("duplicate TOML key", f'[workspace]\nmembers = ["{member}"]\nmembers = ["other"]\n'),
            ):
                with self.subTest(case=label):
                    _write(root / "Cargo.toml", cargo)
                    code, report = _run_cli(["--repo-root", str(root), "--workspace-packages-only"])
                    self.assertEqual(code, 1)
                    self.assertTrue(report["errors"])


class CompleteFactFunctionDeclarationTests(unittest.TestCase):
    def test_function_qualifiers_are_rejected_across_complete_item_prefix(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            path = repo.root / "crates/evo-core/tests/support_scope.rs"
            original = path.read_text(encoding="utf-8")
            declaration = "fn expected_merge_for_pr("
            for prefix in (
                "async ", "async\n", "pub ", "pub\n", "pub(crate)\n",
                'unsafe extern "C" ', 'unsafe\nextern "C"\n', "const\n",
                '#[cfg(any())]\n', 'async /* comment with fn and braces {} */\n',
            ):
                with self.subTest(prefix=prefix):
                    path.write_text(original.replace(declaration, prefix + declaration, 1), encoding="utf-8")
                    code, report = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--structure-only"])
                    self.assertEqual(code, 1)
                    self.assertFalse(report["structure_valid"])
                    self.assertTrue(any("ordinary private synchronous function" in error for error in report["errors"]), report)
            # Trivia and a separate completed string-valued item are healthy.
            trivia = '// async pub fn r#expected_merge_for_pr() {}\n/* unsafe extern "C"\nfn expected_merge_for_pr() {} */\n'
            doc_item = 'const FACT_DOC: &str = r##"pub async fn expected_merge_for_pr() {}"##;\n'
            for prefix in ("", trivia, doc_item + trivia):
                with self.subTest(healthy_prefix=prefix):
                    path.write_text(original.replace(declaration, prefix + declaration, 1), encoding="utf-8")
                    self.assertEqual(css.reviewed_merge_facts(repo.root), css.reviewed_merge_facts(REAL_ROOT))


class FactFunctionTokenTreeTests(unittest.TestCase):
    def test_macro_token_trees_and_bad_delimiters_cannot_supply_fact_function(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            path = repo.root / "crates/evo-core/tests/support_scope.rs"
            original = path.read_text(encoding="utf-8")
            start = original.index("fn expected_merge_for_pr(")
            end = original.index("\n}\n", start) + 2
            function = original[start:end]
            # This macro can discard all table tokens and generate a different function.
            macro = "macro_rules! discard_table { ($name:ident; $($body:tt)*) => { fn $name(_: u64) -> Result<Option<&'static str>, String> { Ok(None) } }; }\n"
            cases = [
                macro + original.replace(function, "discard_table!(expected_merge_for_pr;\n" + function + "\n);", 1),
                macro + original.replace(function, "discard_table![expected_merge_for_pr;\n" + function + "\n];", 1),
                macro + original.replace(function, "discard_table!(expected_merge_for_pr; [({\n" + function + "\n})]);", 1),
                "[)\n" + original,
                original + "\n(",
                original + "\n]",
            ]
            for index, source in enumerate(cases):
                with self.subTest(case=index):
                    path.write_text(source, encoding="utf-8")
                    code, report = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json", "--structure-only"])
                    self.assertEqual(code, 1)
                    self.assertFalse(report["structure_valid"])
                    self.assertTrue(any("top-level Rust function" in error or "delimiters" in error for error in report["errors"]), report)
            prefix = 'const PREVIOUS_ITEM: [(); 1] = [()];\n// ([)]\nconst DELIMITER_DOC: &str = "([)]";\n'
            path.write_text(original.replace(function, prefix + function, 1), encoding="utf-8")
            self.assertEqual(css.reviewed_merge_facts(repo.root), css.reviewed_merge_facts(REAL_ROOT))

if __name__ == "__main__":
    unittest.main()
