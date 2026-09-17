#!/usr/bin/env python3
"""Run both E00 smoke groups: workspace identity, then CLI bootstrap."""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def run(script: Path, args: list[str]) -> None:
    cmd = [sys.executable, str(script), *args]
    proc = subprocess.run(cmd, check=False)
    if proc.returncode != 0:
        raise SystemExit(proc.returncode)


def main(argv: list[str]) -> int:
    run(HERE / "smoke_workspace.py", [])
    if len(argv) >= 2:
        run(HERE / "smoke_cli.py", [argv[1]])
    else:
        print("SMOKE_CLI_SKIPPED: no binary argument", file=sys.stderr)
        return 1
    print("SMOKE_BOTH_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
