#!/usr/bin/env python3
"""Offline root-budget facts from one read transaction and one bounded page.

This diagnostic requires local database-file access. A billing scope is a
filter, not an authorization credential. Different invocations have different
snapshots. Known ledger amounts do not attest to an external provider's bill.
Normal SQLite read-only WAL access may use WAL/SHM coordination files.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
import sys

SCHEMA_VERSION = "rsia.budget_report.v1"
MAX_LIMIT = 1000
I64_MAX = (1 << 63) - 1
STATES = ("reserved", "dispatched", "uncertain", "finalized", "released", "cancelled")
STAGES = (
    "reflection", "merge", "ranking", "development_execution",
    "development_scoring", "practice", "consolidation", "guidance",
    "task_execution", "candidate_generation", "formal_evaluation", "curriculum",
    "meta_evaluation", "history_collection", "storage_cpu", "gray_operations",
    "human_review",
)
FINANCIAL_STATUSES = (
    "not_dispatched", "usage_unknown", "requires_reconciliation", "cost_recorded",
)
MIGRATION_SHA384 = {
    1: "0e6345ee05ea56807bcd56fa7c634081cb4e2fa920a2ee18af47603af211e4c05a161b04c1dcd3309d24328ef7dea110",
    2: "34c753159160d311e72e5a8ed046bbdd7a4f506beb5752062ec316ac57505917d91d08f2bd1c36b491ea03765b5d6270",
    4: "5371d1f447143dbad639729f416d0983211c207f350e5ee05fd09b1f88196481f4016022fe596f4f2a39e4526b74b8e1",
    5: "d2114e8c1fa7277dc298d699fd2868eae699152b473325adbe6f233f5c72d8f5a2f546a675d61182bf58f03c1e7cacc8",
    6: "562e6392a78b82d7a1a458f12eeb05ef223c9a8e77dc9de01e78e35b52a4360e7b9bba0a6c1d767a49398fbbd5aa961e",
}

# Affinity, required NOT NULL, exact primary-key position. Extra, unread columns
# are not audited. Generated replacements of these required columns reject.
REQUIRED_SCHEMA = {
    "_sqlx_migrations": {
        "version": ("INTEGER", False, 1),
        "checksum": ("BLOB", True, 0),
        "success": ("BOOLEAN", True, 0),
    },
    "root_budgets": {
        "billing_scope": ("TEXT", False, 1),
        "root_budget_id": ("TEXT", True, 0),
        "authorizing_namespace": ("TEXT", True, 0),
        "currency": ("TEXT", True, 0),
        "pricing_version": ("TEXT", True, 0),
        "per_call_cap_micros": ("INTEGER", True, 0),
        "total_limit_micros": ("INTEGER", True, 0),
        "spent_micros": ("INTEGER", True, 0),
        "reserved_micros": ("INTEGER", True, 0),
        "stopped": ("INTEGER", True, 0),
    },
    "root_budget_namespaces": {
        "billing_scope": ("TEXT", True, 1),
        "namespace": ("TEXT", True, 2),
    },
    "root_budget_dispatch_groups": {
        "billing_scope": ("TEXT", True, 1),
        "dispatch_group_id": ("TEXT", True, 2),
        "owner_namespace": ("TEXT", True, 0),
    },
    "root_budget_calls": {
        "billing_scope": ("TEXT", True, 1),
        "call_id": ("TEXT", True, 2),
        "namespace": ("TEXT", True, 0),
        "dispatch_group_id": ("TEXT", True, 0),
        "stage": ("TEXT", True, 0),
        "state": ("TEXT", True, 0),
        "reserved_micros": ("INTEGER", True, 0),
        "dispatch_id": ("TEXT", False, 0),
        "actual_cost_micros": ("INTEGER", False, 0),
        "actual_currency": ("TEXT", False, 0),
        "actual_pricing_version": ("TEXT", False, 0),
        "execution_provenance": ("TEXT", False, 0),
        "execution_closed": ("INTEGER", True, 0),
    },
}

ERROR_CODES = frozenset({
    "invalid_arguments", "unsafe_db_path", "database_unavailable",
    "schema_mismatch", "migration_mismatch", "integrity_failed", "unknown_scope",
    "invalid_root", "invalid_call", "invalid_relation", "database_read_failed",
})


class ReportError(Exception):
    """A fixed diagnostic code with no path, SQL error or database content."""

    def __init__(self, code: str):
        self.code = code if code in ERROR_CODES else "database_read_failed"
        super().__init__(self.code)


def _identifier(value, code):
    if type(value) is not str or re.fullmatch(r"[A-Za-z0-9_.:\-]{1,128}", value) is None:
        raise ReportError(code)
    return value


def _integer(value, code, minimum=0):
    if type(value) is not int or not minimum <= value <= I64_MAX:
        raise ReportError(code)
    return value


def _boolean(value, code):
    if type(value) is not int or value not in (0, 1):
        raise ReportError(code)
    return bool(value)


def _open_readonly(db):
    # Same path ancestry and ordinary single-link semantics as restore_backup;
    # no stronger claim about concurrent replacement or filesystem permissions.
    try:
        path = Path(os.path.abspath(os.fspath(db)))
        current = Path(path.anchor)
        for part in path.parts[1:]:
            current /= part
            if stat.S_ISLNK(current.lstat().st_mode):
                raise ReportError("unsafe_db_path")
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise ReportError("unsafe_db_path")
    except ReportError:
        raise
    except (OSError, TypeError, ValueError):
        raise ReportError("database_unavailable") from None
    # Do not use immutable: it can conceal committed WAL data.
    return sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=1.0)


def _affinity(declared):
    declared = declared.upper()
    if "INT" in declared:
        return "INTEGER"
    if any(token in declared for token in ("CHAR", "CLOB", "TEXT")):
        return "TEXT"
    if not declared or "BLOB" in declared:
        return "BLOB"
    if any(token in declared for token in ("REAL", "FLOA", "DOUB")):
        return "REAL"
    return "NUMERIC"


def _verify_schema(con):
    for table, required in REQUIRED_SCHEMA.items():
        kinds = con.execute(
            "SELECT type FROM sqlite_schema WHERE name=? COLLATE BINARY LIMIT 2",
            (table,),
        ).fetchall()
        if [tuple(row) for row in kinds] != [("table",)]:
            raise ReportError("schema_mismatch")
        # table is a fixed internal constant, never a CLI/user identifier.
        columns = con.execute(f'PRAGMA table_xinfo("{table}")').fetchall()
        actual = {row[1]: row for row in columns}
        actual_pk = {row[1]: row[5] for row in columns if row[5]}
        required_pk = {name: rule[2] for name, rule in required.items() if rule[2]}
        if actual_pk != required_pk:
            raise ReportError("schema_mismatch")
        for name, (affinity, not_null, pk) in required.items():
            column = actual.get(name)
            if column is None:
                raise ReportError("schema_mismatch")
            type_ok = (
                column[2].upper() in ("BOOLEAN", "INTEGER")
                if affinity == "BOOLEAN" else _affinity(column[2]) == affinity
            )
            if not type_ok or (not_null and column[3] != 1) or column[5] != pk or column[6] != 0:
                raise ReportError("schema_mismatch")


def _verify_migrations(con):
    rows = con.execute(
        "SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version LIMIT 6"
    ).fetchall()
    if len(rows) != len(MIGRATION_SHA384):
        raise ReportError("migration_mismatch")
    seen = set()
    for version, checksum, success in rows:
        if (
            type(version) is not int or version in seen or version not in MIGRATION_SHA384
            or type(success) is not int or success != 1
            or type(checksum) is not bytes or checksum.hex() != MIGRATION_SHA384[version]
        ):
            raise ReportError("migration_mismatch")
        seen.add(version)
    if seen != set(MIGRATION_SHA384):
        raise ReportError("migration_mismatch")


def _verify_integrity(con):
    rows = con.execute("PRAGMA integrity_check").fetchmany(2)
    if [tuple(row) for row in rows] != [("ok",)]:
        raise ReportError("integrity_failed")


def _read_root(con, scope):
    rows = con.execute(
        """SELECT r.billing_scope,r.root_budget_id,r.authorizing_namespace,
                  r.currency,r.pricing_version,r.per_call_cap_micros,
                  r.total_limit_micros,r.spent_micros,r.reserved_micros,r.stopped,
                  EXISTS(SELECT 1 FROM root_budget_namespaces n
                         WHERE n.billing_scope COLLATE BINARY=r.billing_scope
                           AND n.namespace COLLATE BINARY=r.authorizing_namespace)
                    AS authorizing_member
           FROM root_budgets r WHERE r.billing_scope COLLATE BINARY=? LIMIT 2""",
        (scope,),
    ).fetchall()
    if not rows:
        raise ReportError("unknown_scope")
    if len(rows) != 1:
        raise ReportError("invalid_root")
    row = rows[0]
    for field in ("billing_scope", "root_budget_id", "authorizing_namespace", "currency", "pricing_version"):
        _identifier(row[field], "invalid_root")
    if row["billing_scope"] != scope:
        raise ReportError("invalid_root")
    cap = _integer(row["per_call_cap_micros"], "invalid_root", 1)
    total = _integer(row["total_limit_micros"], "invalid_root", 1)
    if cap > total:
        raise ReportError("invalid_root")
    for field in ("spent_micros", "reserved_micros"):
        _integer(row[field], "invalid_root")
    stopped = _boolean(row["stopped"], "invalid_root")
    if not _boolean(row["authorizing_member"], "invalid_relation"):
        raise ReportError("invalid_relation")
    result = {field: row[field] for field in ("billing_scope", "root_budget_id", "currency", "pricing_version")}
    result.update({field: str(row[field]) for field in (
        "per_call_cap_micros", "total_limit_micros", "spent_micros", "reserved_micros",
    )})
    result["stopped"] = stopped
    return result


def _read_calls(con, scope, after, limit):
    # Relations are projected, not JOIN/WHERE filters: a corrupt returned row
    # must reject the page instead of disappearing from it.
    return con.execute(
        """SELECT c.call_id,c.namespace,c.dispatch_group_id,c.stage,c.state,
                  c.reserved_micros,c.dispatch_id,c.actual_cost_micros,
                  c.actual_currency,c.actual_pricing_version,
                  c.execution_provenance,c.execution_closed,
                  EXISTS(SELECT 1 FROM root_budget_namespaces n
                         WHERE n.billing_scope COLLATE BINARY=c.billing_scope
                           AND n.namespace COLLATE BINARY=c.namespace) AS namespace_member,
                  EXISTS(SELECT 1 FROM root_budget_dispatch_groups g
                         WHERE g.billing_scope COLLATE BINARY=c.billing_scope
                           AND g.dispatch_group_id COLLATE BINARY=c.dispatch_group_id
                           AND g.owner_namespace COLLATE BINARY=c.namespace) AS owned_group
           FROM root_budget_calls c
           WHERE c.billing_scope COLLATE BINARY=?
             AND (? IS NULL OR c.call_id COLLATE BINARY>?)
           ORDER BY c.call_id COLLATE BINARY LIMIT ?""",
        (scope, after, after, limit + 1),
    ).fetchall()


def _financial_status(state, actual_cost):
    if state in ("reserved", "released", "cancelled"):
        return "not_dispatched"
    if state == "dispatched":
        return "usage_unknown"
    if state == "uncertain":
        return "requires_reconciliation"
    return "cost_recorded"


def _project_call(row, root):
    for field in ("call_id", "namespace", "dispatch_group_id"):
        _identifier(row[field], "invalid_call")
    if type(row["stage"]) is not str or row["stage"] not in STAGES:
        raise ReportError("invalid_call")
    state = row["state"]
    if type(state) is not str or state not in STATES:
        raise ReportError("invalid_call")
    reserved = _integer(row["reserved_micros"], "invalid_call", 1)
    dispatch = row["dispatch_id"]
    if dispatch is not None:
        _identifier(dispatch, "invalid_call")
    actual_cost = row["actual_cost_micros"]
    actual_currency = row["actual_currency"]
    actual_pricing = row["actual_pricing_version"]
    actual = (actual_cost, actual_currency, actual_pricing)
    all_null = all(value is None for value in actual)
    all_known = all(value is not None for value in actual)
    if not all_null and not all_known:
        raise ReportError("invalid_call")
    if all_known:
        _integer(actual_cost, "invalid_call")
        _identifier(actual_currency, "invalid_call")
        _identifier(actual_pricing, "invalid_call")
    closed = _boolean(row["execution_closed"], "invalid_call")
    provenance = row["execution_provenance"]
    if provenance is not None and (type(provenance) is not str or provenance not in ("fixture", "external_provider")):
        raise ReportError("invalid_call")
    if state in ("reserved", "released", "cancelled"):
        if dispatch is not None or not all_null or not closed:
            raise ReportError("invalid_call")
    elif dispatch is None:
        raise ReportError("invalid_call")
    if state == "dispatched" and not all_null:
        raise ReportError("invalid_call")
    if state == "finalized" and (
        not all_known or actual_currency != root["currency"]
        or actual_pricing != root["pricing_version"]
    ):
        raise ReportError("invalid_call")
    for field in ("namespace_member", "owned_group"):
        if not _boolean(row[field], "invalid_relation"):
            raise ReportError("invalid_relation")
    return {
        "call_id": row["call_id"], "namespace": row["namespace"],
        "dispatch_group_id": row["dispatch_group_id"], "stage": row["stage"],
        "state": state, "reserved_micros": str(reserved), "dispatch_id": dispatch,
        "actual_cost_micros": None if actual_cost is None else str(actual_cost),
        "actual_currency": actual_currency, "actual_pricing_version": actual_pricing,
        "execution_provenance": provenance, "execution_closed": closed,
        "financial_status": _financial_status(state, actual_cost),
        "execution_open": not closed, "unknown_fee_amount": None,
    }


def _page_summary(calls):
    summary = {
        "state_counts": {state: 0 for state in STATES},
        "financial_status_counts": {status: 0 for status in FINANCIAL_STATUSES},
        "dispatched_count": 0, "execution_open_count": 0,
        "known_but_unreconciled_count": 0,
    }
    finalized = 0
    active_reserved = 0
    for call in calls:
        summary["state_counts"][call["state"]] += 1
        summary["financial_status_counts"][call["financial_status"]] += 1
        summary["dispatched_count"] += int(call["dispatch_id"] is not None)
        summary["execution_open_count"] += int(call["execution_open"])
        if call["state"] == "uncertain" and call["actual_cost_micros"] is not None:
            summary["known_but_unreconciled_count"] += 1
        if call["state"] == "finalized":
            finalized += int(call["actual_cost_micros"])
        if call["state"] in ("reserved", "dispatched", "uncertain"):
            active_reserved += int(call["reserved_micros"])
    summary["page_finalized_cost_micros"] = str(finalized)
    summary["page_active_reserved_micros"] = str(active_reserved)
    return summary


def _read_snapshot(db, billing_scope, after_call_id, limit):
    con = None
    try:
        con = _open_readonly(db)
        con.row_factory = sqlite3.Row
        con.execute("PRAGMA query_only=ON")
        con.execute("BEGIN")
        _verify_schema(con)
        _verify_migrations(con)
        _verify_integrity(con)
        root = _read_root(con, billing_scope)
        rows = _read_calls(con, billing_scope, after_call_id, limit)
        has_more = len(rows) > limit
        calls = [_project_call(row, root) for row in rows[:limit]]
        return {
            "schema_version": SCHEMA_VERSION, "diagnostic_only": True,
            "snapshot_scope": "single_invocation",
            "scope_kind": "all_namespaces_in_billing_scope",
            "validation_scope": "root_and_returned_page",
            "migration_versions": list(MIGRATION_SHA384),
            "migration_checksums_verified": True, "integrity_check": "ok",
            "root": root, "calls": calls,
            "page": {
                "limit": limit, "after_call_id": after_call_id,
                "returned_count": len(calls), "has_more": has_more,
                "next_after_call_id": calls[-1]["call_id"] if has_more else None,
                "complete_scope": after_call_id is None and not has_more,
            },
            "page_summary": _page_summary(calls),
        }
    finally:
        if con is not None:
            try:
                if con.in_transaction:
                    con.rollback()
            finally:
                con.close()


def read_budget_report(db, billing_scope, *, after_call_id=None, limit=100):
    """Return root facts and a page; reject a bad returned row as a whole page.

    Migration/integrity and root/returned-row validation share one transaction.
    Unreturned rows are not validated; page sums never replace root amounts.
    """
    _identifier(billing_scope, "invalid_arguments")
    if after_call_id is not None:
        _identifier(after_call_id, "invalid_arguments")
    if type(limit) is not int or not 1 <= limit <= MAX_LIMIT:
        raise ReportError("invalid_arguments")
    try:
        return _read_snapshot(db, billing_scope, after_call_id, limit)
    except ReportError:
        raise
    except Exception:
        # Includes connection, read, rollback and close failures. Nothing is
        # serialized by the CLI until this complete invocation has returned.
        raise ReportError("database_read_failed") from None


class _Parser(argparse.ArgumentParser):
    def error(self, message):
        raise ReportError("invalid_arguments")


def main(argv=None):
    parser = _Parser(description=__doc__)
    parser.add_argument("--db", required=True, help="existing local SQLite database")
    parser.add_argument("--billing-scope", required=True, help="one complete billing scope")
    parser.add_argument("--after-call-id", help="exclusive BINARY call_id cursor")
    parser.add_argument("--limit", type=int, default=100, help="page size 1..1000 (default 100)")
    try:
        args = parser.parse_args(argv)
        report = read_budget_report(
            args.db, args.billing_scope, after_call_id=args.after_call_id, limit=args.limit,
        )
        encoded = json.dumps(report, ensure_ascii=True, allow_nan=False, separators=(",", ":"))
    except ReportError as error:
        sys.stderr.write("BUDGET_REPORT_ERROR " + error.code + "\n")
        return 2
    except Exception:
        sys.stderr.write("BUDGET_REPORT_ERROR database_read_failed\n")
        return 2
    sys.stdout.write(encoded + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
