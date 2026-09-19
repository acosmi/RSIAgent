#!/usr/bin/env python3
"""Self-tests for scripts/check_support_scope.py (CP-002).

Stdlib-only (unittest + tempfile). Every scenario builds its own minimal
on-disk fixture under tempfile.TemporaryDirectory(); nothing here reads or
writes any file in the real repository except the one dedicated test that
deliberately points the checker at this repo's own real manifest to capture
an honest, non-fabricated run against real data.
"""

from __future__ import annotations

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_support_scope as css  # noqa: E402


def _write(path: Path, content: str = "") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def _valid_manifest_dict() -> dict:
    """A minimal manifest satisfying every frozen CP-001 invariant."""
    stub_entry = {
        "title": "stub entry",
        "status": "verified_stub",
        "impl_files": ["crates/dummy/src/lib.rs"],
        "test_files": ["crates/dummy/tests/lib.rs"],
        "test_command": "cargo test --locked --offline -p dummy --test lib",
    }
    e_scopes = {key: dict(stub_entry) for key in css.REQUIRED_E_SCOPE_KEYS}
    # Give one entry (E14) "planned" status with no test evidence, mirroring
    # the real manifest's E14/E15 shape.
    e_scopes["E14"] = {
        "title": "stub planned entry",
        "status": "planned",
        "impl_files": ["crates/dummy/src/lib.rs"],
        "test_files": [],
        "test_command": "",
    }
    return {
        "plan_version": css.EXPECTED_PLAN_VERSION,
        "plan_sha256": css.EXPECTED_PLAN_SHA256,
        "declaration": css.EXPECTED_DECLARATION,
        "not_full_route_complete": True,
        "legacy_equivalence_unverified": True,
        "dimensions": {"effect": "not_claimed"},
        "traceability": {
            "b_commitments": [f"B{i:02d}" for i in range(1, 11)],
            "u_commitments": [f"U{i:02d}" for i in range(1, 9)],
            "k_commitments": [f"K{i:02d}" for i in range(1, 11)],
            "v_scenarios": [f"V{i:03d}" for i in range(1, 99)],
            "so_sources": [
                {
                    "id": f"SO{i:02d}",
                    "file": "skillopt/engine/trainer.py",
                    "commit": "7" * 40,
                    "blob_sha": "5" * 40,
                    "e_tasks": ["E03"],
                }
                for i in range(1, 19)
            ],
        },
        "e_scopes": e_scopes,
    }


class FixtureRepo:
    """A minimal on-disk repo satisfying _valid_manifest_dict()'s file refs."""

    def __init__(self, root: Path):
        self.root = root
        _write(self.root / "crates/dummy/src/lib.rs", "// dummy impl\n")
        _write(self.root / "crates/dummy/tests/lib.rs", "// dummy test\n")

    def write_manifest(self, manifest: dict, rel: str = "reports/support-scope.json") -> None:
        _write(self.root / rel, json.dumps(manifest, indent=2))


def _run_cli(args: list) -> tuple[int, dict]:
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        code = css.main(args)
    return code, json.loads(buf.getvalue())


class ValidManifestTests(unittest.TestCase):
    def test_valid_manifest_passes_structurally_and_exits_zero(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            repo.write_manifest(_valid_manifest_dict())
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertTrue(report.structure_valid, report.errors)

            code, payload = _run_cli(
                [
                    "--repo-root",
                    str(repo.root),
                    "--manifest",
                    "reports/support-scope.json",
                ]
            )
            self.assertEqual(code, 0)
            self.assertTrue(payload["structure_valid"])
            self.assertIn("disclaimer", payload)

    def test_missing_source_of_truth_is_reported_as_unverified_not_silently_ok(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            repo.write_manifest(_valid_manifest_dict())
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertTrue(report.structure_valid)
            self.assertFalse(report.input_binding_available)
            self.assertTrue(
                any("no --source-of-truth supplied" in n for n in report.needs_verification)
            )

    def test_matching_source_of_truth_hash_marks_input_binding_available(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            sot_path = Path(tmp) / "plan.md"
            sot_path.write_text("frozen plan content for this fixture\n", encoding="utf-8")
            fake_hash = css.sha256_of_file(sot_path)

            manifest = _valid_manifest_dict()
            manifest["plan_sha256"] = fake_hash
            repo.write_manifest(manifest)

            with mock.patch.object(css, "EXPECTED_PLAN_SHA256", fake_hash):
                report = css.run_check(repo.root, "reports/support-scope.json", sot_path)
            self.assertTrue(report.structure_valid, report.errors)
            self.assertTrue(report.input_binding_available)

    def test_mismatched_source_of_truth_hash_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            repo.write_manifest(_valid_manifest_dict())
            sot_path = Path(tmp) / "plan.md"
            sot_path.write_text("this does not match the pinned hash\n", encoding="utf-8")

            report = css.run_check(repo.root, "reports/support-scope.json", sot_path)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("source-of-truth SHA-256" in e for e in report.errors))


class RejectionTests(unittest.TestCase):
    """The four mandated negative categories, plus path-safety hardening."""

    def _manifest_with(self, mutate) -> dict:
        manifest = _valid_manifest_dict()
        mutate(manifest)
        return manifest

    def test_rejects_impl_file_that_does_not_exist_on_disk(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))

            def mutate(m):
                m["e_scopes"]["E00"]["impl_files"] = ["crates/dummy/src/never_written.rs"]

            repo.write_manifest(self._manifest_with(mutate))
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("does not exist on disk" in e for e in report.errors))

    def test_rejects_test_command_pointing_at_a_fake_target(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))

            def mutate(m):
                m["e_scopes"]["E00"]["test_files"] = ["crates/dummy/tests/never_written.rs"]
                m["e_scopes"]["E00"]["test_command"] = (
                    "cargo test --locked --offline -p dummy --test never_written"
                )

            repo.write_manifest(self._manifest_with(mutate))
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("does not exist on disk" in e for e in report.errors))

    def test_rejects_test_command_target_not_listed_in_test_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            _write(Path(tmp) / "crates/dummy/tests/other.rs", "// other\n")

            def mutate(m):
                # test_files still lists lib.rs, but the command targets a
                # different (real, existing) file that is not declared.
                m["e_scopes"]["E00"]["test_command"] = (
                    "cargo test --locked --offline -p dummy --test other"
                )

            repo.write_manifest(self._manifest_with(mutate))
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("is not one of test_files" in e for e in report.errors))

    def test_rejects_duplicate_id_in_a_commitment_set(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))

            def mutate(m):
                # B01 repeated, B10 absent -- array length still 10.
                b = m["traceability"]["b_commitments"]
                b[-1] = b[0]

            repo.write_manifest(self._manifest_with(mutate))
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("duplicate id B01" in e for e in report.errors))

    def test_rejects_id_set_with_correct_length_but_wrong_members(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))

            def mutate(m):
                # B10 replaced by an out-of-range B11: length unchanged, no
                # duplicates, but the set is not the required B01..B10.
                b = m["traceability"]["b_commitments"]
                b[-1] = "B11"

            repo.write_manifest(self._manifest_with(mutate))
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(
                any("missing=" in e and "extra=" in e for e in report.errors)
            )

    def test_rejects_so_source_with_illegal_hash_format(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))

            def mutate(m):
                m["traceability"]["so_sources"][0]["commit"] = "not-a-real-sha"

            repo.write_manifest(self._manifest_with(mutate))
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("40-char hex" in e for e in report.errors))

    def test_rejects_verified_status_with_no_test_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))

            def mutate(m):
                m["e_scopes"]["E00"]["test_files"] = []
                m["e_scopes"]["E00"]["test_command"] = ""

            repo.write_manifest(self._manifest_with(mutate))
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(
                any("requires non-empty test_files" in e for e in report.errors)
            )

    def test_planned_status_tolerates_empty_test_evidence(self):
        # E14 in the base fixture is already "planned" with no test
        # evidence; confirm this alone does not fail the manifest.
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            repo.write_manifest(_valid_manifest_dict())
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertTrue(report.structure_valid, report.errors)


class PathSafetyTests(unittest.TestCase):
    def test_rejects_dot_dot_path_escape(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))

            def mutate(m):
                m["e_scopes"]["E00"]["impl_files"] = ["../../../../etc/passwd"]

            repo.write_manifest(self._manifest_with_escape(mutate))
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(
                any("escapes repo root" in e for e in report.errors)
            )

    def _manifest_with_escape(self, mutate):
        manifest = _valid_manifest_dict()
        mutate(manifest)
        return manifest

    def test_rejects_absolute_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E00"]["impl_files"] = ["/etc/passwd"]
            repo.write_manifest(manifest)
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("absolute path not allowed" in e for e in report.errors))

    def test_rejects_symlink_component(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            real_target = Path(tmp) / "outside_repo_secret.rs"
            real_target.write_text("// outside\n", encoding="utf-8")
            link_path = repo.root / "crates/dummy/src/linked.rs"
            link_path.parent.mkdir(parents=True, exist_ok=True)
            try:
                link_path.symlink_to(real_target)
            except (OSError, NotImplementedError) as exc:  # pragma: no cover
                self.skipTest(f"symlinks not supported in this environment: {exc}")

            manifest = _valid_manifest_dict()
            manifest["e_scopes"]["E00"]["impl_files"] = ["crates/dummy/src/linked.rs"]
            repo.write_manifest(manifest)
            report = css.run_check(repo.root, "reports/support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("symlink component" in e for e in report.errors))

    def test_rejects_manifest_path_itself_escaping_repo_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = FixtureRepo(Path(tmp))
            repo.write_manifest(_valid_manifest_dict())
            report = css.run_check(repo.root, "../support-scope.json", None)
            self.assertFalse(report.structure_valid)
            self.assertTrue(any("escapes repo root" in e for e in report.errors))


class RealRepositoryTests(unittest.TestCase):
    """Runs the checker against this actual working tree's real manifest and
    captures an honest, non-fabricated result (not a synthetic fixture)."""

    def test_real_manifest_report(self):
        repo_root = Path(__file__).resolve().parents[1]
        code, payload = _run_cli(
            [
                "--repo-root",
                str(repo_root),
                "--manifest",
                "reports/support-scope.json",
            ]
        )
        # Record exactly what was observed; do not assume a result.
        print(
            f"[test_real_manifest_report] exit={code} "
            f"structure_valid={payload.get('structure_valid')} "
            f"errors={payload.get('errors')}",
            file=sys.stderr,
        )
        self.assertIsInstance(payload, dict)
        self.assertIn("structure_valid", payload)
        self.assertEqual(code, 0 if payload["structure_valid"] else 1)


if __name__ == "__main__":
    unittest.main()
