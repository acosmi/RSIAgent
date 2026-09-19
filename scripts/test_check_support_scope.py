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
        _write(root / "Cargo.toml", "[workspace]\nmembers = [" + ",".join(json.dumps(x) for x in members) + "]\n")
        for member in members:
            name = member.rsplit("/", 1)[1]
            _write(root / member / "Cargo.toml", f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2024"\n')
            _write(root / member / "src/lib.rs", "pub mod evaluator;\npub mod hosts;\n")
        self.write_manifest(self.manifest)
    def write_manifest(self, manifest: dict, rel: str = "reports/support-scope.json") -> None:
        _write(self.root / rel, json.dumps(manifest, ensure_ascii=False, indent=2))

def _run_cli(args: list[str]) -> tuple[int, dict]:
    output = io.StringIO()
    with contextlib.redirect_stdout(output):
        code = css.main(args)
    return code, json.loads(output.getvalue())

class ValidManifestTests(unittest.TestCase):
    def test_valid_manifest_passes_structurally_and_exits_zero(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertTrue(report.structure_valid, report.errors)
            code, payload = _run_cli(["--repo-root", str(repo.root), "--manifest", "reports/support-scope.json"])
            self.assertEqual(code, 0)
            self.assertTrue(payload["structure_valid"])
            self.assertFalse(payload["plan_binding_available"])
            self.assertFalse(payload["input_binding_available"])
            self.assertEqual(payload["input_binding_scope"], "local_input_manifest_hashes_only")

    def test_missing_source_of_truth_is_reported_as_unverified_not_silently_ok(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            report = css.run_check(repo.root, "reports/support-scope.json", None)
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
            repo.manifest["plan_sha256"] = digest
            for scope in repo.manifest["e_scopes"].values():
                for record in scope["verified_subscopes"]:
                    record["plan_sha256"] = digest
            repo.write_manifest(repo.manifest)
            with mock.patch.object(css, "PLAN_SHA256", digest):
                report = css.run_check(repo.root, "reports/support-scope.json", plan)
            self.assertTrue(report.structure_valid, report.errors)
            self.assertTrue(report.plan_binding_available)
            self.assertFalse(report.input_binding_available)

    def test_mismatched_source_of_truth_hash_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            plan = Path(tmp) / "plan.md"
            plan.write_text("wrong plan\n", encoding="utf-8")
            report = css.run_check(repo.root, "reports/support-scope.json", plan)
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
            report = css.run_check(repo.root, "reports/support-scope.json", None)
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
                    "pr": "https://github.com/acosmi/RSIAgent/pull/999",
                    "merged_sha": None,
                    "input_ref": "out/future/input.json",
                    "input_digest": hashlib.sha256(input_bytes).hexdigest(),
                    "log_path": "out/future/test.log",
                    "record_source": "out/future/acceptance.json",
                    "actual_result": "fixture record exercises schema extensibility",
                    "scope": "synthetic structural fixture only",
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
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertTrue(report.structure_valid, report.errors)

class RejectionTests(unittest.TestCase):
    def test_rejects_impl_file_that_does_not_exist_on_disk(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E00"]["implementation_files"][0] = "scripts/never_written.py"
            repo = FixtureRepo(Path(tmp), manifest)
            # Fixture creation wrote the declared fake; remove it to retain the original protection semantic.
            (repo.root / "scripts/never_written.py").unlink()
            report = css.run_check(repo.root, "reports/support-scope.json", None)
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
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_test_command_target_not_listed_in_test_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E01"]["verified_subscopes"][0]["command"] = "cargo test --locked --offline -p evo-core --test other"
            repo = FixtureRepo(Path(tmp), manifest)
            _write(repo.root / "crates/evo-core/tests/other.rs")
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_duplicate_id_in_a_commitment_set(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["traceability"]["b_commitments"][-1] = copy.deepcopy(manifest["traceability"]["b_commitments"][0])
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_id_set_with_correct_length_but_wrong_members(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["traceability"]["b_commitments"][-1]["id"] = "B11"
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_so_source_with_illegal_hash_format(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["traceability"]["so_sources"][0]["commit"] = "not-a-real-sha"
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_verified_status_with_no_test_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E17"]["status"] = "verified"
            manifest["e_scopes"]["E17"]["remaining"] = []
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E03"]["status"] = "verified"
            manifest["e_scopes"]["E03"]["remaining"] = []
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(
                css.run_check(repo.root, "reports/support-scope.json", None).structure_valid,
                "an existing scoped record must not automatically verify the whole E task",
            )

    def test_planned_status_tolerates_empty_test_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            self.assertEqual(repo.manifest["e_scopes"]["E18"]["status"], "planned")
            self.assertTrue(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_missing_e16_parent_e17_or_e18(self):
        for missing in ("E16", "E17", "E18"):
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as tmp:
                manifest = _valid_manifest_dict()
                del manifest["e_scopes"][missing]
                repo = FixtureRepo(Path(tmp), manifest)
                self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_well_formed_but_wrong_so_commit_and_blob(self):
        for field in ("commit", "blob_sha"):
            with self.subTest(field=field), tempfile.TemporaryDirectory() as tmp:
                manifest = _valid_manifest_dict()
                manifest["traceability"]["so_sources"][0][field] = "a" * 40
                repo = FixtureRepo(Path(tmp), manifest)
                self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_nonexistent_e99_trace_reference(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["traceability"]["so_sources"][0]["e_tasks"] = ["E99"]
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_unknown_status(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E01"]["status"] = "verified_subset"
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

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
                self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_duplicate_json_object_key(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            raw = (repo.root / "reports/support-scope.json").read_text(encoding="utf-8")
            raw = raw.replace('{\n  "schema_version"', '{\n  "schema_version": "rsia.support_scope.v2",\n  "schema_version"', 1)
            (repo.root / "reports/support-scope.json").write_text(raw, encoding="utf-8")
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("duplicate JSON object key" in item for item in report.errors))

    def test_missing_input_is_needs_verification_but_wrong_hash_is_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            missing = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertTrue(missing.structure_valid)
            self.assertFalse(missing.input_binding_available)
            first = repo.manifest["e_scopes"]["E00"]["verified_subscopes"][0]
            _write(repo.root / first["input_ref"], "wrong bytes")
            mismatch = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(mismatch.structure_valid)
            self.assertTrue(any("input manifest SHA-256 mismatch" in item for item in mismatch.errors))

    def test_rejects_nonzero_verified_exit_code(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E01"]["verified_subscopes"][0]["exit_code"] = 101
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_boolean_and_float_zero_exit_codes(self):
        for value in (False, 0.0):
            with self.subTest(value=value), tempfile.TemporaryDirectory() as tmp:
                manifest = _valid_manifest_dict()
                manifest["e_scopes"]["E01"]["verified_subscopes"][0][
                    "exit_code"
                ] = value
                repo = FixtureRepo(Path(tmp), manifest)
                self.assertFalse(
                    css.run_check(
                        repo.root, "reports/support-scope.json", None
                    ).structure_valid
                )

    def test_rejects_range_or_trace_chain_disconnect(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["trace_index_coverage"]["E16.6"]["ranges"].remove("V001–V098")
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["traceability"]["k_commitments"][4]["e_tasks"] = ["E18"]
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

class PathSafetyTests(unittest.TestCase):
    def test_rejects_dot_dot_path_escape(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E00"]["implementation_files"][0] = "../../etc/passwd"
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_absolute_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E00"]["implementation_files"][0] = "/etc/passwd"
            repo = FixtureRepo(Path(tmp), manifest)
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    @unittest.skipUnless(hasattr(Path, "symlink_to"), "symlink unsupported")
    def test_rejects_symlink_component(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            target = repo.root / "scripts/smoke_workspace.py"
            target.unlink()
            target.symlink_to(repo.root / "scripts/inventory_baseline_tests.py")
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

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
            self.assertFalse(css.run_check(repo.root, "reports/support-scope.json", None).structure_valid)

    def test_rejects_manifest_path_itself_escaping_repo_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            self.assertFalse(css.run_check(repo.root, "../support-scope.json", None).structure_valid)

class RealRepositoryTests(unittest.TestCase):
    def test_real_manifest_report(self):
        code, payload = _run_cli(["--repo-root", str(REAL_ROOT), "--manifest", "reports/support-scope.json"])
        print(f"[real] exit={code} structure={payload.get('structure_valid')} plan={payload.get('plan_binding_available')} input={payload.get('input_binding_available')} errors={payload.get('errors')}", file=sys.stderr)
        self.assertEqual(code, 0)
        self.assertTrue(payload["structure_valid"])
        self.assertFalse(payload["plan_binding_available"])
        self.assertFalse(payload["input_binding_available"])

if __name__ == "__main__":
    unittest.main()
