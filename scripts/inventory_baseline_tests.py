#!/usr/bin/env python3
"""Enumerate actual baseline tests from source. Does not invent T001–T126."""
from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEST_RE = re.compile(
    r"#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+([A-Za-z0-9_]+)\s*\("
)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    tests = []
    for path in sorted(ROOT.rglob("*.rs")):
        rel = path.relative_to(ROOT).as_posix()
        if rel.startswith("target/") or rel.startswith("archive/"):
            continue
        text = path.read_text(encoding="utf-8")
        file_sha = sha256_file(path)
        for match in TEST_RE.finditer(text):
            name = match.group(1)
            line = text[: match.start()].count("\n") + 1
            tests.append(
                {
                    "path": rel,
                    "name": name,
                    "line": line,
                    "source_sha256": file_sha,
                    "t_number": None,
                    "legacy_equivalence": "unverified",
                    "note": "enumerated from source; T001-T126 assertions were not provided by v3.3, and v4 does not re-fabricate them",
                }
            )
    payload = {
        "plan_version": "v4",
        "count": len(tests),
        "tests": tests,
        "historical_counts_not_reused": {"rust": 90, "python": 15, "old_smoke": 12, "new_smoke": 8},
        "legacy_equivalence_unverified": True,
    }
    json.dump(payload, sys.stdout, ensure_ascii=False, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
