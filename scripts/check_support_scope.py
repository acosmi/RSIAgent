#!/usr/bin/env python3
"""Read-only validator for the frozen rsia.support_scope.v2 derivative.

The checker never executes manifest commands, starts subprocesses, accesses the
network, or writes repository files. Exit 0 means structure only; it is not an
acceptance, effect, license, host, deployment, or merge decision.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

PLAN_VERSION = "v4.1"
PLAN_SHA256 = "45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150"
SCHEMA_VERSION = "rsia.support_scope.v2"
STATUSES = {"planned", "in_progress", "blocked", "implemented_not_verified", "verified", "explicitly_out_of_scope"}
SKILLOPT_REPO = "https://github.com/microsoft/SkillOpt.git"
SKILLOPT_COMMIT = "79124b37e9a6371e13b753f8bcd7adb1e493ade1"
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
PR_RE = re.compile(r"^https://github\.com/acosmi/RSIAgent/pull/(\d+)$")
IDENT_RE = re.compile(r"^[A-Za-z0-9_-]+$")

EXPECTED_E_SCENARIOS = {
    "E00": "V001 V071 V072 V073 V080 V098",
    "E01": "V010 V011 V012 V013 V084 V085 V096 V097",
    "E02": "V002 V003 V005 V009 V043 V044 V045 V046 V047 V048 V060 V062 V078 V079 V081 V082 V083 V084 V085 V086 V087 V088 V089 V090 V091 V092 V093 V094 V095 V096",
    "E03": "V004 V005 V006 V017 V051 V052 V054 V055 V056 V057 V058 V087 V088 V089 V090 V094 V096",
    "E04": "V007 V008 V009 V038 V083 V085 V086 V087 V093 V094 V096",
    "E05": "V010 V011 V012 V013 V028 V084 V085 V090 V091 V094 V097",
    "E06": "V014 V015 V016 V017 V047 V049 V050 V059 V064 V070 V075 V077 V078 V081 V084 V085 V089 V091 V094 V095 V096",
    "E07": "V003 V010 V014 V015 V016 V042 V047 V059 V070 V077 V087 V088 V089 V091 V096 V097",
    "E08": "V017 V018 V038 V056 V069 V075 V081 V082 V083 V084 V085 V086 V090 V092 V094 V096",
    "E09": "V019 V020 V022 V024 V025 V081 V086 V088 V089 V090 V091 V093 V094 V095 V096",
    "E10": "V020 V021 V022 V023 V025 V026 V027 V081 V086 V094 V096",
    "E11": "V022 V026 V027 V028 V042 V081 V094 V096 V097",
    "E12": "V029 V030 V031 V034 V082 V083 V093 V094 V096",
    "E13": "V016 V032 V037 V038 V082 V084 V087 V090 V091 V094 V096 V097",
    "E14": "V015 V033 V034 V035 V036 V086 V090 V092 V094 V096 V097",
    "E15": "V026 V033 V035 V036 V042 V085 V092 V096 V097",
    "E16": "V081 V082 V083 V084 V085 V086 V087 V088 V089 V090 V091 V092 V093 V094 V095 V096 V097 V098",
    "E16.1": "V005 V006 V017 V051 V052 V053 V054 V055 V056 V076 V087 V090 V098",
    "E16.2": "V017 V039 V066 V067 V068 V069 V070 V075 V091 V092 V098",
    "E16.3": "V014 V018 V038 V063 V064 V065 V078 V091 V092 V098",
    "E16.4": "V002 V003 V009 V037 V039 V060 V061 V062 V077 V087 V091 V098",
    "E16.5": "V007 V008 V017 V018 V037 V038 V039 V066 V069 V073 V075 V081 V082 V083 V084 V085 V086 V089 V090 V092 V094 V096 V098",
    "E16.6": "V001 V039 V042 V071 V072 V073 V074 V080 V098",
    "E17": "V040",
    "E18": "V041",
}
EXPECTED_E_SCENARIOS = {key: set(value.split()) for key, value in EXPECTED_E_SCENARIOS.items()}
EXPECTED_E_IDS = set(EXPECTED_E_SCENARIOS)
EXPECTED_COMMITMENTS = {
    "B01": ("§5.2", "RH01 RH02", "E02 E06", "V044 V048 V079"), "B02": ("§5.3", "RH03", "E02", "V043 V045 V046 V078"),
    "B03": ("§5.4 §6.5", "RH04", "E02 E06 E07", "V047 V049 V050 V059 V077"), "B04": ("§5.5", "RH05", "E02 E16.4", "V060 V061 V062"),
    "B05": ("§6.3", "RH06", "E03", "V051 V052 V055 V056"), "B06": ("§6.4", "RH07", "E03 E14", "V054 V057 V058 V070"),
    "B07": ("§5.6 §6.3", "RH08", "E03 E16.1", "V052 V053 V054 V056 V076"), "B08": ("§11.1", "RH09", "E06 E16.3", "V063 V064 V065 V078"),
    "B09": ("§6.5 §9", "RH01 RH07", "E06 E14 E15", "V015 V033 V036 V070"), "B10": ("§11.2–§11.3", "RH01", "E08 E16.2 E16.6", "V066 V067 V068 V069 V073 V075"),
    "U01": ("§7.1 §7.5", "", "E02 E09 E10 E11", "V025 V028 V081"), "U02": ("§7.5.1–§7.5.3", "", "E10 E11", "V021 V023 V027 V081"),
    "U03": ("§8.1", "", "E12 E13", "V029 V034 V082"), "U04": ("§8.2–§8.4", "", "E04 E12", "V007 V030 V031 V083"),
    "U05": ("§3.2 §10", "", "E01 E05 E06", "V010 V011 V084"), "U06": ("§3.4.1 §6.6", "", "E01 E04 E05", "V012 V013 V085"),
    "U07": ("§7.1.1 §7.2.1", "", "E09", "V020 V024 V086"), "U08": ("§1.4 §12 §14.2", "", "E00 E05 E09 E10 E12 E16.6", "V042 V062 V080 V081 V082 V083 V084 V085 V086"),
    "K01": ("§6.4.1 §5.8", "SO01 SO06", "E02 E03 E04 E07 E13", "V087"), "K02": ("§6.7.1–§6.7.2", "SO02 SO03", "E03 E09", "V088"),
    "K03": ("§5.3.1 §5.8", "SO04 SO05", "E02 E03 E06 E09", "V089"), "K04": ("§6.7.3 §5.8 §11.5", "SO01 SO02", "E03 E05 E08 E09 E13", "V090 V094"),
    "K05": ("§11.5 §10.2", "SO01 SO07 SO10", "E05 E06 E09 E13", "V091"), "K06": ("§9.1 §3.1.1", "SO01 SO08", "E14 E15", "V092 V097"),
    "K07": ("§8.5 §7.4.1", "SO12 SO13", "E04 E09 E10 E12", "V093 V094"), "K08": ("§7.7", "SO11 SO14", "E06 E09", "V095"),
    "K09": ("§3.1.1 §3.6 §6.7.4", "SO13 SO15 SO16", "E01 E04 E07 E11 E15", "V096 V097"), "K10": ("§2.7–§2.8 §16.5–§16.6 §18.7", "SO09 SO10 SO18", "E00 E05 E06 E16", "V091 V098"),
}
EXPECTED_SO = {
    "SO01": ("skillopt/engine/trainer.py", "520a90eb8af422368c8e498b8487504bb8f6c279", "1–240、1270–2110", "E03 E05 E09 E13 E14"),
    "SO02": ("skillopt/gradient/reflect.py", "df68c9b76de83b2465b267b04310399ec5f3cec8", "300–700", "E03 E09"),
    "SO03": ("skillopt/gradient/aggregate.py", "14f0cd978f35afe579913c9b5677a803bbce9e70", "全文", "E03 E09"),
    "SO04": ("skillopt/optimizer/clip.py", "a2ed965f7c81c18426e82e2fd49e589b0952a92f", "全文", "E02 E03 E09"),
    "SO05": ("skillopt/optimizer/skill.py", "65d574157f47e9b3f415d4db94b0e5a9c09e4826", "全文", "E02 E03 E06 E09"),
    "SO06": ("skillopt/optimizer/skill_aware.py", "de39427ed0cc85fa1e716363a22140c91090b2d6", "全文", "E03 E07 E13"),
    "SO07": ("skillopt/optimizer/slow_update.py", "9f8dd52606c3ada4fea1e6c0f72ec38f8a5139e8", "1–260", "E09 E13"),
    "SO08": ("skillopt/optimizer/meta_skill.py", "6e34ff10d90655bb620d3e63b1d2041338616a5c", "全文", "E14 E15"),
    "SO09": ("skillopt/evaluation/gate.py", "10461a98756ebacbf7d5a8bbdc67657950e2c199", "全文", "E05 E06"),
    "SO10": ("configs/_base_/default.yaml", "4fe64088dc8011a2121335155d4a79ae7f90e9f5", "全文；本轮重读90–150", "E00 E03 E05 E13"),
    "SO11": ("skillopt_sleep/consolidate.py", "5b80b4a9825fb5b5a79b04fdad4c655d5f3228f6", "全文", "E05 E06 E09"),
    "SO12": ("skillopt_sleep/rollout.py", "f889333b3eecb83693a7005154dedadccbdc07f2", "全文", "E09 E12"),
    "SO13": ("skillopt_sleep/replay.py", "1502a71cf9069b8ea23293acbb080225cdc51de2", "全文", "E04 E09 E10 E12"),
    "SO14": ("skillopt_sleep/multi_skill.py", "32247639849e2c327ab7a1b7b3258dc7c35f53d4", "全文", "E06 E09"),
    "SO15": ("skillopt_sleep/budget.py", "48875ca0f01cf099967ca2b5a45891e903645e36", "全文", "E01 E04 E11"),
    "SO16": ("docs/sleep/RESULTS.md", "441ffa28f8644631c275456c7ccf4e75fd1d8c99", "全文", "E01 E07 E15"),
    "SO17": ("docs/guide/training-loop.md", "30393dca9533de90e0cd0ff95f26eb327bfec0ff", "全文", "E03 E09"),
    "SO18": ("LICENSE", "cf7bcb2ba475e33648180599c0f83c425f1705d9", "全文；本轮重读及固定字节校验", "E00 E16.2 E16.5"),
}

class CheckerError(Exception):
    pass

def sha256_of_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()

def no_duplicate_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise CheckerError(f"duplicate JSON object key: {key}")
        result[key] = value
    return result

def load_json_strict(raw: str) -> Any:
    try:
        return json.loads(raw, object_pairs_hook=no_duplicate_object)
    except CheckerError:
        raise
    except (json.JSONDecodeError, TypeError) as exc:
        raise CheckerError(f"invalid JSON: {exc}") from exc

def safe_relative(value: Any, what: str) -> str:
    if not isinstance(value, str) or not value:
        raise CheckerError(f"{what}: path must be a non-empty string")
    if value.startswith(("/", "\\")) or re.match(r"^[A-Za-z]:", value) or "\\" in value:
        raise CheckerError(f"{what}: absolute/drive/backslash path is forbidden: {value!r}")
    if any(part in ("", ".", "..") for part in value.split("/")):
        raise CheckerError(f"{what}: path has traversal or non-normal segment: {value!r}")
    return value

def inspect_relative(repo_root: Path, value: Any, what: str, required: bool) -> Path | None:
    rel = safe_relative(value, what)
    current = repo_root
    for part in rel.split("/"):
        current = current / part
        if current.is_symlink():
            raise CheckerError(f"{what}: path contains a symlink component: {rel!r}")
        if not current.exists():
            if required:
                raise CheckerError(f"{what}: path does not exist: {rel!r}")
            return None
    if not current.is_file():
        raise CheckerError(f"{what}: path is not a regular file: {rel!r}")
    try:
        current.resolve(strict=True).relative_to(repo_root.resolve(strict=True))
    except (OSError, ValueError) as exc:
        raise CheckerError(f"{what}: path escapes repository: {rel!r}") from exc
    return current

def string_set(value: Any, what: str) -> set[str]:
    if not isinstance(value, list) or any(not isinstance(item, str) or not item for item in value):
        raise CheckerError(f"{what}: must be an array of non-empty strings")
    if len(value) != len(set(value)):
        raise CheckerError(f"{what}: duplicate id")
    return set(value)

def nonempty(value: Any, what: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise CheckerError(f"{what}: must be a non-empty string")
    return value

def workspace_packages(repo_root: Path) -> dict[str, tuple[Path, dict[str, Any]]]:
    root_manifest = inspect_relative(repo_root, "Cargo.toml", "workspace Cargo.toml", True)
    try:
        workspace = tomllib.loads(root_manifest.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise CheckerError(f"workspace Cargo.toml is invalid: {exc}") from exc
    members = workspace.get("workspace", {}).get("members")
    if not isinstance(members, list) or not members:
        raise CheckerError("workspace Cargo.toml has no explicit members")
    result = {}
    for member in members:
        rel = safe_relative(member, "workspace member")
        if any(char in rel for char in "*?["):
            raise CheckerError("workspace glob members are unsupported by this finite checker")
        cargo = inspect_relative(repo_root, f"{rel}/Cargo.toml", "member Cargo.toml", True)
        try:
            data = tomllib.loads(cargo.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as exc:
            raise CheckerError(f"invalid member Cargo.toml {rel}: {exc}") from exc
        name = data.get("package", {}).get("name")
        if not isinstance(name, str) or not IDENT_RE.fullmatch(name) or name in result:
            raise CheckerError(f"invalid or duplicate package name in {rel}")
        result[name] = (Path(rel), data)
    return result

def resolve_cargo_target(repo_root: Path, command: Any, packages: dict[str, tuple[Path, dict[str, Any]]]) -> str:
    if not isinstance(command, str):
        raise CheckerError("evidence command must be a string")
    tokens = command.split()
    if len(tokens) != 8 or tokens[:5] != ["cargo", "test", "--locked", "--offline", "-p"]:
        raise CheckerError("command is outside the finite single-target cargo test grammar")
    package, mode, target = tokens[5], tokens[6], tokens[7]
    if package not in packages:
        raise CheckerError(f"command names undeclared workspace package {package!r}")
    member, cargo = packages[package]
    if mode == "--test":
        if not IDENT_RE.fullmatch(target):
            raise CheckerError("integration target name is invalid")
        explicit = {item.get("name"): item.get("path", f"tests/{item.get('name')}.rs") for item in cargo.get("test", []) if isinstance(item, dict)}
        if cargo.get("package", {}).get("autotests", True):
            rel = explicit.get(target, f"tests/{target}.rs")
        elif target in explicit:
            rel = explicit[target]
        else:
            raise CheckerError(f"integration target {target!r} is not declared")
        full = (member / safe_relative(rel, "test target path")).as_posix()
    elif mode == "--lib":
        if not target.endswith("::tests"):
            raise CheckerError("--lib filter must end in ::tests")
        module = target[:-7]
        if not module or any(not IDENT_RE.fullmatch(part) for part in module.split("::")):
            raise CheckerError("--lib module filter is invalid")
        lib_rel = cargo.get("lib", {}).get("path", "src/lib.rs")
        lib_path = (member / safe_relative(lib_rel, "lib target path")).as_posix()
        lib_file = inspect_relative(repo_root, lib_path, "crate lib target", True)
        first = module.split("::", 1)[0]
        if f"mod {first};" not in lib_file.read_text(encoding="utf-8"):
            raise CheckerError(f"module {first!r} is not declared by {package}")
        full = (member / "src" / (module.replace("::", "/") + ".rs")).as_posix()
    else:
        raise CheckerError("command must contain exactly one --test or --lib target")
    inspect_relative(repo_root, full, "cargo test target", True)
    return full

def expected_v_to_e() -> dict[str, set[str]]:
    result = {f"V{i:03}": set() for i in range(1, 99)}
    for task, scenarios in EXPECTED_E_SCENARIOS.items():
        for scenario in scenarios:
            result[scenario].add(task)
    return result

def evidence_expected_refs(e_tasks: set[str], evidence_by_task: dict[str, set[str]]) -> set[str]:
    return {item for task in e_tasks for item in evidence_by_task[task]}

def validate_evidence(record: Any, task: str, repo_root: Path, packages: dict[str, tuple[Path, dict[str, Any]]]) -> str:
    if not isinstance(record, dict):
        raise CheckerError(f"{task}: evidence record is not an object")
    identifier = nonempty(record.get("id"), f"{task} evidence id")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", identifier):
        raise CheckerError(f"{task}: evidence id is invalid")
    if record.get("plan_version") != PLAN_VERSION or record.get("plan_sha256") != PLAN_SHA256:
        raise CheckerError(f"{identifier}: plan binding differs")
    source_sha = record.get("source_sha")
    if not isinstance(source_sha, str) or not HEX40.fullmatch(source_sha):
        raise CheckerError(f"{identifier}: source_sha must be lowercase 40-hex")
    pr = record.get("pr")
    if not isinstance(pr, str) or PR_RE.fullmatch(pr) is None:
        raise CheckerError(f"{identifier}: PR URL is invalid")
    merged = record.get("merged_sha")
    if merged is not None and (not isinstance(merged, str) or not HEX40.fullmatch(merged)):
        raise CheckerError(f"{identifier}: merged_sha must be null or lowercase 40-hex")
    digest = record.get("input_digest")
    if not isinstance(digest, str) or not HEX64.fullmatch(digest):
        raise CheckerError(f"{identifier}: input_digest must be lowercase SHA-256")
    for field in ("input_ref", "log_path", "record_source"):
        safe_relative(record.get(field), f"{identifier} {field}")
    test_entry = safe_relative(record.get("test_entry"), f"{identifier} test_entry")
    inspect_relative(repo_root, test_entry, f"{identifier} test_entry", True)
    command = nonempty(record.get("command"), f"{identifier} command")
    if resolve_cargo_target(repo_root, command, packages) != test_entry:
        raise CheckerError(f"{identifier}: command target differs from test_entry")
    exit_code = record.get("exit_code")
    if type(exit_code) is not int or exit_code != 0:
        raise CheckerError(f"{identifier}: verified execution must have integer exit_code 0")
    for field in ("description", "actual_result", "scope", "risk", "rollback"):
        nonempty(record.get(field), f"{identifier} {field}")
    return identifier

def validate_refs(refs: Any, what: str, evidence_owner: dict[str, str], relevant_tasks: set[str]) -> set[str]:
    values = string_set(refs, what)
    unknown = values - set(evidence_owner)
    if unknown:
        raise CheckerError(f"{what}: unknown evidence refs {sorted(unknown)}")
    irrelevant = {ref for ref in values if evidence_owner[ref] not in relevant_tasks}
    if irrelevant:
        raise CheckerError(f"{what}: evidence refs are outside the mapped E scope: {sorted(irrelevant)}")
    return values

def validate_manifest(manifest: Any, repo_root: Path) -> list[dict[str, Any]]:
    if not isinstance(manifest, dict):
        raise CheckerError("manifest root must be an object")
    if manifest.get("schema_version") != SCHEMA_VERSION:
        raise CheckerError(f"unsupported schema_version: {manifest.get('schema_version')!r}")
    if manifest.get("plan_version") != PLAN_VERSION or manifest.get("plan_sha256") != PLAN_SHA256:
        raise CheckerError("manifest is not bound to the frozen v4.1 plan")
    if manifest.get("declaration") != "subset_only" or manifest.get("not_full_route_complete") is not True or manifest.get("legacy_equivalence_unverified") is not True or manifest.get("dimensions", {}).get("effect") != "not_claimed":
        raise CheckerError("manifest overstates completion/effect or drops legacy limitation")
    if string_set(manifest.get("optional_disabled"), "optional_disabled") != {"E17", "E18"}:
        raise CheckerError("E17/E18 must remain disabled")
    trace_coverage = manifest.get("trace_index_coverage")
    expected_coverage = {
        "E00": {"purpose": "index_consistency_only_not_scenario_execution", "ranges": {"K01–K10", "SO01–SO18", "V087–V098"}},
        "E16.6": {"purpose": "complete_derived_index_coverage_not_scenario_execution", "ranges": {"B01–B10", "U01–U08", "K01–K10", "SO01–SO18", "E00–E18", "E16.1–E16.6", "V001–V098"}},
    }
    if not isinstance(trace_coverage, dict) or set(trace_coverage) != set(expected_coverage):
        raise CheckerError("trace_index_coverage must separately contain E00 and E16.6")
    for task, expected in expected_coverage.items():
        if trace_coverage[task].get("purpose") != expected["purpose"] or string_set(trace_coverage[task].get("ranges"), f"{task} trace ranges") != expected["ranges"]:
            raise CheckerError(f"{task} trace index responsibility differs")

    packages = workspace_packages(repo_root)
    scopes = manifest.get("e_scopes")
    if not isinstance(scopes, dict) or set(scopes) != EXPECTED_E_IDS:
        raise CheckerError(f"e_scopes must be the exact 25 task set; got {sorted(scopes) if isinstance(scopes, dict) else scopes!r}")
    evidence_records: list[dict[str, Any]] = []
    evidence_owner: dict[str, str] = {}
    for task, entry in scopes.items():
        if not isinstance(entry, dict):
            raise CheckerError(f"{task}: entry must be an object")
        nonempty(entry.get("title"), f"{task} title")
        status = entry.get("status")
        if status not in STATUSES:
            raise CheckerError(f"{task}: unknown status {status!r}")
        nonempty(entry.get("status_reason"), f"{task} status_reason")
        files = entry.get("implementation_files")
        if not isinstance(files, list) or any(not isinstance(item, str) for item in files) or len(files) != len(set(files)):
            raise CheckerError(f"{task}: implementation_files must be a duplicate-free string array")
        if status != "planned" and not files:
            raise CheckerError(f"{task}: non-planned scope requires implementation files")
        for file in files:
            inspect_relative(repo_root, file, f"{task} implementation file", True)
        if string_set(entry.get("scenarios"), f"{task} scenarios") != EXPECTED_E_SCENARIOS[task]:
            raise CheckerError(f"{task}: scenario execution responsibility differs")
        remaining = entry.get("remaining")
        if not isinstance(remaining, list) or any(not isinstance(item, str) or not item.strip() for item in remaining):
            raise CheckerError(f"{task}: remaining must be an array of non-empty strings")
        records = entry.get("verified_subscopes")
        if not isinstance(records, list):
            raise CheckerError(f"{task}: verified_subscopes must be an array")
        if status == "verified" and (remaining or not records):
            raise CheckerError(f"{task}: verified status requires evidence and no remaining work")
        if status != "verified" and not remaining:
            raise CheckerError(f"{task}: non-verified task must state remaining work")
        for record in records:
            identifier = validate_evidence(record, task, repo_root, packages)
            if identifier in evidence_owner:
                raise CheckerError(f"duplicate evidence id {identifier}")
            evidence_owner[identifier] = task
            evidence_records.append(record)
        if status == "verified":
            completion = string_set(
                entry.get("completion_evidence_refs"),
                f"{task} completion evidence refs",
            )
            owned = {ref for ref, owner in evidence_owner.items() if owner == task}
            if not completion or not completion.issubset(owned):
                raise CheckerError(
                    f"{task}: verified status lacks explicit task completion evidence"
                )

    trace = manifest.get("traceability")
    if not isinstance(trace, dict):
        raise CheckerError("traceability must be an object")
    so_entries = trace.get("so_sources")
    if not isinstance(so_entries, list):
        raise CheckerError("so_sources must be an array")
    so_by_id = {entry.get("id"): entry for entry in so_entries if isinstance(entry, dict)}
    if len(so_by_id) != len(so_entries) or set(so_by_id) != set(EXPECTED_SO):
        raise CheckerError("SO01-SO18 must be exact and duplicate-free")
    for sid, expected in EXPECTED_SO.items():
        entry = so_by_id[sid]
        path, blob, read_range, tasks_text = expected
        if entry.get("repository") != SKILLOPT_REPO or entry.get("commit") != SKILLOPT_COMMIT or entry.get("path") != path or entry.get("blob_sha") != blob or entry.get("read_range") != read_range or entry.get("license") != "MIT":
            raise CheckerError(f"{sid}: fixed source pin differs")
        tasks = set(tasks_text.split())
        if string_set(entry.get("e_tasks"), f"{sid} e_tasks") != tasks:
            raise CheckerError(f"{sid}: E mapping differs")
        nonempty(entry.get("license_status"), f"{sid} license_status")
        source_status = entry.get("verification_status")
        if source_status not in {"reference_pins_only", "verified"}:
            raise CheckerError(f"{sid}: unknown verification_status")
        local = entry.get("local_use")
        if not isinstance(local, dict) or local.get("mode") not in {"reference_only_no_upstream_runtime_import", "adapted_with_license_notice"}:
            raise CheckerError(f"{sid}: local use mode is invalid")
        artifacts = local.get("artifacts")
        if not isinstance(artifacts, list) or not artifacts or len(artifacts) != len(set(artifacts)):
            raise CheckerError(f"{sid}: local landing is absent or duplicated")
        for artifact in artifacts:
            inspect_relative(repo_root, artifact, f"{sid} local artifact", True)
        source_refs = entry.get("verification_evidence_refs", [])
        refs = validate_refs(source_refs, f"{sid} evidence refs", evidence_owner, tasks)
        if source_status == "verified":
            completion = string_set(
                entry.get("completion_evidence_refs"),
                f"{sid} completion evidence refs",
            )
            if not completion or not completion.issubset(refs):
                raise CheckerError(f"{sid}: verified source status lacks completion evidence")

    expected_v = expected_v_to_e()
    v_entries = trace.get("v_scenarios")
    if not isinstance(v_entries, list):
        raise CheckerError("v_scenarios must be an array")
    v_by_id = {entry.get("id"): entry for entry in v_entries if isinstance(entry, dict)}
    if len(v_by_id) != len(v_entries) or set(v_by_id) != set(expected_v):
        raise CheckerError("V001-V098 must be exact and duplicate-free")
    for vid, tasks in expected_v.items():
        entry = v_by_id[vid]
        if entry.get("specification_status") != "included" or entry.get("effect_status") != "not_claimed":
            raise CheckerError(f"{vid}: specification/effect axes differ")
        status, verification = entry.get("status"), entry.get("verification_status")
        if status not in STATUSES or verification not in STATUSES:
            raise CheckerError(f"{vid}: unknown status")
        if status == "planned" and verification != "planned":
            raise CheckerError(f"{vid}: planned scenario has incompatible verification status")
        if (status == "verified") != (verification == "verified"):
            raise CheckerError(f"{vid}: verified status axes disagree")
        if string_set(entry.get("e_tasks"), f"{vid} e_tasks") != tasks:
            raise CheckerError(f"{vid}: E responsibility differs")
        refs = validate_refs(entry.get("related_evidence_refs"), f"{vid} evidence refs", evidence_owner, tasks)
        remaining = nonempty(entry.get("remaining"), f"{vid} remaining") if status != "verified" else entry.get("remaining")
        if status == "verified":
            completion = string_set(
                entry.get("completion_evidence_refs"),
                f"{vid} completion evidence refs",
            )
            if (
                not refs
                or not completion
                or not completion.issubset(refs)
                or remaining not in (None, "", [])
            ):
                raise CheckerError(
                    f"{vid}: verified scenario lacks completion evidence or retains unfinished scope"
                )

    groups = {"b_commitments": "B", "u_commitments": "U", "k_commitments": "K"}
    for field, prefix in groups.items():
        entries = trace.get(field)
        if not isinstance(entries, list):
            raise CheckerError(f"{field} must be an array")
        expected_ids = {key for key in EXPECTED_COMMITMENTS if key.startswith(prefix)}
        by_id = {entry.get("id"): entry for entry in entries if isinstance(entry, dict)}
        if len(by_id) != len(entries) or set(by_id) != expected_ids:
            raise CheckerError(f"{field} exact set differs")
        for cid in expected_ids:
            entry = by_id[cid]
            sections, sources, tasks_text, scenarios = EXPECTED_COMMITMENTS[cid]
            tasks = set(tasks_text.split())
            if entry.get("specification_status") != "included" or entry.get("effect_status") != "not_claimed":
                raise CheckerError(f"{cid}: specification/effect axes differ")
            implementation, verification = entry.get("implementation_status"), entry.get("verification_status")
            if implementation not in STATUSES or verification not in STATUSES:
                raise CheckerError(f"{cid}: unknown four-axis status")
            if implementation == "planned" and verification != "planned":
                raise CheckerError(f"{cid}: planned implementation has incompatible verification")
            if verification == "verified" and implementation in {"planned", "blocked"}:
                raise CheckerError(f"{cid}: verified commitment has incompatible implementation status")
            if string_set(entry.get("sections"), f"{cid} sections") != set(sections.split()) or string_set(entry.get("sources"), f"{cid} sources") != set(sources.split()) or string_set(entry.get("e_tasks"), f"{cid} e_tasks") != tasks or string_set(entry.get("v_scenarios"), f"{cid} scenarios") != set(scenarios.split()):
                raise CheckerError(f"{cid}: normative trace chain differs")
            artifacts = entry.get("local_artifacts")
            if not isinstance(artifacts, list) or not artifacts or len(artifacts) != len(set(artifacts)):
                raise CheckerError(f"{cid}: local artifacts are absent or duplicated")
            for artifact in artifacts:
                inspect_relative(repo_root, artifact, f"{cid} local artifact", True)
            refs = validate_refs(entry.get("related_evidence_refs"), f"{cid} evidence refs", evidence_owner, tasks)
            if verification == "verified":
                completion = string_set(
                    entry.get("completion_evidence_refs"),
                    f"{cid} completion evidence refs",
                )
                if not refs or not completion or not completion.issubset(refs):
                    raise CheckerError(
                        f"{cid}: verified commitment lacks completion evidence"
                    )
            if verification != "verified":
                nonempty(entry.get("remaining"), f"{cid} remaining")
    return evidence_records

@dataclass
class CheckReport:
    structure_valid: bool = True
    plan_binding_available: bool = False
    input_binding_available: bool = False
    input_binding_scope: str = "local_input_manifest_hashes_only"
    needs_verification: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    plan_version: str | None = None
    plan_sha256: str | None = None
    def fail(self, message: str) -> None:
        self.structure_valid = False
        self.errors.append(message)
    def to_dict(self) -> dict[str, Any]:
        return {
            "structure_valid": self.structure_valid,
            "plan_binding_available": self.plan_binding_available,
            "input_binding_available": self.input_binding_available,
            "input_binding_scope": self.input_binding_scope,
            "needs_verification": self.needs_verification,
            "errors": self.errors,
            "plan_version": self.plan_version,
            "plan_sha256": self.plan_sha256,
            "disclaimer": "exit 0 proves only the frozen derivative's local structure. Commands were parsed, never executed; Git ancestry, logs, licenses, hosts, effects, and deployment were not automatically authenticated.",
        }

def validate_external_plan(path: Path) -> str:
    if path.is_symlink() or not path.is_file():
        raise CheckerError(f"source-of-truth is linked or not a regular file: {path}")
    actual = sha256_of_file(path)
    if actual != PLAN_SHA256:
        raise CheckerError(f"source-of-truth SHA-256 mismatch: computed={actual} expected={PLAN_SHA256}")
    return actual

def run_check(repo_root: Path, manifest_rel: str, source_of_truth: Path | None) -> CheckReport:
    report = CheckReport()
    if not repo_root.is_dir():
        report.fail(f"repo root does not exist or is not a directory: {repo_root}")
        return report
    if source_of_truth is not None:
        try:
            validate_external_plan(source_of_truth)
            report.plan_binding_available = True
        except CheckerError as exc:
            report.fail(str(exc))
            return report
    else:
        report.needs_verification.append("no --source-of-truth supplied: plan bytes were not locally hash-bound")
    try:
        manifest_path = inspect_relative(repo_root, manifest_rel, "manifest", True)
        manifest = load_json_strict(manifest_path.read_text(encoding="utf-8"))
        report.plan_version = manifest.get("plan_version") if isinstance(manifest, dict) else None
        report.plan_sha256 = manifest.get("plan_sha256") if isinstance(manifest, dict) else None
        evidence = validate_manifest(manifest, repo_root)
        all_inputs = bool(evidence)
        for record in evidence:
            identifier = record["id"]
            input_path = inspect_relative(repo_root, record["input_ref"], f"{identifier} input_ref", False)
            if input_path is None:
                all_inputs = False
                report.needs_verification.append(f"{identifier}: local input manifest is unavailable")
            elif sha256_of_file(input_path) != record["input_digest"]:
                all_inputs = False
                raise CheckerError(f"{identifier}: local input manifest SHA-256 mismatch")
            for field in ("log_path", "record_source"):
                path = inspect_relative(repo_root, record[field], f"{identifier} {field}", False)
                if path is None:
                    report.needs_verification.append(f"{identifier}: local {field} is unavailable")
        report.input_binding_available = all_inputs
        report.needs_verification.extend([
            "manifest commands were parsed but never executed; exit codes and actual results require controller verification",
            "source_sha, PR ancestry, merge state, upstream SO objects, and license conclusions require independent verification",
        ])
    except (CheckerError, OSError, UnicodeError) as exc:
        report.fail(str(exc))
    return report

def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Read-only rsia.support_scope.v2 structural checker")
    parser.add_argument("--repo-root", required=True, type=Path)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--source-of-truth", type=Path)
    return parser

def main(argv: list[str] | None = None) -> int:
    args = build_arg_parser().parse_args(argv)
    report = run_check(args.repo_root.absolute(), args.manifest, args.source_of_truth)
    print(json.dumps(report.to_dict(), indent=2, sort_keys=True))
    return 0 if report.structure_valid else 1

if __name__ == "__main__":
    sys.exit(main())
