#!/usr/bin/env python3
"""Restore a backup directory only when a revoke watermark is present.

Does not overwrite an existing live data dir. Missing watermark => isolate, do not mount.
"""
from __future__ import annotations

import argparse
import json
import shutil
import sqlite3
import sys
from pathlib import Path


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--backup", required=True)
    p.add_argument("--dest", required=True)
    args = p.parse_args()
    backup = Path(args.backup)
    dest = Path(args.dest)
    db = backup / "rsia.sqlite3"
    if not db.is_file():
        print("missing backup db", file=sys.stderr)
        return 2
    if dest.exists():
        print("dest exists; refusing to overwrite", file=sys.stderr)
        return 2
    con = sqlite3.connect(db)
    try:
        rows = con.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='revoke_watermark'").fetchall()
        if not rows:
            print("ISOLATE missing revoke_watermark; not mounting", file=sys.stderr)
            return 1
        wm = con.execute("SELECT namespace,seq,digest FROM revoke_watermark").fetchall()
        if not wm:
            print("ISOLATE empty revoke watermark; not mounting", file=sys.stderr)
            return 1
    finally:
        con.close()
    dest.mkdir(parents=True)
    shutil.copy2(db, dest / "rsia.sqlite3")
    blobs = backup / "blobs"
    if blobs.exists():
        shutil.copytree(blobs, dest / "blobs")
    (dest / "restore.json").write_text(json.dumps({"watermark_rows": len(wm), "isolated": False}) + "\n")
    print("RESTORE_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
