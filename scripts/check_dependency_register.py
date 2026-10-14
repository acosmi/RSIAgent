#!/usr/bin/env python3
"""Closed, read-only dependency registration and checksum-bound primary evidence.

Input-only is an early structural gate. The default complete gate reads every
locked archive and never fetches, extracts, installs, or grants release rights.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import sys
import tarfile
import tomllib
import zlib

SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
MEMBERS = ("apps/rsia", "crates/evo-core", "crates/evo-engine", "crates/evo-http", "crates/evo-mcp", "crates/evo-storage")
MANIFESTS = ("Cargo.toml",) + tuple(member + "/Cargo.toml" for member in MEMBERS)
REGISTER_SCHEMA = "rsia.dependency_register.v1"
INVENTORY_SCHEMA = "rsia.dependency_license_inventory.v1"
LICENSE_ALLOWLIST = ['(MIT OR Apache-2.0) AND Unicode-3.0', 'Apache-2.0', 'Apache-2.0 AND ISC', 'Apache-2.0 OR BSL-1.0', 'Apache-2.0 OR BSL-1.0 OR MIT', 'Apache-2.0 OR ISC OR MIT', 'Apache-2.0 OR MIT', 'Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT', 'BSD-2-Clause OR Apache-2.0 OR MIT', 'BSD-3-Clause', 'CDLA-Permissive-2.0', 'ISC', 'MIT', 'MIT AND BSD-3-Clause', 'MIT OR Apache-2.0', 'MIT OR Apache-2.0 OR LGPL-2.1-or-later', 'MIT OR Apache-2.0 OR Zlib', 'Unicode-3.0', 'Unlicense OR MIT', 'Zlib', 'Zlib OR Apache-2.0 OR MIT']
LEGACY_EXCEPTIONS = {'Apache-2.0/MIT': [{'checksum': 'da0e4dd2a88388a1f4ccc7c9ce104604dab68d9f408dc34cd45823d5a9069095', 'name': 'flume', 'version': '0.11.1'}], 'MIT/Apache-2.0': [{'checksum': '9564fc758e15025b46aa6643b1b77d047d1a56a1aea6e01002ac0c7026876213', 'name': 'fs2', 'version': '0.4.3'}, {'checksum': 'b9e0384b61958566e926dc50660321d12159025e767c18e043daf26b70104c39', 'name': 'ident_case', 'version': '1.0.1'}, {'checksum': 'e661dda6640fad38e827a6d4a310ff4763082116fe217f279885c97f511bb0b7', 'name': 'num-bigint-dig', 'version': '0.8.6'}, {'checksum': 'b4596b6d070b27117e987119b4dac604f3c58cfb0b191112e24771b2faeac1a6', 'name': 'plain', 'version': '0.2.3'}, {'checksum': 'd3491c14715ca2294c4d6a88f15e84739788c1d030eed8c110436aafdaa2f3fd', 'name': 'serde_urlencoded', 'version': '0.7.1'}, {'checksum': '7b4df3d392d81bd458a8a621b8bffbd2302a12ffe288a9d931670948749463b1', 'name': 'stringprep', 'version': '0.1.5'}, {'checksum': '7df058c713841ad818f1dc5d3fd88063241cc61f49f5fbea4b951e8cf5a8d71d', 'name': 'unicode-properties', 'version': '0.1.4'}, {'checksum': 'accd4ea62f7bb7a82fe23066fb0957d48ef677f6eeb8215f372f52e48bb32426', 'name': 'vcpkg', 'version': '0.2.15'}, {'checksum': '0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a', 'name': 'version_check', 'version': '0.9.5'}, {'checksum': '5c839a674fcd7a98952e593242ea400abe93992746761e38641405d28b00f419', 'name': 'winapi', 'version': '0.3.9'}, {'checksum': 'ac3b87c63620426dd9b991e5ce0329eff545bccbbb34f3be09ff6fb6ab51b7b6', 'name': 'winapi-i686-pc-windows-gnu', 'version': '0.4.0'}, {'checksum': '712e227841d057c1ee1cd2fb22fa7e5a5461ae8e48fa2ca79ec42cfc1931183f', 'name': 'winapi-x86_64-pc-windows-gnu', 'version': '0.4.0'}]}
MAX_COMPRESSED = 64 * 1024 * 1024
MAX_INFLATED = 256 * 1024 * 1024
MAX_MEMBERS = 20_000
MAX_MEMBER = 64 * 1024 * 1024
MAX_METADATA = 1024 * 1024
MAX_TEXT = 2 * 1024 * 1024
MAX_TEXT_FILES = 64
MAX_TEXT_TOTAL = 16 * 1024 * 1024
MAX_JSON = 16 * 1024 * 1024
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
TEXT_NAME = re.compile(r"(?:LICENSE|LICENCE|COPYING|NOTICE)(?:[._-].*)?\Z", re.I)
DEPENDENCY_KEYS = {"version", "path", "package", "features", "default-features", "workspace"}
SECTION_KINDS = {"dependencies": "regular", "dev-dependencies": "dev", "build-dependencies": "build"}


class DependencyError(ValueError):
    """A stable, relative-path diagnostic suitable for public smoke controls."""


def refuse(category: str, message: str) -> None:
    raise DependencyError(f"{category}: {message}")


def digest(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def canonical(value: object) -> bytes:
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode()


def fields(value: object, wanted: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != wanted:
        refuse("SCHEMA", label + " has missing/unknown fields or wrong type")
    return value


def nonempty(value: object, label: str) -> str:
    if not isinstance(value, str) or not value.strip() or "\x00" in value:
        refuse("SCHEMA", label + " must be a nonempty string")
    return value


def sha(value: object, label: str) -> str:
    if not isinstance(value, str) or not HEX64.fullmatch(value):
        refuse("SCHEMA", label + " must be lowercase SHA-256")
    return value


def sequence(value: object, label: str) -> list:
    if not isinstance(value, list):
        refuse("SCHEMA", label + " must be an array")
    return value


def strings(value: object, label: str, *, sorted_unique: bool = False) -> list[str]:
    values = [nonempty(x, label) for x in sequence(value, label)]
    if len(values) != len(set(values)) or (sorted_unique and values != sorted(values)):
        refuse("SCHEMA", label + " must be unique" + (" and sorted" if sorted_unique else ""))
    return values


def relative(value: object, label: str) -> str:
    value = nonempty(value, label)
    parts = value.split("/")
    if "\\" in value or ":" in value or value.startswith("/") or any(part in {"", ".", ".."} for part in parts):
        refuse("SCHEMA", label + " must be a canonical relative POSIX path")
    return value


def unique_object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            refuse("SCHEMA", "duplicate JSON field " + key)
        result[key] = value
    return result


def read_regular(path: Path, label: str, limit: int, *, boundary: Path) -> bytes:
    # Reject traversal before any open, including a malicious archive basename.
    try:
        inside = path.relative_to(boundary)
    except ValueError:
        refuse("IDENTITY", label + " escapes the selected root")
    if any(part in {".", ".."} for part in inside.parts):
        refuse("IDENTITY", label + " has a noncanonical selected-root path")
    # Do not follow a symlink between the selected root and the final file.
    current = path
    while current != boundary:
        if current.is_symlink():
            refuse("IDENTITY", label + " is a symlink")
        if current.parent == current:
            refuse("IDENTITY", label + " escapes the selected root")
        current = current.parent
    if boundary.is_symlink():
        refuse("IDENTITY", label + " has a symlink root")
    try:
        before = path.lstat()
        if not stat.S_ISREG(before.st_mode) or before.st_size > limit:
            refuse("ARCHIVE_FORMAT", label + " is not a bounded regular file")
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
                refuse("ARCHIVE_FORMAT", label + " is not a bounded regular file")
            raw = stream.read(limit + 1)
            if len(raw) > limit:
                refuse("ARCHIVE_FORMAT", label + " exceeds its size limit")
            return raw
    except DependencyError:
        raise
    except OSError:
        refuse("PRIMARY_MISSING" if label.endswith(".crate") else "INPUT_BINDING", label + " cannot be read")


def json_file(root: Path, name: str) -> tuple[dict, bytes]:
    raw = read_regular(root / name, name, MAX_JSON, boundary=root)
    try:
        value = json.loads(raw, object_pairs_hook=unique_object, parse_constant=lambda _: refuse("SCHEMA", "nonfinite JSON number"))
    except DependencyError:
        raise
    except (ValueError, UnicodeError, RecursionError):
        refuse("SCHEMA", name + " is not valid UTF-8 JSON")
    if not isinstance(value, dict):
        refuse("SCHEMA", name + " root must be an object")
    return value, raw


def toml_file(root: Path, name: str) -> tuple[dict, str]:
    raw = read_regular(root / name, name, MAX_METADATA, boundary=root)
    try:
        return tomllib.loads(raw.decode()), digest(raw)
    except (ValueError, UnicodeError):
        refuse("INPUT_BINDING", name + " is not valid TOML")


def identity(row: dict) -> tuple[str, str, str]:
    return (row["name"], row["version"], row.get("source", ""))


def permitted_license(row: dict) -> None:
    raw = nonempty(row["license"], "license")
    if raw in LICENSE_ALLOWLIST:
        return
    qualified = {key: row[key] for key in ("name", "version", "checksum")}
    if raw not in LEGACY_EXCEPTIONS or qualified not in LEGACY_EXCEPTIONS[raw]:
        refuse("LICENSE_MISMATCH", "whole license expression is not approved for " + row["name"])


def version_matches(requirement: str, version: str) -> bool:
    if not re.fullmatch(r"(?:=[0-9]+\.[0-9]+\.[0-9]+|[0-9]+(?:\.[0-9]+){0,2})", requirement):
        refuse("VERSION", "unsupported dependency version requirement")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        refuse("VERSION", "unsupported direct locked version")
    actual = tuple(map(int, version.split(".")))
    numbers = tuple(map(int, requirement.lstrip("=").split(".")))
    if requirement.startswith("="):
        return actual == numbers
    lower = numbers + (0,) * (3 - len(numbers))
    position = next((index for index, number in enumerate(lower) if number), len(numbers) - 1)
    upper = list(lower)
    upper[position] += 1
    for index in range(position + 1, 3):
        upper[index] = 0
    return lower <= actual < tuple(upper)


def safe_package_name(value: object) -> str:
    if not isinstance(value, str) or len(value) > 64 or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]*", value):
        refuse("IDENTITY", "package name/alias is not a safe crate basename")
    return value


def safe_package_version(value: object) -> str:
    if not isinstance(value, str) or len(value) > 128 or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", value):
        refuse("IDENTITY", "locked version is not a safe crate basename")
    return value


def declaration(raw: object, label: str) -> dict:
    if isinstance(raw, str):
        return {"version": nonempty(raw, label)}
    if not isinstance(raw, dict) or not raw or not set(raw).issubset(DEPENDENCY_KEYS):
        refuse("SCHEMA", label + " has unsupported dependency shape")
    for key in ("workspace", "default-features"):
        if key in raw and type(raw[key]) is not bool:
            refuse("SCHEMA", label + "." + key + " must be boolean")
    for key in ("version", "path", "package"):
        if key in raw:
            nonempty(raw[key], label + "." + key)
    if "features" in raw:
        strings(raw["features"], label + ".features")
    if "workspace" in raw and raw["workspace"] is not True:
        refuse("SCHEMA", "workspace inheritance must be true")
    return raw


def graph(root: Path) -> tuple[dict, list[dict], list[dict], list[dict]]:
    manifests = {name: toml_file(root, name) for name in MANIFESTS}
    workspace = manifests["Cargo.toml"][0].get("workspace", {})
    if not isinstance(workspace, dict) or sorted(workspace.get("members", [])) != list(MEMBERS):
        refuse("EDGE_COVERAGE", "requires the exact six workspace members")
    shared = workspace.get("dependencies", {})
    if not isinstance(shared, dict):
        refuse("SCHEMA", "workspace dependencies must be a table")
    lock, lock_sha = toml_file(root, "Cargo.lock")
    packages = sequence(lock.get("package"), "lock packages")
    locked = {}
    for item in packages:
        if not isinstance(item, dict):
            refuse("SCHEMA", "lock package must be a table")
        row = {"name": safe_package_name(item.get("name")), "version": safe_package_version(item.get("version"))}
        if "source" in item:
            if item["source"] != SOURCE:
                refuse("SOURCE", "lock logical source is not approved")
            row.update(source=SOURCE, checksum=sha(item.get("checksum"), "lock checksum"))
        elif "checksum" in item:
            refuse("IDENTITY", "local lock package must not claim a registry checksum")
        key = identity(row)
        if key in locked:
            refuse("IDENTITY", "duplicate lock identity")
        locked[key] = row
    member_info = {}
    root_package = workspace.get("package", {})
    for member in MEMBERS:
        package = manifests[member + "/Cargo.toml"][0].get("package", {})
        name = nonempty(package.get("name"), "member name")
        version = package.get("version")
        if isinstance(version, dict):
            if set(version) != {"workspace"} or version["workspace"] is not True:
                refuse("SCHEMA", "member version inheritance must be workspace=true")
            version = root_package.get("version")
        version = nonempty(version, "member version")
        row = {"name": name, "version": version, "manifest": member + "/Cargo.toml"}
        if identity(row) not in locked or name in {x["name"] for x in member_info.values()}:
            refuse("IDENTITY", "workspace package does not match a unique local lock identity")
        member_info[member] = row
    local_keys = {identity(row) for row in member_info.values()}
    if local_keys != {key for key in locked if not key[2]}:
        refuse("IDENTITY", "local lock package coverage differs from six workspace members")
    edges = []
    used_shared = set()
    for member in MEMBERS:
        document = manifests[member + "/Cargo.toml"][0]
        sections = [(None, document)]
        targets = document.get("target", {})
        if not isinstance(targets, dict):
            refuse("SCHEMA", "target dependencies must be tables")
        sections += [(nonempty(target, "target"), table) for target, table in targets.items()]
        for target, table in sections:
            if not isinstance(table, dict):
                refuse("SCHEMA", "dependency section must be a table")
            for section, kind in SECTION_KINDS.items():
                dependencies = table.get(section, {})
                if not isinstance(dependencies, dict):
                    refuse("SCHEMA", "dependency section must be a table")
                for alias, raw in dependencies.items():
                    safe_package_name(alias)
                    own = declaration(raw, "consumer declaration")
                    inherited = own.get("workspace", False)
                    shared_raw = None
                    if inherited:
                        if set(own) - {"workspace", "features", "default-features"} or alias not in shared:
                            refuse("SCHEMA", "unsupported workspace consumer override")
                        shared_raw = shared[alias]
                        base = declaration(shared_raw, "workspace declaration")
                        if "workspace" in base:
                            refuse("SCHEMA", "nested workspace inheritance is unsupported")
                        used_shared.add(alias)
                    else:
                        base = own
                    default = base.get("default-features", True)
                    if inherited and "default-features" in own and own["default-features"] != default:
                        refuse("SCHEMA", "conflicting default-features inheritance is unsupported")
                    features = sorted(set(base.get("features", [])) | set(own.get("features", [])))
                    name = safe_package_name(base.get("package", alias))
                    if "path" in base:
                        origin = root if inherited else root / member
                        path_value = base["path"]
                        if Path(path_value).is_absolute() or "\\" in path_value or ":" in path_value:
                            refuse("IDENTITY", "internal dependency path must be local")
                        destination = (origin / path_value).resolve()
                        matches = [path for path in MEMBERS if (root / path).resolve() == destination]
                        if len(matches) != 1 or member_info[matches[0]]["name"] != name:
                            refuse("IDENTITY", "internal path/member/name mismatch")
                        candidate = member_info[matches[0]]
                        if "version" in base and not version_matches(base["version"], candidate["version"]):
                            refuse("VERSION", "internal dependency version mismatch")
                        key = (name, candidate["version"], "")
                        path = matches[0]
                    else:
                        if "version" not in base:
                            refuse("VERSION", "registry declaration has no supported version")
                        matches = [key for key in locked if key[0] == name and key[2] == SOURCE and version_matches(base["version"], key[1])]
                        if len(matches) != 1:
                            refuse("VERSION", "direct dependency does not resolve to one locked identity")
                        key, path = matches[0], None
                    consumer = {"manifest": member + "/Cargo.toml", "kind": kind, "target": target, "alias": alias, "declaration": raw, "workspace_declaration": shared_raw, "features": features, "default_features": default}
                    edges.append({"identity": key, "path": path, "consumer": consumer})
    if used_shared != set(shared):
        refuse("EDGE_COVERAGE", "workspace declarations lack registered consumption")
    inputs = {"cargo_lock_sha256": lock_sha, "manifests": {name: value[1] for name, value in manifests.items()}}
    return inputs, sorted(locked.values(), key=identity), sorted(member_info.values(), key=identity), edges


def consumer_key(row: dict) -> tuple:
    return row["manifest"], row["kind"], row["target"] or "", row["alias"]


def reference(value: object, label: str) -> dict:
    row = fields(value, {"path", "sha256"}, label)
    relative(row["path"], label + " path")
    sha(row["sha256"], label + " SHA")
    return row


def validate_inputs(value: object, actual: dict) -> None:
    inputs = fields(value, {"cargo_lock_sha256", "manifests"}, "inputs")
    sha(inputs["cargo_lock_sha256"], "lock SHA")
    fields(inputs["manifests"], set(MANIFESTS), "manifest bindings")
    for value in inputs["manifests"].values():
        sha(value, "manifest SHA")
    if inputs != actual:
        refuse("INPUT_BINDING", "lock/manifest byte bindings differ from actual inputs")


def inventory_rows(document: dict, inputs: dict, locked: list[dict], locals_: list[dict], root: Path) -> dict:
    fields(document, {"schema_version", "inputs", "registry_packages", "workspace_packages"}, "inventory")
    if document["schema_version"] != INVENTORY_SCHEMA:
        refuse("SCHEMA", "unsupported inventory schema")
    validate_inputs(document["inputs"], inputs)
    rows = {}
    for row in sequence(document["registry_packages"], "registry inventory"):
        fields(row, {"name", "version", "source", "checksum", "license", "metadata_sha256", "license_file", "bundled_license_texts", "bundled_text_status"}, "registry inventory row")
        for key in ("name", "version", "source", "license"):
            nonempty(row[key], key)
        if row["source"] != SOURCE:
            refuse("SOURCE", "inventory logical source is not approved")
        sha(row["checksum"], "inventory checksum")
        sha(row["metadata_sha256"], "primary metadata SHA")
        if row["license_file"] is not None:
            reference(row["license_file"], "license-file")
        texts = [reference(x, "bundled text") for x in sequence(row["bundled_license_texts"], "bundled texts")]
        paths = [x["path"] for x in texts]
        if len(texts) > MAX_TEXT_FILES or paths != sorted(set(paths)) or row["bundled_text_status"] != ("present" if texts else "not_bundled"):
            refuse("SCHEMA", "bundled text coverage/status is invalid")
        permitted_license(row)
        key = identity(row)
        if key in rows:
            refuse("IDENTITY", "duplicate inventory identity")
        rows[key] = row
    expected = {identity(row): row for row in locked if "source" in row}
    if set(rows) != set(expected):
        refuse("IDENTITY", "registry inventory does not cover the exact lock identities")
    for key, row in rows.items():
        if any(row[name] != expected[key][name] for name in ("name", "version", "source", "checksum")):
            refuse("IDENTITY", "inventory identity/checksum does not match lock")
    local_rows = {}
    project_license = {"path": "LICENSE", "sha256": digest(read_regular(root / "LICENSE", "LICENSE", MAX_TEXT, boundary=root))}
    for row in sequence(document["workspace_packages"], "workspace inventory"):
        fields(row, {"name", "version", "manifest", "license_file"}, "workspace inventory row")
        nonempty(row["name"], "workspace name")
        nonempty(row["version"], "workspace version")
        relative(row["manifest"], "workspace manifest")
        reference(row["license_file"], "project license-file")
        key = identity(row)
        if key in local_rows:
            refuse("IDENTITY", "duplicate workspace inventory identity")
        local_rows[key] = row
    wanted = {identity(row): dict(row, license_file=project_license) for row in locals_}
    if local_rows != wanted:
        refuse("IDENTITY", "workspace inventory does not match real local packages/LICENSE")
    return rows


def register_rows(document: dict, inputs: dict, inventory: dict, inventory_raw: bytes, edges: list[dict]) -> None:
    fields(document, {"schema_version", "inputs", "inventory_sha256", "source_allowlist", "license_allowlist", "legacy_license_exceptions", "dependencies", "internal_packages"}, "register")
    if document["schema_version"] != REGISTER_SCHEMA:
        refuse("SCHEMA", "unsupported register schema")
    validate_inputs(document["inputs"], inputs)
    if sha(document["inventory_sha256"], "inventory SHA") != digest(inventory_raw):
        refuse("INPUT_BINDING", "register inventory SHA differs from actual bytes")
    if document["source_allowlist"] != [SOURCE]:
        refuse("SOURCE", "source allowlist differs from the reviewed whole policy")
    if document["license_allowlist"] != LICENSE_ALLOWLIST or document["legacy_license_exceptions"] != LEGACY_EXCEPTIONS:
        refuse("LICENSE_MISMATCH", "license policy differs from the reviewed whole/exact policy")
    expected = {}
    for edge in edges:
        key = edge["identity"]
        expected.setdefault(key, {"path": edge["path"], "consumers": []})["consumers"].append(edge["consumer"])
    seen = {}
    for external, label in ((True, "dependencies"), (False, "internal_packages")):
        for row in sequence(document[label], label):
            wanted = {"name", "version", "source", "checksum", "license", "purpose", "alternative", "consumers"} if external else {"name", "version", "path", "consumers"}
            fields(row, wanted, "register package")
            nonempty(row["name"], "register name")
            nonempty(row["version"], "register version")
            if external:
                if row["source"] != SOURCE:
                    refuse("SOURCE", "register logical source is not approved")
                sha(row["checksum"], "register checksum")
                permitted_license(row)
                nonempty(row["purpose"], "purpose")
                nonempty(row["alternative"], "alternative")
            else:
                relative(row["path"], "internal package path")
            key = identity(row)
            if key in seen:
                refuse("IDENTITY", "duplicate register identity")
            if key not in expected:
                refuse("EDGE_COVERAGE", "extra or version-mismatched register package")
            if external:
                if any(row[name] != inventory[key][name] for name in ("name", "version", "source", "checksum", "license")):
                    refuse("LICENSE_MISMATCH", "register identity/license differs from inventory")
            elif row["path"] != expected[key]["path"]:
                refuse("IDENTITY", "registered internal path differs from manifest")
            consumers = []
            for consumer in sequence(row["consumers"], "consumers"):
                fields(consumer, {"manifest", "kind", "target", "alias", "declaration", "workspace_declaration", "features", "default_features"}, "consumer")
                relative(consumer["manifest"], "consumer manifest")
                nonempty(consumer["alias"], "consumer alias")
                if consumer["kind"] not in {"regular", "dev", "build"} or type(consumer["default_features"]) is not bool:
                    refuse("SCHEMA", "consumer kind/default-features is invalid")
                if consumer["target"] is not None:
                    nonempty(consumer["target"], "consumer target")
                declaration(consumer["declaration"], "registered declaration")
                if consumer["workspace_declaration"] is not None:
                    declaration(consumer["workspace_declaration"], "registered workspace declaration")
                strings(consumer["features"], "consumer features", sorted_unique=True)
                consumers.append(consumer)
            keys = [consumer_key(row) for row in consumers]
            if keys != sorted(set(keys)) or consumers != sorted(expected[key]["consumers"], key=consumer_key):
                refuse("EDGE_COVERAGE", "registered consumer/target/features differ from real declarations for " + row["name"])
            seen[key] = row
    if set(seen) != set(expected):
        refuse("EDGE_COVERAGE", "register does not cover every declared dependency identity")


def primary_home(fixture_root: Path | None) -> Path:
    if fixture_root is not None:
        selected = fixture_root
    elif os.environ.get("CARGO_HOME"):
        selected = Path(os.environ["CARGO_HOME"])
    else:
        selected = Path.home() / ".cargo"
    if selected.is_symlink():
        refuse("IDENTITY", "selected primary root is a symlink")
    return selected.absolute()


def archive_bytes(row: dict, home: Path) -> bytes:
    safe_package_name(row.get("name"))
    safe_package_version(row.get("version"))
    if row.get("source") != SOURCE:
        refuse("SOURCE", "primary logical source is not approved")
    sha(row.get("checksum"), "primary checksum")
    cache = home / "registry/cache"
    if cache.is_symlink() or (home / "registry").is_symlink():
        refuse("IDENTITY", "registry cache is a symlink")
    try:
        transports = sorted(cache.iterdir(), key=lambda path: path.name)
    except OSError:
        refuse("PRIMARY_MISSING", "registry cache is unavailable")
    if len(transports) > 64:
        refuse("ARCHIVE_FORMAT", "too many registry transport entries")
    filename = row["name"] + "-" + row["version"] + ".crate"
    payload = None
    for directory in transports:
        candidate = directory / filename
        if directory.is_symlink():
            refuse("IDENTITY", "registry transport directory is a symlink")
        if not directory.is_dir():
            refuse("IDENTITY", "registry transport entry is not a directory")
        if not candidate.exists() and not candidate.is_symlink():
            continue
        raw = read_regular(candidate, filename, MAX_COMPRESSED, boundary=home)
        if digest(raw) != row["checksum"]:
            refuse("CHECKSUM", "archive transport copy differs from locked checksum for " + row["name"])
        if payload is None:
            payload = raw
    if payload is None:
        refuse("PRIMARY_MISSING", "locked archive missing for " + row["name"])
    return payload


def primary_record(row: dict, home: Path) -> dict:
    raw = archive_bytes(row, home)
    try:
        with gzip.GzipFile(fileobj=io.BytesIO(raw)) as compressed:
            inflated = compressed.read(MAX_INFLATED + 1)
        if len(inflated) > MAX_INFLATED:
            refuse("ARCHIVE_FORMAT", "inflated archive exceeds limit")
        prefix = row["name"] + "-" + row["version"]
        files = {}
        with tarfile.open(fileobj=io.BytesIO(inflated), mode="r:") as archive:
            count = 0
            names = set()
            for member in archive:
                count += 1
                if count > MAX_MEMBERS or member.size < 0 or member.size > MAX_MEMBER:
                    refuse("ARCHIVE_FORMAT", "archive member count/size exceeds limit")
                header = inflated[member.offset_data - 512:member.offset_data]
                name_bytes, name_nul, name_padding = header[:100].partition(b"\x00")
                if name_nul and any(name_padding):
                    refuse("ARCHIVE_FORMAT", "nonzero suffix hidden after raw archive name NUL")
                header_name = name_bytes.decode("utf-8", "surrogateescape")
                if header[257:263] == b"ustar\x00":
                    prefix_bytes, prefix_nul, prefix_padding = header[345:500].partition(b"\x00")
                    if prefix_nul and any(prefix_padding):
                        refuse("ARCHIVE_FORMAT", "nonzero suffix hidden after USTAR prefix NUL")
                    header_prefix = prefix_bytes.decode("utf-8", "surrogateescape")
                    if header_prefix:
                        header_name = header_prefix + "/" + header_name
                archive_paths = [header_name]
                position = member.offset
                actual_header = member.offset_data - 512
                while position < actual_header:
                    extended = tarfile.TarInfo.frombuf(inflated[position:position + 512], "utf-8", "surrogateescape")
                    count += 1
                    if count > MAX_MEMBERS or extended.size > MAX_METADATA or extended.type not in {tarfile.GNUTYPE_LONGNAME, tarfile.GNUTYPE_LONGLINK, tarfile.XHDTYPE, tarfile.XGLTYPE}:
                        refuse("ARCHIVE_FORMAT", "unsupported or oversized extended archive header")
                    if extended.type == tarfile.GNUTYPE_LONGNAME:
                        header_name = inflated[position + 512:position + 512 + extended.size].rstrip(b"\x00").decode("utf-8", "surrogateescape")
                        archive_paths.append(header_name)
                    position += 512 + ((extended.size + 511) // 512) * 512
                if position != actual_header:
                    refuse("ARCHIVE_FORMAT", "extended archive header alignment differs")
                archive_paths.append(member.pax_headers.get("path", header_name))
                for original_name in archive_paths:
                    if member.isdir() and original_name.endswith("/"):
                        original_name = original_name[:-1]
                    try:
                        name = relative(original_name, "archive member")
                    except DependencyError:
                        refuse("ARCHIVE_FORMAT", "noncanonical raw/effective archive member path")
                    if PurePosixPath(name).parts[0] != prefix:
                        refuse("ARCHIVE_FORMAT", "raw/effective archive header escapes the package root")
                if name != member.name or member.type not in {tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE}:
                    refuse("ARCHIVE_FORMAT", "normalized or nonordinary archive header")
                if name in names or PurePosixPath(name).parts[0] != prefix or not (member.isfile() or member.isdir()):
                    refuse("ARCHIVE_FORMAT", "duplicate, escaping, or nonordinary archive member")
                names.add(name)
                if member.isfile():
                    files[name] = member
            tail = inflated[archive.offset:]
            if len(tail) < 1024 or any(tail):
                refuse("ARCHIVE_FORMAT", "archive lacks a clean final tar terminator")
            metadata_name = prefix + "/Cargo.toml"
            if metadata_name not in files or files[metadata_name].size > MAX_METADATA:
                refuse("ARCHIVE_FORMAT", "unique ordinary primary Cargo.toml missing or oversized")
            metadata = archive.extractfile(files[metadata_name]).read(MAX_METADATA + 1)
            package = tomllib.loads(metadata.decode()).get("package", {})
            if not isinstance(package, dict) or package.get("name") != row["name"] or package.get("version") != row["version"]:
                refuse("IDENTITY", "primary metadata name/version differs from lock")
            license_raw = nonempty(package.get("license"), "primary license")
            license_file = None
            if "license-file" in package:
                value = relative(package["license-file"], "primary license-file")
                filename = prefix + "/" + value
                if filename not in files or files[filename].size > MAX_TEXT:
                    refuse("LICENSE_FILE", "primary license-file reference is missing or oversized")
                license_file = {"path": filename, "sha256": digest(archive.extractfile(files[filename]).read(MAX_TEXT + 1))}
            texts, total = [], 0
            for filename, member in sorted(files.items()):
                if not TEXT_NAME.fullmatch(PurePosixPath(filename).name):
                    continue
                total += member.size
                if member.size > MAX_TEXT or total > MAX_TEXT_TOTAL or len(texts) >= MAX_TEXT_FILES:
                    refuse("ARCHIVE_FORMAT", "bundled license text exceeds limit")
                texts.append({"path": filename, "sha256": digest(archive.extractfile(member).read(MAX_TEXT + 1))})
            result = {key: row[key] for key in ("name", "version", "source", "checksum")}
            result.update(license=license_raw, metadata_sha256=digest(metadata), license_file=license_file, bundled_license_texts=texts, bundled_text_status="present" if texts else "not_bundled")
            return result
    except DependencyError:
        raise
    except (OSError, ValueError, EOFError, UnicodeError, tarfile.TarError, zlib.error):
        refuse("ARCHIVE_FORMAT", "invalid compressed primary archive for " + row["name"])


def inspect_dependencies(root: Path, *, complete: bool = True, fixture_root: Path | None = None) -> tuple[list[str], dict]:
    info = {"schema_version": "rsia.dependency_check.v1", "phase": "complete" if complete else "input_only", "primary_checked": 0, "notice": "Technical provenance and mechanical policy only; no legal clearance or release rights."}
    try:
        root = root.absolute()
        inputs, locked, locals_, edges = graph(root)
        inventory, inventory_raw = json_file(root, "docs/dependency-license-inventory.json")
        register, _ = json_file(root, "docs/dependency-register.json")
        rows = inventory_rows(inventory, inputs, locked, locals_, root)
        register_rows(register, inputs, rows, inventory_raw, edges)
        if complete:
            home = primary_home(fixture_root)
            for key, expected in sorted(rows.items()):
                actual = primary_record(expected, home)
                if actual["license"] != expected["license"]:
                    refuse("LICENSE_MISMATCH", "primary whole license differs from inventory for " + key[0])
                if actual != expected:
                    refuse("IDENTITY", "primary metadata/text provenance differs from inventory for " + key[0])
                info["primary_checked"] += 1
        info.update(cargo_lock_sha256=inputs["cargo_lock_sha256"], external_direct=len(register["dependencies"]), declaration_edges=len(edges), registry_locked=len(rows), local_locked=len(locals_))
        return [], info
    except DependencyError as error:
        return [str(error)], info
    except (OSError, ValueError, TypeError, KeyError, AttributeError):
        return ["SCHEMA: malformed or unavailable dependency inputs"], info


def dependency_register_errors(root: Path, *, complete: bool = True, fixture_root: Path | None = None) -> list[str]:
    return inspect_dependencies(root, complete=complete, fixture_root=fixture_root)[0]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--input-only", action="store_true")
    parser.add_argument("--fixture-root", type=Path)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    errors, info = inspect_dependencies(args.repo_root, complete=not args.input_only, fixture_root=args.fixture_root)
    if errors:
        for error in errors:
            print("DEPENDENCY_REGISTER_FAIL: " + error, file=sys.stderr)
        return 1
    print(json.dumps(info, sort_keys=True) if args.json else "DEPENDENCY_REGISTER_OK: " + info["phase"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
