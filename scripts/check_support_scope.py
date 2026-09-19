#!/usr/bin/env python3
"""Read-only structural checker for reports/support-scope.json (CP-002, E16.6).

Scope and guarantees
---------------------
This tool performs a *structural* check of a support-scope manifest:

- every ``e_scopes`` entry's ``impl_files``/``test_files`` resolve to real,
  on-disk regular files inside the given repo root (no absolute paths, no
  ``..`` escapes, no symlink components);
- every non-``planned`` entry's ``test_command`` is parsed (never executed)
  into the concrete file it targets, which must both exist on disk and be
  declared in that entry's ``test_files``;
- the frozen CP-001 traceability id sets (B01-B10, U01-U08, K01-K10,
  SO01-SO18, V001-V098) are present as exact, duplicate-free sets, not just
  arrays of the right length;
- ``so_sources`` provenance fields (``commit``, ``blob_sha``) are well-formed
  40-character hex strings and every entry maps to at least one E task;
- the manifest's frozen ``plan_version``/``plan_sha256``/``declaration``/
  ``not_full_route_complete``/``legacy_equivalence_unverified`` fields match
  the CP-001 baseline;
- status vs. evidence compatibility: any status other than ``"planned"``
  must carry non-empty ``test_files`` and ``test_command``.

What this tool deliberately does NOT do
----------------------------------------
- It never executes any ``test_command`` (or any other command) found in the
  manifest: commands are tokenized and pattern-matched only, never passed to
  a shell or ``exec``-family call.
- It never performs network access.
- It never writes to the repository; it only reads files and prints a JSON
  report to stdout.
- A clean run (``exit 0`` / ``structure_valid: true``) means only that the
  manifest is internally consistent and every referenced path is real. It is
  NOT a claim that any test actually passed, that a human/controller has
  accepted the work, that a PR may be merged, that upstream licenses were
  reviewed, or that anything is production-ready. See ``needs_verification``
  in the report for what this run could not itself confirm.

``--source-of-truth`` is an optional *local*, out-of-repo reference document
(the frozen v4.1 plan text). It is never required to live inside the repo
root -- by design it is a local-only file the operator points at -- but when
supplied its SHA-256 is computed and compared against the manifest's
``plan_sha256`` before anything else, so a stale/incorrect reference cannot
silently pass.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

# CP-001 froze these values for the current cycle; a legitimate future
# re-pin requires an explicit reviewed follow-up task that updates both the
# manifest and these constants together, not a silent drift.
EXPECTED_PLAN_VERSION = "v4.1"
EXPECTED_PLAN_SHA256 = (
    "45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150"
)
EXPECTED_DECLARATION = "subset_only"

# Frozen traceability id-set shape: (json field, prefix, count, zero-pad width).
ID_SET_SPECS = [
    ("b_commitments", "B", 10, 2),
    ("u_commitments", "U", 8, 2),
    ("k_commitments", "K", 10, 2),
    ("v_scenarios", "V", 98, 3),
]
SO_PREFIX_WIDTH = 2
SO_COUNT = 18

# E-scopes that must be present in any manifest claiming CP-001 lineage.
# Extra keys (e.g. a future E17/E18) are tolerated; this is a floor, not an
# exact-equality check, so the checker does not need updating for every
# legitimate future addition.
REQUIRED_E_SCOPE_KEYS = [f"E{i:02d}" for i in range(0, 16)] + [
    "E16.1",
    "E16.2",
    "E16.3",
    "E16.4",
    "E16.5",
    "E16.6",
]

_HEX40 = re.compile(r"^[0-9a-fA-F]{40}$")


class CheckerError(Exception):
    """A structural violation found in the manifest or a path it declares."""


# ---------------------------------------------------------------------------
# Path safety: every path *declared inside the manifest* is untrusted input
# and must be confined to repo_root, non-absolute, escape-free, and free of
# symlink components before it is ever touched on disk.
# ---------------------------------------------------------------------------


def ensure_safe_relative_path(rel: Any, *, what: str) -> str:
    if not isinstance(rel, str) or not rel:
        raise CheckerError(f"{what}: path must be a non-empty string, got {rel!r}")
    if rel.startswith("/") or rel.startswith("\\"):
        raise CheckerError(f"{what}: absolute path not allowed: {rel!r}")
    if re.match(r"^[A-Za-z]:", rel):
        raise CheckerError(f"{what}: absolute path not allowed: {rel!r}")
    if "\\" in rel:
        raise CheckerError(f"{what}: path must use forward slashes: {rel!r}")
    for part in rel.split("/"):
        if part in ("", ".", ".."):
            raise CheckerError(
                f"{what}: path escapes repo root or has an illegal segment: {rel!r}"
            )
    return rel


def resolve_regular_file_in_repo(repo_root: Path, rel: Any, *, what: str) -> Path:
    """Resolve `rel` under `repo_root`, rejecting escapes, symlinked path
    components, and anything that is not a plain regular file."""
    ensure_safe_relative_path(rel, what=what)
    current = repo_root
    for part in rel.split("/"):
        current = current / part
        if current.is_symlink():
            raise CheckerError(f"{what}: path contains a symlink component: {rel!r}")
    if not current.exists():
        raise CheckerError(f"{what}: path does not exist on disk: {rel!r}")
    if not current.is_file():
        raise CheckerError(f"{what}: path is not a regular file: {rel!r}")
    return current


def sha256_of_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


# ---------------------------------------------------------------------------
# Traceability id-set validation (exact sets, not just counts)
# ---------------------------------------------------------------------------


def validate_id_set(field_name: str, values: Any, prefix: str, count: int, width: int) -> None:
    if not isinstance(values, list):
        raise CheckerError(f"{field_name}: must be an array")
    seen: set[str] = set()
    for item in values:
        if not isinstance(item, str):
            raise CheckerError(f"{field_name}: entry {item!r} is not a string")
        if item in seen:
            raise CheckerError(f"{field_name}: duplicate id {item}")
        seen.add(item)
    expected = {f"{prefix}{i:0{width}d}" for i in range(1, count + 1)}
    if seen != expected:
        missing = sorted(expected - seen)
        extra = sorted(seen - expected)
        raise CheckerError(
            f"{field_name}: id set mismatch, missing={missing} extra={extra}"
        )


def validate_so_sources(so_sources: Any) -> None:
    if not isinstance(so_sources, list):
        raise CheckerError("so_sources: must be an array")
    seen: set[str] = set()
    for so in so_sources:
        if not isinstance(so, dict):
            raise CheckerError("so_sources: entry is not an object")
        sid = so.get("id")
        if not isinstance(sid, str) or not sid:
            raise CheckerError("so_sources: entry missing string id")
        if sid in seen:
            raise CheckerError(f"so_sources: duplicate id {sid}")
        seen.add(sid)

        commit = so.get("commit")
        if not isinstance(commit, str) or not _HEX40.match(commit):
            raise CheckerError(
                f"{sid}: commit must be a 40-char hex sha, got {commit!r}"
            )
        blob_sha = so.get("blob_sha")
        if not isinstance(blob_sha, str) or not _HEX40.match(blob_sha):
            raise CheckerError(
                f"{sid}: blob_sha must be a 40-char hex sha, got {blob_sha!r}"
            )
        e_tasks = so.get("e_tasks")
        if not isinstance(e_tasks, list) or not e_tasks:
            raise CheckerError(f"{sid}: e_tasks must be a non-empty array")

    expected = {f"SO{i:0{SO_PREFIX_WIDTH}d}" for i in range(1, SO_COUNT + 1)}
    if seen != expected:
        missing = sorted(expected - seen)
        extra = sorted(seen - expected)
        raise CheckerError(
            f"so_sources: id set mismatch, missing={missing} extra={extra}"
        )


# ---------------------------------------------------------------------------
# test_command parsing (structural only -- never executed)
# ---------------------------------------------------------------------------


def resolve_test_command_target(cmd: Any) -> str:
    """Parse ``cargo test ... -p <crate> (--test <name> | --lib <module>::tests)``
    and return the repo-relative file it must exercise, without running it."""
    if not isinstance(cmd, str) or not cmd.strip().startswith("cargo test"):
        raise CheckerError(f"test_command must start with 'cargo test', got {cmd!r}")
    tokens = cmd.split()

    crate_name: Optional[str] = None
    for i, tok in enumerate(tokens):
        if tok == "-p" and i + 1 < len(tokens):
            crate_name = tokens[i + 1]
            break
    if crate_name is None:
        raise CheckerError(f"test_command missing -p <crate>: {cmd!r}")

    for i, tok in enumerate(tokens):
        if tok == "--test" and i + 1 < len(tokens):
            return f"crates/{crate_name}/tests/{tokens[i + 1]}.rs"

    for i, tok in enumerate(tokens):
        if tok == "--lib" and i + 1 < len(tokens):
            filt = tokens[i + 1]
            if not filt.endswith("::tests"):
                raise CheckerError(
                    f"--lib filter must target a `::tests` module, got {filt!r}"
                )
            module = filt[: -len("::tests")]
            module_path = module.replace("::", "/")
            return f"crates/{crate_name}/src/{module_path}.rs"

    raise CheckerError(
        f"test_command must use --test <name> or --lib <module>::tests: {cmd!r}"
    )


# ---------------------------------------------------------------------------
# Per-e_scope entry validation
# ---------------------------------------------------------------------------


def validate_e_scope_entry(name: str, entry: Any, repo_root: Path) -> None:
    if not isinstance(entry, dict):
        raise CheckerError(f"{name}: entry is not an object")

    status = entry.get("status")
    if not isinstance(status, str) or not status:
        raise CheckerError(f"{name}: missing status")

    impl_files = entry.get("impl_files")
    if not isinstance(impl_files, list) or not impl_files:
        raise CheckerError(f"{name}: impl_files must be a non-empty array")
    for f in impl_files:
        resolve_regular_file_in_repo(repo_root, f, what=f"{name}: impl_files")

    test_files = entry.get("test_files")
    if not isinstance(test_files, list):
        raise CheckerError(f"{name}: test_files must be an array")
    test_command = entry.get("test_command") or ""

    if status == "planned":
        # Planned work may legitimately have no test evidence yet, but any
        # test_files it does list must still be real.
        for f in test_files:
            resolve_regular_file_in_repo(repo_root, f, what=f"{name}: test_files")
        return

    if not test_files:
        raise CheckerError(f"{name}: status {status!r} requires non-empty test_files")
    if not test_command:
        raise CheckerError(
            f"{name}: status {status!r} requires a non-empty test_command"
        )

    for f in test_files:
        resolve_regular_file_in_repo(repo_root, f, what=f"{name}: test_files")

    target = resolve_test_command_target(test_command)
    if target not in test_files:
        raise CheckerError(
            f"{name}: test_command target {target!r} is not one of "
            f"test_files {test_files!r}"
        )
    resolve_regular_file_in_repo(repo_root, target, what=f"{name}: test_command target")


# ---------------------------------------------------------------------------
# Top-level report
# ---------------------------------------------------------------------------


@dataclass
class CheckReport:
    structure_valid: bool = True
    input_binding_available: bool = False
    needs_verification: list = field(default_factory=list)
    errors: list = field(default_factory=list)
    plan_version: Optional[str] = None
    plan_sha256: Optional[str] = None

    def fail(self, message: str) -> None:
        self.structure_valid = False
        self.errors.append(message)

    def to_dict(self) -> dict:
        return {
            "structure_valid": self.structure_valid,
            "input_binding_available": self.input_binding_available,
            "needs_verification": self.needs_verification,
            "errors": self.errors,
            "plan_version": self.plan_version,
            "plan_sha256": self.plan_sha256,
            "disclaimer": (
                "structure_valid=true / exit 0 means only that the manifest's "
                "declared paths, ids, and test_command targets are internally "
                "consistent and present on disk. This run did not execute any "
                "test, does not constitute controller acceptance or approval, "
                "does not authorize a merge, and does not assert license "
                "re-review or production readiness."
            ),
        }


def run_check(
    repo_root: Path, manifest_rel: str, source_of_truth: Optional[Path]
) -> CheckReport:
    report = CheckReport()

    if not repo_root.is_dir():
        report.fail(f"repo root does not exist or is not a directory: {repo_root}")
        return report

    try:
        manifest_path = resolve_regular_file_in_repo(
            repo_root, manifest_rel, what="manifest"
        )
    except CheckerError as exc:
        report.fail(str(exc))
        return report

    try:
        raw = manifest_path.read_text(encoding="utf-8")
    except OSError as exc:
        report.fail(f"manifest could not be read: {exc}")
        return report

    try:
        manifest = json.loads(raw)
    except json.JSONDecodeError as exc:
        report.fail(f"manifest is not valid JSON: {exc}")
        return report

    if not isinstance(manifest, dict):
        report.fail("manifest root must be a JSON object")
        return report

    report.plan_version = manifest.get("plan_version")
    report.plan_sha256 = manifest.get("plan_sha256")

    if report.plan_version != EXPECTED_PLAN_VERSION:
        report.fail(
            f"plan_version must be {EXPECTED_PLAN_VERSION!r}, got {report.plan_version!r}"
        )
    if report.plan_sha256 != EXPECTED_PLAN_SHA256:
        report.fail(
            f"plan_sha256 must be {EXPECTED_PLAN_SHA256!r}, got {report.plan_sha256!r}"
        )
    if manifest.get("declaration") != EXPECTED_DECLARATION:
        report.fail(
            f"declaration must be {EXPECTED_DECLARATION!r}, "
            f"got {manifest.get('declaration')!r}"
        )
    if manifest.get("not_full_route_complete") is not True:
        report.fail("not_full_route_complete must be true")
    if manifest.get("legacy_equivalence_unverified") is not True:
        report.fail("legacy_equivalence_unverified must be true")

    if source_of_truth is not None:
        if not source_of_truth.is_file() or source_of_truth.is_symlink():
            report.fail(
                f"source-of-truth path is not a regular file: {source_of_truth}"
            )
        else:
            actual = sha256_of_file(source_of_truth)
            if report.plan_sha256 and actual == report.plan_sha256:
                report.input_binding_available = True
            else:
                report.fail(
                    "source-of-truth SHA-256 does not match manifest plan_sha256: "
                    f"computed={actual!r} manifest={report.plan_sha256!r}"
                )
    else:
        report.needs_verification.append(
            "no --source-of-truth supplied: plan_sha256 was not cross-checked "
            "against an actual local document in this run"
        )

    traceability = manifest.get("traceability")
    if not isinstance(traceability, dict):
        report.fail("traceability must be an object")
        traceability = {}

    for field_name, prefix, count, width in ID_SET_SPECS:
        try:
            validate_id_set(field_name, traceability.get(field_name), prefix, count, width)
        except CheckerError as exc:
            report.fail(str(exc))

    try:
        validate_so_sources(traceability.get("so_sources"))
    except CheckerError as exc:
        report.fail(str(exc))

    e_scopes = manifest.get("e_scopes")
    if not isinstance(e_scopes, dict):
        report.fail("e_scopes must be an object")
        e_scopes = {}

    for key in REQUIRED_E_SCOPE_KEYS:
        if key not in e_scopes:
            report.fail(f"e_scopes missing required key {key}")

    for name, entry in e_scopes.items():
        try:
            validate_e_scope_entry(name, entry, repo_root)
        except CheckerError as exc:
            report.fail(f"e_scopes[{name}]: {exc}")

    report.needs_verification.append(
        "test_command targets were parsed and confirmed to exist; none were "
        "executed by this checker, so pass/fail of the underlying test suite "
        "is not itself confirmed by this run"
    )
    report.needs_verification.append(
        "so_sources commit/blob_sha were format-validated only; upstream "
        "reachability, content, and license status were not re-verified"
    )

    return report


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Read-only structural checker for reports/support-scope.json. "
            "Exit 0 means the manifest is internally consistent; it is not "
            "an acceptance, approval, or merge decision."
        )
    )
    parser.add_argument(
        "--repo-root",
        required=True,
        type=Path,
        help="Path to the repository root that the manifest's paths are relative to.",
    )
    parser.add_argument(
        "--manifest",
        required=True,
        help="Manifest path relative to --repo-root (e.g. reports/support-scope.json).",
    )
    parser.add_argument(
        "--source-of-truth",
        type=Path,
        default=None,
        help=(
            "Optional path to a local copy of the frozen plan document. Not "
            "required to live inside --repo-root. Its SHA-256 is compared "
            "against the manifest's plan_sha256."
        ),
    )
    return parser


def main(argv: Optional[list] = None) -> int:
    args = build_arg_parser().parse_args(argv)
    repo_root = args.repo_root.resolve()
    report = run_check(repo_root, args.manifest, args.source_of_truth)
    print(json.dumps(report.to_dict(), indent=2, sort_keys=True))
    return 0 if report.structure_valid else 1


if __name__ == "__main__":
    sys.exit(main())
