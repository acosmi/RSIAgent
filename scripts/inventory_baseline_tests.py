#!/usr/bin/env python3
"""Enumerate and bind actual Rust baseline tests without inventing T001–T126."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEST_RE = re.compile(
    r"#\[(?P<attribute>(?:tokio::)?test)(?:\s*\([^\]]*\))?\]\s*"
    r"(?:#\[[^\]]+\]\s*)*"
    r"(?P<async>async\s+)?fn\s+(?P<name>[A-Za-z0-9_]+)\s*\([^)]*\)"
    r"(?:\s*->\s*[^\{]+)?\s*\{",
    re.MULTILINE,
)
CARGO_RUNNING_RE = re.compile(r"^\s*Running .+?/deps/(?P<target>[^/\s]+?)-[0-9a-f]+\)?\s*$")
CARGO_TEST_RE = re.compile(
    r"^test (?P<runner>\S+) \.\.\. (?P<status>ok|FAILED|ignored)(?:,.*)?$"
)
ASSERTION_RE = re.compile(r"\b(?:assert|assert_eq|assert_ne|matches|panic)!\s*\(")
HEX40_RE = re.compile(r"^[0-9a-fA-F]{40}$")
HEX64_RE = re.compile(r"^[0-9a-fA-F]{64}$")
EXCLUDED_TOP_LEVEL = {".git", "archive", "out", "target"}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_blob_sha1(data: bytes) -> str:
    header = f"blob {len(data)}\0".encode("ascii")
    return hashlib.sha1(header + data).hexdigest()


def balanced_segment(text: str, opening: int, left: str, right: str) -> str:
    depth = 0
    for index in range(opening, len(text)):
        char = text[index]
        if char == left:
            depth += 1
        elif char == right:
            depth -= 1
            if depth == 0:
                return text[opening : index + 1]
    return text[opening:]


def source_evidence(body: str) -> dict[str, object]:
    assertions = []
    for match in ASSERTION_RE.finditer(body):
        opening = body.find("(", match.start())
        exact = balanced_segment(body, opening, "(", ")")
        macro = body[match.start() : opening].strip()
        assertions.append(
            {
                "macro": macro,
                "source_excerpt": exact[:500],
                "source_sha256": hashlib.sha256(exact.encode("utf-8")).hexdigest(),
                "truncated": len(exact) > 500,
            }
        )
    before_first_assertion = body[: ASSERTION_RE.search(body).start()] if ASSERTION_RE.search(body) else body
    fixture_lines = []
    for line in before_first_assertion.splitlines():
        stripped = line.strip()
        if stripped.startswith("let ") or stripped.startswith("let mut "):
            fixture_lines.append(stripped[:500])
    return {
        "lexical_test_region_sha256": hashlib.sha256(body.encode("utf-8")).hexdigest(),
        "lexical_region_review_required": True,
        "lexical_region_note": (
            "brace-delimited lexical excerpt; strings/comments are not a Rust syntax proof"
        ),
        "fixture_source_excerpt": fixture_lines[:12],
        "fixture_excerpt_truncated": len(fixture_lines) > 12,
        "fixture_review_required": True,
        "assertions": assertions,
        "validation_semantics_review_required": True,
        "automatic_v_mapping": None,
    }


def git_source_blobs(source_sha: str) -> tuple[bool, dict[str, str]]:
    verify = subprocess.run(
        ["git", "-C", str(ROOT), "cat-file", "-e", f"{source_sha}^{{commit}}"],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if verify.returncode != 0:
        return False, {}
    tree = subprocess.run(
        ["git", "-C", str(ROOT), "ls-tree", "--full-tree", "-r", "-z", source_sha],
        check=False,
        capture_output=True,
    )
    if tree.returncode != 0:
        return False, {}
    blobs = {}
    for entry in tree.stdout.split(b"\0"):
        if not entry:
            continue
        metadata, path = entry.split(b"\t", 1)
        _mode, kind, blob_sha = metadata.decode("ascii").split()
        if kind == "blob":
            blobs[path.decode("utf-8")] = blob_sha
    return True, blobs


def is_code_input(path: str) -> bool:
    parts = Path(path).parts
    name = parts[-1] if parts else ""
    return (
        name in {"Cargo.lock", "Cargo.toml", "rust-toolchain.toml", "build.rs"}
        or Path(path).suffix in {".rs", ".sql"}
        or "fixtures" in parts
        or "tests" in parts
    )


def current_code_input_blobs() -> dict[str, str]:
    blobs = {}
    for path in ROOT.rglob("*"):
        if not path.is_file():
            continue
        rel = path.relative_to(ROOT).as_posix()
        if rel.split("/", 1)[0] in EXCLUDED_TOP_LEVEL or not is_code_input(rel):
            continue
        blobs[rel] = git_blob_sha1(path.read_bytes())
    return blobs


def package_target(path: Path) -> str | None:
    current = path.parent
    while current != ROOT.parent:
        cargo_toml = current / "Cargo.toml"
        if cargo_toml.is_file():
            relative = path.relative_to(current)
            if relative.parts and relative.parts[0] == "tests":
                return path.stem.replace("-", "_")
            text = cargo_toml.read_text(encoding="utf-8")
            package = re.search(r"(?ms)^\[package\].*?^name\s*=\s*\"([^\"]+)\"", text)
            if package:
                return package.group(1).replace("-", "_")
            return None
        if current == ROOT:
            break
        current = current.parent
    return None


def cargo_results(path: Path) -> list[dict[str, str]]:
    results = []
    target = None
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        running = CARGO_RUNNING_RE.match(line)
        if running:
            target = running.group("target")
            continue
        test = CARGO_TEST_RE.match(line)
        if test and target is not None:
            status = {"ok": "passed", "FAILED": "failed", "ignored": "ignored"}[
                test.group("status")
            ]
            results.append(
                {
                    "target": target,
                    "runner_name": test.group("runner"),
                    "status": status,
                }
            )
    return results


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Enumerate Rust tests and bind them to an explicit plan, source, and cargo log."
    )
    parser.add_argument("--plan-version")
    parser.add_argument("--plan-sha256")
    parser.add_argument(
        "--ledger",
        type=Path,
        help="read one plan_version and plan SHA-256 binding from the public ledger",
    )
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--test-log", required=True, type=Path)
    args = parser.parse_args(argv)
    direct_plan = args.plan_version is not None or args.plan_sha256 is not None
    if args.ledger is not None and direct_plan:
        parser.error("--ledger cannot be combined with --plan-version or --plan-sha256")
    if args.ledger is None:
        if args.plan_version is None or args.plan_sha256 is None:
            parser.error(
                "provide --ledger, or provide both --plan-version and --plan-sha256"
            )
        args.plan_binding_mode = "explicit"
        args.ledger_sha256 = None
    else:
        if not args.ledger.is_file():
            parser.error(f"--ledger does not exist: {args.ledger}")
        ledger_text = args.ledger.read_text(encoding="utf-8")
        versions = re.findall(
            r"^plan_version[：:]\s*`?([A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*)`?(?=[`；;\s]|$)",
            ledger_text,
            re.MULTILINE,
        )
        hashes = re.findall(r"plan_sha256[：:]\s*`([0-9a-fA-F]{64})`", ledger_text)
        hashes.extend(
            re.findall(
                r"规范正文 SHA-256[：:]\s*`([0-9a-fA-F]{64})`",
                ledger_text,
            )
        )
        if len(versions) != 1 or len(hashes) != 1:
            parser.error(
                "--ledger must contain exactly one plan_version and one 64-hex "
                "规范正文 SHA-256 binding"
            )
        args.plan_version = versions[0]
        args.plan_sha256 = hashes[0]
        args.plan_binding_mode = "ledger"
        args.ledger_sha256 = sha256_file(args.ledger)
    if not HEX64_RE.fullmatch(args.plan_sha256):
        parser.error("resolved plan SHA-256 must be exactly 64 hexadecimal characters")
    if not HEX40_RE.fullmatch(args.source_sha):
        parser.error("--source-sha must be exactly 40 hexadecimal characters")
    if not args.plan_version.strip():
        parser.error("--plan-version must not be empty")
    if not args.test_log.is_file():
        parser.error(f"--test-log does not exist: {args.test_log}")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    source_commit_verified, pinned_blobs = git_source_blobs(args.source_sha)
    pinned_code_inputs = {
        path: blob for path, blob in pinned_blobs.items() if is_code_input(path)
    }
    current_code_inputs = current_code_input_blobs()
    added_code_inputs = sorted(set(current_code_inputs) - set(pinned_code_inputs))
    missing_code_inputs = sorted(set(pinned_code_inputs) - set(current_code_inputs))
    modified_code_inputs = sorted(
        path
        for path in set(current_code_inputs).intersection(pinned_code_inputs)
        if current_code_inputs[path] != pinned_code_inputs[path]
    )
    code_input_tree_matches_source = (
        source_commit_verified
        and not added_code_inputs
        and not missing_code_inputs
        and not modified_code_inputs
    )
    observed = cargo_results(args.test_log)
    by_target_and_name: dict[tuple[str, str], list[dict[str, str]]] = defaultdict(list)
    for result in observed:
        by_target_and_name[(result["target"], result["runner_name"].split("::")[-1])].append(
            result
        )
    tests = []
    scanned_source_bindings = []
    for path in sorted(ROOT.rglob("*.rs")):
        rel = path.relative_to(ROOT).as_posix()
        if rel.split("/", 1)[0] in EXCLUDED_TOP_LEVEL:
            continue
        text = path.read_text(encoding="utf-8")
        data = path.read_bytes()
        file_sha = sha256_file(path)
        blob_sha = git_blob_sha1(data)
        pinned_blob_sha = pinned_blobs.get(rel)
        source_blob_matches = source_commit_verified and pinned_blob_sha == blob_sha
        scanned_source_bindings.append(source_blob_matches)
        target = package_target(path)
        for match in TEST_RE.finditer(text):
            name = match.group("name")
            line = text[: match.start()].count("\n") + 1
            function_line = text[: match.start("name")].count("\n") + 1
            opening = text.find("{", match.start(), match.end())
            body = balanced_segment(text, opening, "{", "}") if opening >= 0 else ""
            matches = by_target_and_name.get((target or "", name), [])
            if not code_input_tree_matches_source:
                initial_result = {
                    "status": "unverified",
                    "result_kind": "unverified",
                    "cargo_target": target,
                    "cargo_runner_name": matches[0]["runner_name"]
                    if len(matches) == 1
                    else None,
                    "matched_from_test_log": len(matches) == 1,
                    "review_required": True,
                    "reason": (
                        "the complete Rust/build/lock/migration/fixture input tree differs from "
                        "--source-sha; old log results cannot be inherited"
                    ),
                }
            elif len(matches) == 1:
                initial_result = {
                    "status": matches[0]["status"],
                    "result_kind": "log_observation_not_attested",
                    "cargo_target": matches[0]["target"],
                    "cargo_runner_name": matches[0]["runner_name"],
                    "matched_from_test_log": True,
                    "review_required": True,
                    "reason": (
                        "cargo output matched this pinned source test name, but the log does not "
                        "contain a cryptographic source attestation"
                    ),
                }
            else:
                initial_result = {
                    "status": "unverified",
                    "result_kind": "unverified",
                    "cargo_target": target,
                    "cargo_runner_name": None,
                    "matched_from_test_log": False,
                    "review_required": True,
                    "reason": "no unique cargo-log match"
                    if not matches
                    else "ambiguous cargo-log matches",
                }
            tests.append(
                {
                    "path": rel,
                    "name": name,
                    "line": line,
                    "function_line": function_line,
                    "test_attribute": match.group("attribute"),
                    "async": bool(match.group("async")),
                    "source_blob_sha1": blob_sha,
                    "pinned_source_blob_sha1": pinned_blob_sha,
                    "source_blob_matches_source_sha": source_blob_matches,
                    "source_sha256": file_sha,
                    "t_number": None,
                    "legacy_equivalence": "unverified",
                    "source_evidence": source_evidence(body),
                    "initial_result": initial_result,
                    "note": (
                        "enumerated from source; T001-T126 assertions were not provided, "
                        "so historical equivalence and V mapping require review"
                    ),
                }
            )
    matched = sum(
        test["initial_result"]["status"] != "unverified" for test in tests
    )
    statuses: dict[str, int] = defaultdict(int)
    for test in tests:
        statuses[test["initial_result"]["status"]] += 1
    scanned_tree_matches_source = bool(scanned_source_bindings) and all(scanned_source_bindings)
    payload = {
        "schema": "rsia.baseline_test_inventory.v2",
        "plan_version": args.plan_version,
        "plan_sha256": args.plan_sha256.lower(),
        "plan_binding_mode": args.plan_binding_mode,
        "ledger": str(args.ledger.resolve()) if args.ledger is not None else None,
        "ledger_sha256": args.ledger_sha256,
        "source_sha": args.source_sha.lower(),
        "source_commit_object_verified": source_commit_verified,
        "scanned_tree_matches_source": scanned_tree_matches_source,
        "code_input_tree_matches_source": code_input_tree_matches_source,
        "source_binding_verified": code_input_tree_matches_source,
        "source_binding_note": (
            "test-log results are associated only when the complete Rust/build/lock/migration/"
            "fixture input set exactly matches source_sha"
        ),
        "added_code_inputs": added_code_inputs,
        "missing_code_inputs": missing_code_inputs,
        "modified_code_inputs": modified_code_inputs,
        "test_log": str(args.test_log.resolve()),
        "test_log_sha256": sha256_file(args.test_log),
        "test_log_source_attested": False,
        "test_log_evidence_note": (
            "cargo output is a log observation only; it contains no cryptographic source binding"
        ),
        "count": len(tests),
        "matched_initial_results": matched,
        "initial_result_counts": dict(sorted(statuses.items())),
        "cargo_log_observations": len(observed),
        "raw_log_observations": observed,
        "tests": tests,
        "historical_counts_not_reused": {
            "rust": 90,
            "python": 15,
            "old_smoke": 12,
            "new_smoke": 8,
        },
        "legacy_equivalence_unverified": True,
    }
    json.dump(payload, sys.stdout, ensure_ascii=False, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
