#!/usr/bin/env python3
"""CLI bootstrap smoke. Does not start a service or call a model."""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: smoke_cli.py <rsia-binary>", file=sys.stderr)
        return 2
    binary = Path(argv[1])
    if not binary.is_file():
        print(f"missing binary: {binary}", file=sys.stderr)
        return 1
    proc = subprocess.run(
        [str(binary)],
        check=False,
        capture_output=True,
        text=True,
    )
    combined = (proc.stdout or "") + (proc.stderr or "")
    if proc.returncode != 0:
        print(f"cli exit {proc.returncode}\n{combined}", file=sys.stderr)
        return 1
    needle = "implementation in progress; no service is started"
    if needle not in combined:
        print(f"unexpected bootstrap output:\n{combined}", file=sys.stderr)
        return 1
    print("SMOKE_CLI_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
