#!/usr/bin/env python3
"""Workspace identity smoke: lockfile, members, migrations, toolchain.

Verifies the tree under test; does not rewrite inputs (V001).
"""
from __future__ import annotations

import hashlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPECTED_MEMBERS = [
    "crates/evo-core",
    "crates/evo-storage",
    "crates/evo-engine",
    "crates/evo-mcp",
    "crates/evo-http",
    "apps/rsia",
]

# 0003 belonged to the unavailable historical increment. It remains reserved
# even though no 0003 file is present in this checkout.
RESERVED_MIGRATION_NUMBERS = {3}
BASELINE_MIGRATION_SHA256 = {
    "0001_runtime.sql": "a2ccef4eba4411a4a5f83aa6b32525345edd7e0cd5e3aaff54b8bc8ef672e684",
    "0002_revoke_graph.sql": "1695a07425e95a13338a349887c98ba3e4a90ed9cfff35a2f46f49c488c1e057",
    "0004_replay_worlds.sql": "528d50b397f49021c65484b26c2d4e9f0cfdadf4074648af55c19af843f47fb9",
}
MIGRATION_RE = re.compile(r"^(\d{4})_.+\.sql$")


def fail(msg: str) -> int:
    print(f"SMOKE_WORKSPACE_FAIL: {msg}", file=sys.stderr)
    return 1


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    lock = ROOT / "Cargo.lock"
    if not lock.is_file():
        return fail("Cargo.lock missing")
    if b"\r\n" in lock.read_bytes():
        return fail("Cargo.lock contains CRLF")

    cargo_toml = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    for member in EXPECTED_MEMBERS:
        if f'"{member}"' not in cargo_toml:
            return fail(f"workspace member missing: {member}")
        if not (ROOT / member / "Cargo.toml").is_file():
            return fail(f"member crate missing: {member}")

    migrations = ROOT / "crates/evo-storage/migrations"
    if not migrations.is_dir():
        return fail("migrations directory missing")
    names = sorted(p.name for p in migrations.iterdir() if p.suffix == ".sql")
    numbers: dict[int, list[str]] = {}
    for name in names:
        match = MIGRATION_RE.fullmatch(name)
        if match is None:
            return fail(f"migration filename is not NNNN_name.sql: {name}")
        number = int(match.group(1))
        numbers.setdefault(number, []).append(name)
    duplicates = {number: files for number, files in numbers.items() if len(files) > 1}
    if duplicates:
        return fail(f"duplicate migration numbers: {duplicates}")
    reused = sorted(RESERVED_MIGRATION_NUMBERS.intersection(numbers))
    if reused:
        rendered = ",".join(f"{number:04d}" for number in reused)
        return fail(f"reserved migration number reused: {rendered}")
    for name, expected_sha256 in BASELINE_MIGRATION_SHA256.items():
        path = migrations / name
        if not path.is_file():
            return fail(f"baseline migration missing: {name}")
        actual_sha256 = sha256_file(path)
        if actual_sha256 != expected_sha256:
            return fail(
                f"baseline migration checksum changed: {name} "
                f"expected={expected_sha256} actual={actual_sha256}"
            )

    toolchain = ROOT / "rust-toolchain.toml"
    text = toolchain.read_text(encoding="utf-8")
    if 'channel = "stable"' not in text:
        return fail("rust-toolchain.toml does not pin stable")
    if "rustfmt" not in text or "clippy" not in text:
        return fail("rust-toolchain.toml missing rustfmt/clippy")

    # Increment reconstruction artifacts must not re-enter source.
    delivery = ROOT / ".delivery"
    if delivery.exists():
        return fail(".delivery reconstruction artifacts must not ship")

    nxt = 1
    occupied = set(numbers)
    while nxt in occupied or nxt in RESERVED_MIGRATION_NUMBERS:
        nxt += 1
    print("SMOKE_WORKSPACE_OK")
    print(f"migrations={','.join(names)}")
    print(
        "reserved_migrations="
        + ",".join(f"{number:04d}" for number in sorted(RESERVED_MIGRATION_NUMBERS))
    )
    print(f"next_assignable_migration={nxt:04d}")
    for name in sorted(BASELINE_MIGRATION_SHA256):
        print(f"migration_sha256[{name}]={BASELINE_MIGRATION_SHA256[name]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
