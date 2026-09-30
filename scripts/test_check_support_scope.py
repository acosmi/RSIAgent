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

def _fresh_record(base: dict, task: str, name: str, stem: str) -> dict:
    """Copy an existing record but give it its own id, PR, source, log, input and record_source."""
    record = copy.deepcopy(base)
    record.pop("pr_state", None)  # a fresh unmerged record starts without a recorded PR state
    record.update(
        {
            "id": f"{task}.{name}",
            "source_sha": hashlib.sha1(f"{task}.{name}".encode()).hexdigest(),
            "pr": "https://github.com/acosmi/RSIAgent/pull/999",
            "merged_sha": None,
            "log_path": f"out/fixture/{stem}.log",
            "input_ref": f"out/fixture/{stem}-input.json",
            "input_digest": hashlib.sha256(stem.encode()).hexdigest(),
            "record_source": f"out/fixture/{stem}-acceptance.json",
        }
    )
    return record

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
                    "pr_state": "draft",
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
            self.assertEqual(repo.manifest["e_scopes"]["E14"]["status"], "planned")
            self.assertEqual(repo.manifest["e_scopes"]["E14"]["verified_subscopes"], [])
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

class ControllerFalseAcceptanceTests(unittest.TestCase):
    """One rejection per false acceptance reproduced by the controller (2026-09-30), plus legitimate counterparts."""

    def _errors(self, manifest: dict) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            report = css.run_check(repo.root, "reports/support-scope.json", None)
        self.assertFalse(report.structure_valid, "manipulated manifest must be rejected")
        return report.errors

    def _assert_valid(self, manifest: dict) -> css.CheckReport:
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp), manifest)
            report = css.run_check(repo.root, "reports/support-scope.json", None)
        self.assertTrue(report.structure_valid, report.errors)
        return report

    def test_rejects_planned_scope_carrying_verified_evidence(self):
        manifest = _valid_manifest_dict()
        e14 = manifest["e_scopes"]["E14"]
        self.assertEqual(e14["status"], "planned")
        e14["verified_subscopes"].append(
            _fresh_record(manifest["e_scopes"]["E13"]["verified_subscopes"][0], "E14", "controller_acceptance", "e14")
        )
        errors = self._errors(manifest)
        self.assertIn(
            "E14: status 'planned' is incompatible with verified evidence records ['E14.controller_acceptance']",
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
            "E16: status 'verified' is stronger than sub-package E16.1 status 'implemented_not_verified'",
            errors[0],
        )
        with self.subTest(variant="implemented_not_verified parent over a blocked child"):
            manifest = _valid_manifest_dict()
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
        e13, e14 = manifest["e_scopes"]["E13"], manifest["e_scopes"]["E14"]
        e14["status"] = "in_progress"
        e14["implementation_files"] = list(e13["implementation_files"])
        borrowed = copy.deepcopy(e13["verified_subscopes"][0])
        borrowed["id"] = "E14.controller_acceptance"
        e14["verified_subscopes"].append(borrowed)
        errors = self._errors(manifest)
        self.assertIn(
            f"E14.controller_acceptance: log_path {borrowed['log_path']!r} duplicates evidence E13.controller_acceptance of task E13",
            errors[0],
        )
        with self.subTest(variant="same test run re-logged under another task"):
            manifest = _valid_manifest_dict()
            e13, e14 = manifest["e_scopes"]["E13"], manifest["e_scopes"]["E14"]
            e14["status"] = "in_progress"
            rerun = copy.deepcopy(e13["verified_subscopes"][0])
            rerun.update({"id": "E14.controller_acceptance", "log_path": "out/fixture/e14-rerun.log"})
            e14["verified_subscopes"].append(rerun)
            errors = self._errors(manifest)
            self.assertIn(
                f"the same test run ({rerun['test_entry']} at source_sha {rerun['source_sha']}) is already claimed by E13.controller_acceptance of task E13",
                errors[0],
            )
        with self.subTest(variant="evidence id namespaced under a foreign task"):
            manifest = _valid_manifest_dict()
            e14 = manifest["e_scopes"]["E14"]
            e14["status"] = "in_progress"
            e14["verified_subscopes"].append(
                _fresh_record(manifest["e_scopes"]["E13"]["verified_subscopes"][0], "E13", "second_controller_acceptance", "e13-second")
            )
            errors = self._errors(manifest)
            self.assertIn("E14: evidence id 'E13.second_controller_acceptance' is not namespaced under its own task", errors[0])
        with self.subTest(variant="test target outside the task's declared crates"):
            manifest = _valid_manifest_dict()
            e14 = manifest["e_scopes"]["E14"]
            e14["status"] = "in_progress"
            e14["verified_subscopes"].append(
                _fresh_record(manifest["e_scopes"]["E01"]["verified_subscopes"][0], "E14", "controller_acceptance", "e14-foreign")
            )
            errors = self._errors(manifest)
            self.assertIn(
                "E14.controller_acceptance: test_entry 'crates/evo-core/tests/evaluation_v41.rs' (package evo-core) is not among E14's declared implementation_files nor inside a crate they declare",
                errors[0],
            )

    def test_rejects_unknown_keys_asserting_effect_or_verdict(self):
        manifest = _valid_manifest_dict()
        first_v = manifest["traceability"]["v_scenarios"][0]["id"]
        first_b = manifest["traceability"]["b_commitments"][0]["id"]
        first_so = manifest["traceability"]["so_sources"][0]["id"]
        cases = [
            ("scope entry", ("e_scopes", "E13"), "effect", "E13 scope entry: unknown key(s) ['effect']"),
            ("evidence record", ("e_scopes", "E13", "verified_subscopes", 0), "verdict", "E13 evidence E13.controller_acceptance: unknown key(s) ['verdict']"),
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
            self.assertTrue(record["pr"].endswith("/47"))
            return record

        merged_pr47 = pr47(_valid_manifest_dict())
        self.assertEqual(merged_pr47["pr_state"], "merged")
        self.assertTrue(merged_pr47["merged_sha"])
        pr47_id = merged_pr47["id"]
        with self.subTest(variant="merged_sha copied from another PR's merge commit"):
            manifest = _valid_manifest_dict()
            record = pr47(manifest)
            record["merged_sha"] = manifest["e_scopes"]["E13"]["verified_subscopes"][0]["merged_sha"]
            errors = self._errors(manifest)
            self.assertIn(
                f"{pr47_id}: merged_sha {record['merged_sha']} is already recorded as the merge commit of PR #42 (E13.controller_acceptance)",
                errors[0],
            )
        with self.subTest(variant="merged_sha equal to another PR's source_sha"):
            manifest = _valid_manifest_dict()
            pr47(manifest)["merged_sha"] = manifest["e_scopes"]["E13"]["verified_subscopes"][0]["source_sha"]
            errors = self._errors(manifest)
            self.assertIn("is the source_sha of PR #42 (E13.controller_acceptance), not a merge of PR #47", errors[0])
        with self.subTest(variant="same PR merged in one record and unmerged in another"):
            manifest = _valid_manifest_dict()
            second = _fresh_record(pr47(manifest), "E16.4", "second_controller_acceptance", "pr47-second")
            second.update({"pr": pr47(manifest)["pr"], "merged_sha": None})
            manifest["e_scopes"]["E16.4"]["verified_subscopes"].append(second)
            errors = self._errors(manifest)
            self.assertIn(
                f"E16.4.second_controller_acceptance: PR #47 carries merged_sha None here but {merged_pr47['merged_sha']!r} in {pr47_id}",
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

    def test_unverifiable_merged_sha_claim_is_named_in_needs_verification(self):
        """A fabricated merged_sha that collides with nothing in the manifest cannot be refuted without git
        ancestry (no subprocess allowed); the checker must at least name the claim as unverified."""
        manifest = _valid_manifest_dict()
        record = manifest["e_scopes"]["E16.4"]["verified_subscopes"][0]
        record.pop("pr_state")
        record["merged_sha"] = hashlib.sha1(b"fabricated merge of pr47").hexdigest()
        report = self._assert_valid(manifest)
        self.assertTrue(
            any(
                f"{record['id']}: merged_sha {record['merged_sha']} for PR #47 (pr_state unrecorded) is a manifest assertion" in item
                for item in report.needs_verification
            ),
            report.needs_verification,
        )

    def test_new_task_with_fresh_evidence_is_still_registrable(self):
        manifest = _valid_manifest_dict()
        e14 = manifest["e_scopes"]["E14"]
        e14["status"] = "implemented_not_verified"
        record = _fresh_record(
            manifest["e_scopes"]["E13"]["verified_subscopes"][0], "E14", "inheritance_controller_acceptance", "e14-fresh"
        )
        record.update(
            {
                "test_entry": "crates/evo-engine/tests/meta.rs",
                "command": "cargo test --locked --offline -p evo-engine --test meta",
                "pr_state": "open",
            }
        )
        e14["verified_subscopes"].append(record)
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
        merged_record = e10["verified_subscopes"][1]
        self.assertTrue(merged_record["pr"].endswith("/43") and merged_record["merged_sha"])
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
        self.assertTrue(any("for PR #43 (pr_state merged) is a manifest assertion" in item for item in report.needs_verification))

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
