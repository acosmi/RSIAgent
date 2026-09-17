#!/usr/bin/env python3
"""Workspace identity smoke: lockfile, members, migrations, toolchain.

Verifies the tree under test; does not rewrite inputs (V001).
"""
from __future__ import annotations

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


def fail(msg: str) -> int:
    print(f"SMOKE_WORKSPACE_FAIL: {msg}", file=sys.stderr)
    return 1


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
    if "0001_runtime.sql" not in names:
        return fail("0001_runtime.sql missing")
    # Increment package occupied 0003_v3_assets.sql. This tree must not invent
    # a second 0003_exploration_worlds.sql under the same number.
    if "0003_exploration_worlds.sql" in names:
        return fail("0003_exploration_worlds.sql must not reuse the 0003 slot")

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

    numbers = []
    for name in names:
        prefix = name.split("_", 1)[0]
        if prefix.isdigit():
            numbers.append(int(prefix))
    nxt = 1
    while nxt in numbers:
        nxt += 1
    print("SMOKE_WORKSPACE_OK")
    print(f"migrations={','.join(names)}")
    print(f"next_free_migration={nxt:04d}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
