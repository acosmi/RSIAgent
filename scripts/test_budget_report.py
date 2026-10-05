"""Structural SQLite fixtures and adversarial tests, not production bill writers.

The separate AG078 evidence generator exercises existing public Rust writers.
These tests execute the frozen migrations and intentionally break metadata.
"""

from contextlib import redirect_stderr, redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

from scripts import budget_report as report

ROOT = Path(__file__).resolve().parents[1]
SECRET = "SECRET-AG078-structural-fixture"
UNSET = object()


def without_checks(sql):
    """Intentionally damage CHECK constraints while retaining table types/PKs."""
    while "CHECK(" in sql or "CHECK (" in sql:
        start = sql.index("CHECK")
        opening = sql.index("(", start)
        depth, quoted, index = 1, False, opening + 1
        while depth:
            char = sql[index]
            if char == "'":
                if quoted and sql[index + 1:index + 2] == "'":
                    index += 2
                    continue
                quoted = not quoted
            elif not quoted:
                depth += (char == "(") - (char == ")")
            index += 1
        end = index
        # Standalone table checks also own their separator; inline column
        # checks leave the column separator in place.
        line_start = sql.rfind("\n", 0, start) + 1
        if not sql[line_start:start].strip():
            previous = sql[:line_start].rstrip()
            if previous.endswith(","):
                line_start = len(previous) - 1
            start = line_start
        sql = sql[:start] + sql[end:]
    return sql


class StructuralFixture:
    def __init__(self, path, overrides=None):
        self.path = path
        self.con = sqlite3.connect(path)
        for source in sorted((ROOT / "crates/evo-storage/migrations").glob("*.sql")):
            sql = source.read_text()
            if overrides and source.name in overrides:
                sql = overrides[source.name](sql)
            self.con.executescript(sql)
        self.con.execute("""CREATE TABLE _sqlx_migrations (
            version BIGINT PRIMARY KEY, description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL, checksum BLOB NOT NULL,
            execution_time BIGINT NOT NULL)""")
        for source in sorted((ROOT / "crates/evo-storage/migrations").glob("*.sql")):
            self.con.execute(
                "INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(?,?,?,?,?)",
                (int(source.name.split("_", 1)[0]), SECRET, 1, hashlib.sha384(source.read_bytes()).digest(), 1),
            )
        self.add_root("scope-a")

    def add_root(self, scope, **changes):
        values = {
            "billing_scope": scope, "root_budget_id": "root-" + scope,
            "authorizing_namespace": "ns-a", "currency": "USD",
            "pricing_version": "price-v1", "payment_subject": SECRET,
            "authorization_receipt_digest": "a" * 64,
            "per_call_cap_micros": 100, "total_limit_micros": 1000,
            "spent_micros": 7, "reserved_micros": 55, "stopped": 0,
            "created_at": 1,
        }
        values.update(changes)
        if values["stopped"] == 1:
            values.update(stop_reason=SECRET, stop_committed_at=2)
        self.con.execute(
            "INSERT INTO root_budgets(" + ",".join(values) + ") VALUES(" + ",".join("?" for _ in values) + ")",
            tuple(values.values()),
        )
        for namespace in ("ns-a", "ns-b"):
            self.con.execute("INSERT INTO root_budget_namespaces VALUES(?,?)", (scope, namespace))

    def add_call(self, call_id, *, scope="scope-a", namespace="ns-a", state="reserved",
                 stage="reflection", reserved=10, cost=UNSET, currency=UNSET,
                 pricing=UNSET, closed=UNSET, dispatch=UNSET, provenance=None):
        dispatched = state in ("dispatched", "uncertain", "finalized")
        if dispatch is UNSET:
            dispatch = "dispatch-" + scope + "-" + call_id if dispatched else None
        if closed is UNSET:
            closed = 0 if dispatched else 1
        if cost is UNSET:
            cost = 7 if state == "finalized" else None
        if currency is UNSET:
            currency = "USD" if cost is not None else None
        if pricing is UNSET:
            pricing = "price-v1" if cost is not None else None
        group = "group-" + call_id
        self.con.execute(
            "INSERT INTO root_budget_dispatch_groups(billing_scope,dispatch_group_id,owner_namespace,created_at) VALUES(?,?,?,1)",
            (scope, group, namespace),
        )
        values = {
            "billing_scope": scope, "call_id": call_id, "namespace": namespace,
            "dispatch_group_id": group, "stage": stage, "actual_input_digest": "b" * 64,
            "reserved_micros": reserved, "state": state, "lease_token": SECRET,
            "lease_epoch": 1, "lease_until": 1000, "dispatch_id": dispatch,
            "actual_cost_micros": cost, "actual_currency": currency,
            "actual_pricing_version": pricing, "execution_provenance": provenance,
            "execution_closed": closed, "created_at": 2,
            "request_artifact_schema": "unknown.structural.request.v999",
            "request_artifact_digest": "c" * 64,
            "request_artifact_body": json.dumps({"request": SECRET}),
            "transport_artifact_schema": "unknown.structural.transport.v999",
            "transport_artifact_digest": "d" * 64,
            "transport_artifact_body": json.dumps({"transport": SECRET}),
            "response_artifact_schema": "unknown.structural.response.v999",
            "response_artifact_digest": "e" * 64,
            "response_artifact_body": json.dumps({"response": SECRET}),
            "response_usable": 0, "response_block_reason": SECRET,
        }
        if dispatch is not None:
            values["dispatched_at"] = 3
            if closed:
                values.update(execution_closed_at=4, execution_close_reason=SECRET)
        if state == "finalized":
            values["finalized_at"] = 4
        self.con.execute(
            "INSERT INTO root_budget_calls(" + ",".join(values) + ") VALUES(" + ",".join("?" for _ in values) + ")",
            tuple(values.values()),
        )
        self.con.execute(
            "INSERT INTO root_budget_events(billing_scope,call_id,event_kind,event_at,details) VALUES(?,?,?,3,?)",
            (scope, call_id, "fixture_event", json.dumps({"reason": SECRET})),
        )

    def close(self):
        self.con.close()


class BudgetReportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ag078-", dir=os.environ.get("TMPDIR"))
        self.folder = Path(self.temp.name).resolve()
        self.db = StructuralFixture(self.folder / "rsia.sqlite3")

    def tearDown(self):
        self.db.close()
        self.temp.cleanup()

    def read(self, **kwargs):
        self.db.con.commit()
        return report.read_budget_report(self.db.path, kwargs.pop("scope", "scope-a"), **kwargs)

    def unchecked_budget(self):
        self.db.close()
        self.db = StructuralFixture(self.folder / "unchecked.sqlite3", {"0005_root_budget.sql": without_checks})

    def error(self, code, **kwargs):
        with self.assertRaises(report.ReportError) as caught:
            self.read(**kwargs)
        self.assertEqual(caught.exception.code, code)
        self.assertEqual(str(caught.exception), code)

    def cli(self, *arguments):
        self.db.con.commit()
        return subprocess.run(
            [sys.executable, str(ROOT / "scripts/budget_report.py"), "--db", str(self.db.path), *arguments],
            cwd=ROOT, text=True, capture_output=True,
        )

    def captured_main(self, *arguments):
        output, error = io.StringIO(), io.StringIO()
        with redirect_stdout(output), redirect_stderr(error):
            code = report.main(["--db", str(self.db.path), "--billing-scope", "scope-a", *arguments])
        return code, output.getvalue(), error.getvalue()

    def test_frozen_migrations_match_actual_source_bytes(self):
        expected = {
            int(path.name.split("_", 1)[0]): hashlib.sha384(path.read_bytes()).hexdigest()
            for path in (ROOT / "crates/evo-storage/migrations").glob("*.sql")
        }
        self.assertEqual(list(sorted(expected)), [1, 2, 4, 5, 6])
        self.assertEqual(report.MIGRATION_SHA384, expected)

    def test_six_states_keep_unknown_and_exact_page_classification(self):
        expected = {
            "reserved": "not_dispatched", "released": "not_dispatched",
            "cancelled": "not_dispatched", "dispatched": "usage_unknown",
            "uncertain": "requires_reconciliation", "finalized": "cost_recorded",
        }
        for index, state in enumerate(expected):
            self.db.add_call(f"c{index}", state=state, namespace="ns-b" if index % 2 else "ns-a")
        result = self.read()
        self.assertEqual({row["state"]: row["financial_status"] for row in result["calls"]}, expected)
        self.assertEqual(result["page_summary"]["state_counts"], {state: 1 for state in expected})
        self.assertEqual(result["page_summary"]["financial_status_counts"], {
            "not_dispatched": 3, "usage_unknown": 1, "requires_reconciliation": 1, "cost_recorded": 1,
        })
        self.assertEqual(result["page_summary"]["dispatched_count"], 3)
        self.assertEqual(result["page_summary"]["execution_open_count"], 3)
        self.assertEqual(result["page_summary"]["page_finalized_cost_micros"], "7")
        self.assertEqual(result["page_summary"]["page_active_reserved_micros"], "30")
        self.assertEqual(result["root"]["reserved_micros"], "55")
        self.assertTrue(result["page"]["complete_scope"])
        self.assertEqual({row["namespace"] for row in result["calls"]}, {"ns-a", "ns-b"})
        self.assertTrue(all(row["unknown_fee_amount"] is None for row in result["calls"]))
        self.assertEqual(result["schema_version"], "rsia.budget_report.v1")
        self.assertTrue(result["diagnostic_only"])
        self.assertEqual(result["snapshot_scope"], "single_invocation")
        self.assertEqual(result["scope_kind"], "all_namespaces_in_billing_scope")
        self.assertEqual(result["validation_scope"], "root_and_returned_page")
        self.assertEqual(result["integrity_check"], "ok")
        for forbidden in ("can_dispatch", "can_use", "reconciled_all", "benefit"):
            self.assertNotIn(forbidden, result)

    def test_all_seventeen_current_stages_are_included(self):
        stages = (
            "reflection", "merge", "ranking", "development_execution", "development_scoring",
            "practice", "consolidation", "guidance", "task_execution", "candidate_generation",
            "formal_evaluation", "curriculum", "meta_evaluation", "history_collection",
            "storage_cpu", "gray_operations", "human_review",
        )
        for index, stage in enumerate(stages):
            self.db.add_call(f"c{index:02}", stage=stage)
        self.assertEqual([row["stage"] for row in self.read()["calls"]], list(stages))
        self.assertEqual(len(stages), 17)

    def test_zero_is_known_and_null_is_not_zero(self):
        self.db.add_call("a-zero", state="finalized", cost=0)
        self.db.add_call("b-unknown", state="dispatched")
        calls = self.read()["calls"]
        self.assertEqual(calls[0]["actual_cost_micros"], "0")
        self.assertIsNone(calls[1]["actual_cost_micros"])
        self.assertIsNone(calls[1]["actual_currency"])
        self.assertIsNone(calls[1]["actual_pricing_version"])

    def test_uncertain_nonempty_mismatched_actual_is_still_unreconciled(self):
        self.db.add_call("mismatch", state="uncertain", cost=9, currency="EUR", pricing="price-other", provenance="fixture", closed=1)
        result = self.read()
        call = result["calls"][0]
        self.assertEqual(call["actual_cost_micros"], "9")
        self.assertEqual(call["actual_currency"], "EUR")
        self.assertEqual(call["actual_pricing_version"], "price-other")
        self.assertEqual(call["financial_status"], "requires_reconciliation")
        self.assertEqual(call["execution_provenance"], "fixture")
        self.assertFalse(call["execution_open"])
        self.assertEqual(result["page_summary"]["known_but_unreconciled_count"], 1)
        self.assertEqual(result["page_summary"]["financial_status_counts"]["cost_recorded"], 0)
        self.assertEqual(result["page_summary"]["page_finalized_cost_micros"], "0")

    def test_overflow_shaped_uncertain_retains_known_amount_and_original_root(self):
        self.db.add_call("overflow", state="uncertain", cost=4)
        self.db.con.execute("UPDATE root_budgets SET spent_micros=?,stopped=1,stop_reason=?,stop_committed_at=4", (report.I64_MAX - 2, SECRET))
        result = self.read()
        self.assertEqual(result["root"]["spent_micros"], str(report.I64_MAX - 2))
        self.assertTrue(result["root"]["stopped"])
        self.assertEqual(result["calls"][0]["actual_cost_micros"], "4")
        self.assertEqual(result["calls"][0]["financial_status"], "requires_reconciliation")
        self.assertEqual(result["page_summary"]["page_finalized_cost_micros"], "0")

    def test_execution_closed_and_financial_finality_are_independent(self):
        self.db.add_call("final-open", state="finalized", closed=0)
        self.db.add_call("unknown-closed", state="uncertain", closed=1)
        calls = self.read()["calls"]
        self.assertEqual((calls[0]["financial_status"], calls[0]["execution_open"]), ("cost_recorded", True))
        self.assertEqual((calls[1]["financial_status"], calls[1]["execution_open"]), ("requires_reconciliation", False))

    def test_terminal_rows_keep_historical_positive_reserved_amount(self):
        for state in ("released", "cancelled", "finalized"):
            self.db.add_call(state, state=state, reserved=83)
        result = self.read()
        self.assertEqual([call["reserved_micros"] for call in result["calls"]], ["83"] * 3)
        self.assertEqual(result["page_summary"]["page_active_reserved_micros"], "0")

    def test_exact_i64_values_and_page_sums_beyond_i64_do_not_use_float(self):
        for call_id in ("a", "b"):
            self.db.add_call(call_id, state="finalized", cost=report.I64_MAX, reserved=report.I64_MAX)
        for call_id in ("c", "d"):
            self.db.add_call(call_id, reserved=report.I64_MAX)
        result = self.read()
        self.assertEqual(result["page_summary"]["page_finalized_cost_micros"], str(report.I64_MAX * 2))
        self.assertEqual(result["page_summary"]["page_active_reserved_micros"], str(report.I64_MAX * 2))
        self.assertEqual(result["calls"][0]["actual_cost_micros"], str(report.I64_MAX))
        self.assertEqual(result["root"]["spent_micros"], "7")

    def test_root_legal_actual_spent_exceeds_total_without_clamping(self):
        self.db.con.execute("UPDATE root_budgets SET spent_micros=1001,stopped=1,stop_reason=?,stop_committed_at=4", (SECRET,))
        self.assertEqual(self.read()["root"]["spent_micros"], "1001")

    def test_fixture_external_and_null_provenance_are_not_promoted(self):
        for index, provenance in enumerate(("fixture", "external_provider", None)):
            self.db.add_call(f"c{index}", state="finalized", provenance=provenance)
        self.assertEqual([call["execution_provenance"] for call in self.read()["calls"]], ["fixture", "external_provider", None])

    def test_unknown_artifact_schema_and_secret_bodies_are_not_read_or_output(self):
        self.db.add_call("body", state="uncertain")
        self.db.con.execute("INSERT INTO objects(namespace,kind,id,owner,body) VALUES('ns-a','artifact','unknown','owner',?)", (json.dumps({"id": "unknown", "schema_version": "unknown.v999", "body": SECRET}),))
        output = json.dumps(self.read())
        self.assertNotIn(SECRET, output)
        for field in ("lease_token", "payment_subject", "authorizing_namespace", "request_artifact", "transport_artifact", "response_artifact", "reason"):
            self.assertNotIn('"' + field, output)

    def test_valid_limit_one_and_thousand_and_default_hundred(self):
        for index in range(1001):
            self.db.add_call(f"c{index:04}")
        result = self.read()
        self.assertEqual(result["page"]["limit"], 100)
        self.assertEqual(result["page"]["returned_count"], 100)
        self.assertTrue(result["page"]["has_more"])
        self.assertEqual(self.read(limit=1)["page"]["returned_count"], 1)
        last = self.read(limit=1000)
        self.assertEqual(last["page"]["returned_count"], 1000)
        self.assertFalse(last["page"]["complete_scope"])
        self.assertEqual(last["page_summary"]["page_active_reserved_micros"], "10000")

    def test_invalid_limit_types_bounds_and_cursor_are_fixed_argument_errors(self):
        for limit in (0, 1001, -1, True, 1.0, "1", None):
            with self.subTest(limit=limit):
                self.error("invalid_arguments", limit=limit)
        for cursor in ("", "x' OR 1=1--", "é", "x" * 129, 5, False):
            with self.subTest(cursor=cursor):
                self.error("invalid_arguments", after_call_id=cursor)

    def test_identifiers_allow_all_current_ascii_prefixes_and_long_currency(self):
        for call_id in ("-leading", ".dot", ":colon", "_underscore"):
            self.db.add_call(call_id)
        self.db.con.execute("UPDATE root_budgets SET currency=?,pricing_version=?", ("-" + "x" * 127, ":version"))
        result = self.read()
        self.assertEqual([row["call_id"] for row in result["calls"]], sorted(["-leading", ".dot", ":colon", "_underscore"]))
        self.assertEqual(len(result["root"]["currency"]), 128)

    def test_keyset_binary_order_cursor_and_no_offset_or_count(self):
        ids = ["a", "A", "z", "Z", "_a", ":a", "-a"]
        for call_id in ids:
            self.db.add_call(call_id)
        actual = []
        cursor = None
        pages = []
        while True:
            result = self.read(after_call_id=cursor, limit=2)
            pages.append(result)
            actual.extend(row["call_id"] for row in result["calls"])
            if not result["page"]["has_more"]:
                break
            cursor = result["page"]["next_after_call_id"]
        self.assertEqual(actual, sorted(ids, key=lambda value: value.encode("ascii")))
        self.assertTrue(all(not page["page"]["complete_scope"] for page in pages))
        self.assertIsNone(pages[-1]["page"]["next_after_call_id"])

    def test_truncated_page_counts_only_returned_rows_and_preserves_root(self):
        self.db.add_call("a", state="finalized", cost=3)
        self.db.add_call("b", state="uncertain", cost=99)
        first = self.read(limit=1)
        self.assertEqual(first["page"]["next_after_call_id"], "a")
        self.assertFalse(first["page"]["complete_scope"])
        self.assertEqual(first["page_summary"]["state_counts"]["uncertain"], 0)
        self.assertEqual(first["page_summary"]["known_but_unreconciled_count"], 0)
        self.assertEqual(first["page_summary"]["page_finalized_cost_micros"], "3")
        self.assertEqual(first["root"]["spent_micros"], "7")
        second = self.read(limit=1, after_call_id="a")
        self.assertFalse(second["page"]["has_more"])
        self.assertFalse(second["page"]["complete_scope"])
        self.assertEqual(second["root"], first["root"])

    def test_bad_lookahead_is_not_returned_or_claimed_validated(self):
        self.db.add_call("a")
        self.db.add_call("b")
        self.db.con.execute("UPDATE root_budget_calls SET stage=? WHERE call_id='b'", (SECRET,))
        result = self.read(limit=1)
        self.assertTrue(result["page"]["has_more"])
        self.assertEqual(result["page"]["next_after_call_id"], "a")
        self.assertEqual(len(result["calls"]), 1)
        self.error("invalid_call", limit=1, after_call_id="a")

    def test_empty_scope_and_empty_cursor_tail_are_distinct(self):
        empty = self.read()
        self.assertEqual(empty["calls"], [])
        self.assertTrue(empty["page"]["complete_scope"])
        self.assertEqual(empty["root"]["reserved_micros"], "55")
        self.db.add_call("a")
        tail = self.read(after_call_id="z")
        self.assertEqual(tail["calls"], [])
        self.assertFalse(tail["page"]["has_more"])
        self.assertFalse(tail["page"]["complete_scope"])
        self.assertIsNone(tail["page"]["next_after_call_id"])
        self.error("invalid_arguments", after_call_id="")

    def test_scope_filter_does_not_leak_another_scope_or_offer_namespace_filter(self):
        self.db.add_root("scope-b", spent_micros=901)
        self.db.add_call("same", namespace="ns-b")
        self.db.add_call("same", scope="scope-b", state="finalized", cost=901)
        result = self.read()
        self.assertEqual(result["root"]["spent_micros"], "7")
        self.assertEqual(result["calls"][0]["namespace"], "ns-b")
        self.assertIsNone(result["calls"][0]["actual_cost_micros"])
        rejected = self.cli("--billing-scope", "scope-a", "--namespace", "ns-b")
        self.assertEqual(rejected.returncode, 2)
        self.assertEqual(rejected.stdout, "")
        self.assertEqual(rejected.stderr, "BUDGET_REPORT_ERROR invalid_arguments\n")

    def test_unknown_scope_and_injection_do_not_fall_back(self):
        self.error("unknown_scope", scope="missing")
        for scope in ("scope-a' OR 1=1--", "", "é", "x" * 129, None):
            self.error("invalid_arguments", scope=scope)

    def test_wrong_group_owner_is_not_filtered_from_page(self):
        self.db.add_call("bad")
        self.db.con.execute("UPDATE root_budget_dispatch_groups SET owner_namespace='ns-b'")
        self.error("invalid_relation")

    def test_missing_namespace_membership_is_not_filtered(self):
        self.db.add_call("bad", namespace="ns-b")
        self.db.con.execute("DELETE FROM root_budget_namespaces WHERE namespace='ns-b'")
        self.error("invalid_relation")

    def test_group_in_another_scope_cannot_be_borrowed_by_same_id(self):
        self.db.add_root("scope-b")
        self.db.add_call("same")
        self.db.add_call("same", scope="scope-b")
        self.db.con.execute("DELETE FROM root_budget_dispatch_groups WHERE billing_scope='scope-a'")
        self.error("invalid_relation")

    def test_authorizing_namespace_must_remain_member(self):
        self.db.con.execute("DELETE FROM root_budget_namespaces WHERE namespace='ns-a'")
        self.error("invalid_relation")

    def test_root_rejects_real_text_negative_zero_and_bad_bool(self):
        self.unchecked_budget()
        columns = {
            "per_call_cap_micros": (0, -1, 1.5, SECRET),
            "total_limit_micros": (0, -1, 1.5, SECRET),
            "spent_micros": (-1, 1.5, SECRET),
            "reserved_micros": (-1, 1.5, SECRET),
            "stopped": (2, -1, 0.5, SECRET),
            "currency": ("", "é", "x" * 129, SECRET + "!"),
            "pricing_version": ("", "é"),
            "root_budget_id": ("", "é"),
        }
        for column, values in columns.items():
            original = self.db.con.execute("SELECT " + column + " FROM root_budgets").fetchone()[0]
            for value in values:
                with self.subTest(column=column, value=value):
                    self.db.con.execute("PRAGMA ignore_check_constraints=ON")
                    self.db.con.execute("UPDATE root_budgets SET " + column + "=?", (value,))
                    self.error("invalid_root")
                    self.db.con.execute("UPDATE root_budgets SET " + column + "=?", (original,))
        self.db.con.execute("UPDATE root_budgets SET per_call_cap_micros=1001")
        self.error("invalid_root")

    def test_call_rejects_bad_amount_bool_state_stage_and_identity(self):
        self.unchecked_budget()
        self.db.add_call("bad", state="uncertain")
        columns = {
            "reserved_micros": (0, -1, 1.5, SECRET),
            "actual_cost_micros": (-1, 0.5, SECRET),
            "execution_closed": (2, -1, 0.5, SECRET),
            "stage": ("", "future_stage", SECRET),
            "state": ("", "usage_uncertain", SECRET),
            "namespace": ("", "é"),
            "dispatch_id": ("", "é", "x" * 129),
            "execution_provenance": ("trusted_provider", SECRET),
        }
        for column, values in columns.items():
            original = self.db.con.execute("SELECT " + column + " FROM root_budget_calls").fetchone()[0]
            for value in values:
                with self.subTest(column=column, value=value):
                    self.db.con.execute("PRAGMA ignore_check_constraints=ON")
                    self.db.con.execute("UPDATE root_budget_calls SET " + column + "=?", (value,))
                    self.error("invalid_call")
                    self.db.con.execute("UPDATE root_budget_calls SET " + column + "=?", (original,))

    def test_i64_overflow_encoded_as_real_or_text_is_not_an_integer_amount(self):
        self.db.add_call("overflow", state="uncertain", cost=7)
        for value in (float(1 << 63), str(1 << 63)):
            self.db.con.execute("UPDATE root_budget_calls SET actual_cost_micros=?", (value,))
            self.error("invalid_call")

    def test_state_invariants_and_partial_actual_triplets_reject_whole_page(self):
        self.unchecked_budget()
        self.db.add_call("bad", state="uncertain", cost=0)
        for field in ("actual_currency", "actual_pricing_version", "actual_cost_micros"):
            original = self.db.con.execute("SELECT " + field + " FROM root_budget_calls").fetchone()[0]
            self.db.con.execute("UPDATE root_budget_calls SET " + field + "=NULL")
            self.error("invalid_call")
            self.db.con.execute("UPDATE root_budget_calls SET " + field + "=?", (original,))
        self.db.con.execute("PRAGMA ignore_check_constraints=ON")
        self.db.con.execute("UPDATE root_budget_calls SET state='dispatched'")
        self.error("invalid_call")
        self.db.con.execute("UPDATE root_budget_calls SET state='uncertain',dispatch_id=NULL")
        self.error("invalid_call")

    def test_not_dispatched_requires_no_dispatch_no_actual_and_closed(self):
        self.unchecked_budget()
        self.db.add_call("bad")
        self.db.con.execute("PRAGMA ignore_check_constraints=ON")
        for state in ("reserved", "released", "cancelled"):
            self.db.con.execute("UPDATE root_budget_calls SET state=?,execution_closed=0", (state,))
            self.error("invalid_call")
            self.db.con.execute("UPDATE root_budget_calls SET execution_closed=1,dispatch_id='dispatch-bad'")
            self.error("invalid_call")
            self.db.con.execute("UPDATE root_budget_calls SET dispatch_id=NULL,actual_cost_micros=0,actual_currency='USD',actual_pricing_version='price-v1'")
            self.error("invalid_call")
            self.db.con.execute("UPDATE root_budget_calls SET actual_cost_micros=NULL,actual_currency=NULL,actual_pricing_version=NULL")

    def test_finalized_requires_actual_and_matching_root_price_identity(self):
        self.db.add_call("finalized", state="finalized", cost=0)
        for column, bad in (("actual_currency", "EUR"), ("actual_pricing_version", "price-other")):
            original = self.db.con.execute("SELECT " + column + " FROM root_budget_calls").fetchone()[0]
            self.db.con.execute("UPDATE root_budget_calls SET " + column + "=?", (bad,))
            self.error("invalid_call")
            self.db.con.execute("UPDATE root_budget_calls SET " + column + "=?", (original,))
        self.db.con.execute("UPDATE root_budget_calls SET actual_cost_micros=NULL,actual_currency=NULL,actual_pricing_version=NULL")
        self.error("invalid_call")

    def test_missing_extra_failed_or_wrong_checksum_migration_rejects(self):
        for operation in ("DELETE FROM _sqlx_migrations WHERE version=6", "UPDATE _sqlx_migrations SET success=0 WHERE version=6", "UPDATE _sqlx_migrations SET checksum=x'00' WHERE version=6", "UPDATE _sqlx_migrations SET checksum='SECRET' WHERE version=6"):
            self.db.con.execute("SAVEPOINT bad_migration")
            self.db.con.execute(operation)
            self.db.con.commit()
            self.error("migration_mismatch")
            self.db.con.execute("DELETE FROM _sqlx_migrations WHERE version=6")
            self.db.con.execute("INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(6,?,1,?,1)", (SECRET, bytes.fromhex(report.MIGRATION_SHA384[6])))
        self.db.con.execute("INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(7,?,1,x'00',1)", (SECRET,))
        self.error("migration_mismatch")

    def test_migration_actual_types_are_checked(self):
        for column, value in (("version", 6.5), ("success", 0.5), ("success", SECRET)):
            original = self.db.con.execute("SELECT " + column + " FROM _sqlx_migrations WHERE version=6").fetchone()[0]
            self.db.con.execute("UPDATE _sqlx_migrations SET " + column + "=? WHERE version=6", (value,))
            self.error("migration_mismatch")
            if column == "version":
                self.db.con.execute("UPDATE _sqlx_migrations SET version=6 WHERE version=6.5")
            else:
                self.db.con.execute("UPDATE _sqlx_migrations SET " + column + "=? WHERE version=6", (original,))

    def test_duplicate_migration_with_missing_pk_is_rejected(self):
        self.db.con.execute("ALTER TABLE _sqlx_migrations RENAME TO old_migrations")
        self.db.con.execute("CREATE TABLE _sqlx_migrations(version BIGINT,checksum BLOB NOT NULL,success BOOLEAN NOT NULL)")
        self.db.con.execute("INSERT INTO _sqlx_migrations SELECT version,checksum,success FROM old_migrations")
        self.db.con.execute("INSERT INTO _sqlx_migrations SELECT version,checksum,success FROM old_migrations WHERE version=6")
        self.error("schema_mismatch")

    def test_bad_table_column_type_pk_and_nullable_required_field_reject(self):
        cases = (
            ("reserved_micros INTEGER NOT NULL", "reserved_micros REAL NOT NULL"),
            ("reserved_micros INTEGER NOT NULL", "reserved_micros INTEGER"),
            ("PRIMARY KEY(billing_scope, call_id)", "UNIQUE(billing_scope, call_id)"),
            ("stage TEXT NOT NULL", "stage BLOB NOT NULL"),
            ("execution_closed INTEGER NOT NULL", "execution_closed REAL NOT NULL"),
        )
        for index, (old, new) in enumerate(cases):
            with self.subTest(case=index):
                fixture = StructuralFixture(self.folder / f"bad-{index}.sqlite3", {"0005_root_budget.sql": lambda sql, old=old, new=new: sql.replace(old, new)})
                try:
                    fixture.con.commit()
                    with self.assertRaises(report.ReportError) as caught:
                        report.read_budget_report(fixture.path, "scope-a")
                    self.assertEqual(caught.exception.code, "schema_mismatch")
                finally:
                    fixture.close()

    def test_missing_table_or_column_and_view_replacements_reject(self):
        self.db.con.execute("ALTER TABLE root_budget_calls RENAME COLUMN stage TO removed_stage")
        self.error("schema_mismatch")
        self.db.con.execute("ALTER TABLE root_budget_calls RENAME TO hidden_calls")
        self.db.con.execute("CREATE VIEW root_budget_calls AS SELECT * FROM hidden_calls")
        self.error("schema_mismatch")
        self.db.con.execute("DROP VIEW root_budget_calls")
        self.error("schema_mismatch")

    def test_integrity_result_must_be_unique_ok_and_errors_are_fixed(self):
        self.db.con.commit()
        real_connect = sqlite3.connect
        class IntegrityConnection(sqlite3.Connection):
            def execute(self, sql, parameters=()):
                if sql == "PRAGMA integrity_check":
                    return mock.Mock(fetchmany=lambda count: [("ok",), (SECRET,)])
                return super().execute(sql, parameters)
        with mock.patch.object(report.sqlite3, "connect", side_effect=lambda *args, **kwargs: real_connect(*args, factory=IntegrityConnection, **kwargs)):
            self.error("integrity_failed")

    def test_corrupt_database_and_missing_database_do_not_create_or_leak(self):
        missing = self.folder / "missing-parent" / "missing.sqlite3"
        with self.assertRaises(report.ReportError) as caught:
            report.read_budget_report(missing, "scope-a")
        self.assertEqual(caught.exception.code, "database_unavailable")
        self.assertFalse(missing.parent.exists())
        bad = self.folder / "corrupt.sqlite3"
        bad.write_bytes(SECRET.encode())
        with self.assertRaises(report.ReportError) as caught:
            report.read_budget_report(bad, "scope-a")
        self.assertEqual(caught.exception.code, "database_read_failed")
        self.assertNotIn(SECRET, str(caught.exception))

    def test_symlink_ancestor_final_symlink_hardlink_and_directory_reject(self):
        link = self.folder / "linked.sqlite3"
        link.symlink_to(self.db.path)
        directory_link = self.folder / "linked-directory"
        directory_link.symlink_to(self.folder, target_is_directory=True)
        for path in (link, directory_link / self.db.path.name, self.folder):
            with self.assertRaises(report.ReportError) as caught:
                report.read_budget_report(path, "scope-a")
            self.assertEqual(caught.exception.code, "unsafe_db_path")
        hardlink = self.folder / "hardlink.sqlite3"
        os.link(self.db.path, hardlink)
        try:
            with self.assertRaises(report.ReportError) as caught:
                report.read_budget_report(self.db.path, "scope-a")
            self.assertEqual(caught.exception.code, "unsafe_db_path")
        finally:
            hardlink.unlink()

    def test_uri_escapes_spaces_hash_question_percent_and_unicode_path(self):
        self.db.con.commit()
        unusual = self.folder / "name #? % é.sqlite3"
        backup = sqlite3.connect(unusual)
        try:
            self.db.con.backup(backup)
        finally:
            backup.close()
        self.assertEqual(report.read_budget_report(unusual, "scope-a")["root"]["billing_scope"], "scope-a")

    def test_authorizer_trace_and_business_snapshots_prove_no_body_reads_or_writes(self):
        self.db.add_call("body", state="uncertain")
        self.db.con.commit()
        tables = ("root_budgets", "root_budget_namespaces", "root_budget_dispatch_groups", "root_budget_calls", "root_budget_events", "objects")
        before = {table: self.db.con.execute("SELECT * FROM " + table).fetchall() for table in tables}
        allowed = {
            "sqlite_master": {"name", "type"},
            "_sqlx_migrations": {"version", "checksum", "success"},
            "root_budgets": {"billing_scope", "root_budget_id", "authorizing_namespace",
                             "currency", "pricing_version", "per_call_cap_micros",
                             "total_limit_micros", "spent_micros", "reserved_micros", "stopped"},
            "root_budget_namespaces": {"billing_scope", "namespace"},
            "root_budget_dispatch_groups": {"billing_scope", "dispatch_group_id", "owner_namespace"},
            "root_budget_calls": {"billing_scope", "call_id", "namespace", "dispatch_group_id",
                                  "stage", "state", "reserved_micros", "dispatch_id",
                                  "actual_cost_micros", "actual_currency", "actual_pricing_version",
                                  "execution_provenance", "execution_closed"},
        }
        write_actions = {sqlite3.SQLITE_INSERT, sqlite3.SQLITE_UPDATE, sqlite3.SQLITE_DELETE,
                         sqlite3.SQLITE_CREATE_TABLE, sqlite3.SQLITE_DROP_TABLE,
                         sqlite3.SQLITE_ALTER_TABLE, sqlite3.SQLITE_ATTACH, sqlite3.SQLITE_DETACH}
        reads, integrity_reads, writes, trace, connections = [], [], [], [], []
        integrity_phase = {"active": False}
        def authorizer(action, first, second, database, trigger):
            if action in write_actions:
                writes.append((action, first, second))
                return sqlite3.SQLITE_DENY
            if action == sqlite3.SQLITE_READ:
                if integrity_phase["active"]:
                    # The formal contract explicitly exempts internal reads
                    # of SQLite's integrity_check (including FTS metadata).
                    integrity_reads.append((first, second))
                    return sqlite3.SQLITE_OK
                reads.append((first, second))
                if first not in allowed or second not in allowed[first]:
                    return sqlite3.SQLITE_DENY
            return sqlite3.SQLITE_OK
        real_connect = sqlite3.connect
        class AuditedConnection(sqlite3.Connection):
            def execute(self, sql, parameters=()):
                # Keep the flag through fetch; reset before every subsequent
                # application statement. No other statement gets an exemption.
                integrity_phase["active"] = sql == "PRAGMA integrity_check"
                return super().execute(sql, parameters)
        def connect(*args, **kwargs):
            self.assertTrue(kwargs["uri"])
            self.assertTrue(args[0].endswith("?mode=ro"))
            self.assertNotIn("immutable", args[0])
            connection = real_connect(*args, factory=AuditedConnection, **kwargs)
            connection.set_trace_callback(trace.append)
            connection.set_authorizer(authorizer)
            connections.append(connection)
            return connection
        with mock.patch.object(report.sqlite3, "connect", side_effect=connect):
            result = report.read_budget_report(self.db.path, "scope-a")
        after = {table: self.db.con.execute("SELECT * FROM " + table).fetchall() for table in tables}
        self.assertEqual(before, after)
        self.assertEqual(writes, [])
        self.assertEqual(len(connections), 1)
        self.assertEqual(trace.count("BEGIN"), 1)
        self.assertEqual(trace[-1], "ROLLBACK")
        self.assertTrue(reads)
        self.assertNotIn(SECRET, json.dumps(result))
        page_sql = [sql for sql in trace if "FROM root_budget_calls c" in sql]
        self.assertEqual(len(page_sql), 1)
        self.assertIn("LIMIT 101", page_sql[0])
        self.assertNotIn("OFFSET", page_sql[0].upper())
        self.assertNotIn("COUNT(", page_sql[0].upper())
        self.assertNotIn("SELECT *", page_sql[0].upper())

    def test_real_committed_uncheckpointed_wal_is_visible(self):
        self.db.con.commit()
        self.db.con.execute("PRAGMA journal_mode=WAL")
        self.db.con.execute("PRAGMA wal_autocheckpoint=0")
        self.db.con.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        self.db.add_call("wal-new", state="finalized", cost=29)
        self.db.con.execute("UPDATE root_budgets SET spent_micros=29")
        self.db.con.commit()
        wal = Path(str(self.db.path) + "-wal")
        self.assertGreater(wal.stat().st_size, 0)
        result = report.read_budget_report(self.db.path, "scope-a")
        self.assertEqual(result["root"]["spent_micros"], "29")
        self.assertEqual(result["calls"][0]["call_id"], "wal-new")
        self.assertEqual(result["calls"][0]["actual_cost_micros"], "29")

    def test_one_read_transaction_keeps_root_and_page_consistent_across_writer_commit(self):
        self.db.add_call("a-old", state="finalized", cost=1)
        self.db.con.execute("UPDATE root_budgets SET spent_micros=1")
        self.db.con.commit()
        self.db.con.execute("PRAGMA journal_mode=WAL")
        root_read, writer_committed = threading.Event(), threading.Event()
        failures = []
        def writer():
            connection = sqlite3.connect(self.db.path)
            try:
                if not root_read.wait(10):
                    raise AssertionError("reader did not reach root/page barrier")
                connection.execute("UPDATE root_budgets SET spent_micros=9")
                connection.execute("UPDATE root_budget_calls SET actual_cost_micros=9")
                connection.commit()
                writer_committed.set()
            except Exception as error:
                failures.append(error)
                writer_committed.set()
            finally:
                connection.close()
        original_read = report._read_calls
        def synchronized_page(con, scope, after, limit):
            self.assertTrue(con.in_transaction)
            root_read.set()
            self.assertTrue(writer_committed.wait(10), "writer did not commit at barrier")
            self.assertEqual(failures, [])
            return original_read(con, scope, after, limit)
        thread = threading.Thread(target=writer)
        thread.start()
        try:
            with mock.patch.object(report, "_read_calls", side_effect=synchronized_page):
                old_snapshot = report.read_budget_report(self.db.path, "scope-a")
        finally:
            root_read.set()
            thread.join(10)
        self.assertFalse(thread.is_alive())
        self.assertEqual(failures, [])
        self.assertEqual(old_snapshot["root"]["spent_micros"], "1")
        self.assertEqual(old_snapshot["calls"][0]["actual_cost_micros"], "1")
        new_snapshot = report.read_budget_report(self.db.path, "scope-a")
        self.assertEqual(new_snapshot["root"]["spent_micros"], "9")
        self.assertEqual(new_snapshot["calls"][0]["actual_cost_micros"], "9")
        self.assertEqual(new_snapshot["snapshot_scope"], "single_invocation")

    def test_exclusive_lock_is_fixed_error_without_partial_json(self):
        self.db.con.commit()
        self.db.con.execute("BEGIN EXCLUSIVE")
        try:
            code, output, error = self.captured_main()
        finally:
            self.db.con.rollback()
        self.assertEqual((code, output, error), (2, "", "BUDGET_REPORT_ERROR database_read_failed\n"))

    def test_late_read_and_close_failures_never_serialize_half_success_or_sql_secret(self):
        self.db.con.commit()
        with mock.patch.object(report, "_read_calls", side_effect=sqlite3.OperationalError("SQL " + SECRET)):
            self.assertEqual(self.captured_main(), (2, "", "BUDGET_REPORT_ERROR database_read_failed\n"))
        real_connect = sqlite3.connect
        class CloseFailure(sqlite3.Connection):
            def close(self):
                super().close()
                raise sqlite3.OperationalError(SECRET)
        with mock.patch.object(report.sqlite3, "connect", side_effect=lambda *args, **kwargs: real_connect(*args, factory=CloseFailure, **kwargs)):
            self.assertEqual(self.captured_main(), (2, "", "BUDGET_REPORT_ERROR database_read_failed\n"))

    def test_cli_success_error_and_cursor_pages_are_complete_single_documents(self):
        self.db.add_call("a")
        self.db.add_call("b", namespace="ns-b", state="uncertain")
        first = self.cli("--billing-scope", "scope-a", "--limit", "1")
        self.assertEqual((first.returncode, first.stderr), (0, ""))
        result = json.loads(first.stdout)
        self.assertEqual(result["page"]["next_after_call_id"], "a")
        second = self.cli("--billing-scope", "scope-a", "--limit", "1", "--after-call-id", "a")
        self.assertEqual((second.returncode, second.stderr), (0, ""))
        self.assertEqual(json.loads(second.stdout)["calls"][0]["call_id"], "b")
        self.assertNotIn(SECRET, first.stdout + first.stderr + second.stdout + second.stderr)
        for arguments, expected in (
            (("--billing-scope", "missing"), "unknown_scope"),
            (("--billing-scope", "scope-a", "--limit", "0"), "invalid_arguments"),
            (("--billing-scope", "scope-a", "--limit", "1001"), "invalid_arguments"),
            (("--billing-scope", "scope-a", "--limit", SECRET), "invalid_arguments"),
            (("--billing-scope", "scope-a", "--after-call-id", ""), "invalid_arguments"),
            (("--billing-scope", "scope-a' OR 1=1--"), "invalid_arguments"),
        ):
            failed = self.cli(*arguments)
            self.assertEqual(failed.returncode, 2)
            self.assertEqual(failed.stdout, "")
            self.assertEqual(failed.stderr, "BUDGET_REPORT_ERROR " + expected + "\n")
        self.db.con.execute("UPDATE root_budget_calls SET stage=? WHERE call_id='b'", (SECRET,))
        failure = self.cli("--billing-scope", "scope-a")
        self.assertEqual((failure.returncode, failure.stdout, failure.stderr), (2, "", "BUDGET_REPORT_ERROR invalid_call\n"))


if __name__ == "__main__":
    unittest.main()
