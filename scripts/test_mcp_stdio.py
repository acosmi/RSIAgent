#!/usr/bin/env python3
"""Local, real-process MCP contract tests for an already-built rsia binary.

Uses only the Python standard library and the disabled/reference-host startup
configuration. Pins the observed rmcp 3.3.0 / 2025-06-18 handshake and existing
v1 goldens; does not establish a protocol-version matrix, Claude compatibility,
v2 business negotiation, in-flight cancellation, or model benefit. EOF here is
connection shutdown, not a business cancellation request.

Each unittest owns a fresh directory/database. Only the restart, isolation,
and rejected-frame recovery cases reuse their own database across processes. Frame-error
retries prove that the tested prepare key can still succeed, not that every
database table is unchanged. Pipe I/O uses raw bytes, bounded buffers, and
deadlines; stderr goes to a temporary file so it cannot block the child.

Usage: python3 scripts/test_mcp_stdio.py /absolute/path/to/rsia [--reverse]
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import time
import tomllib
from typing import Any
import unittest

ROOT = Path(__file__).resolve().parents[1]
FRAME_LIMIT = 65536
BUFFER_LIMIT = FRAME_LIMIT * 2
TIMEOUT = 10.0
NAMESPACE = "mcp-stdio-harness"
ACTOR = "stdio-agent"
TOOLS = ("evo_prepare", "evo_feedback", "evo_propose", "evo_inspect")
ADMIN_TOOLS = (
    "experiment.register", "evaluation.start", "evaluation.status",
    "exploration.start", "replay.run", "curriculum.step", "meta.start",
)
STARTUP = "rsia startup: code_execution=disabled sandbox=unavailable recovery=normal"
INITIALIZE = {
    "protocolVersion": "2025-06-18",
    "capabilities": {"tools": {}},
    "serverInfo": {"name": "rsia", "version": "0.2.0"},
    "instructions": (
        "Four bounded RSIA model tools. Identity is fixed by trusted startup configuration."
    ),
}
TYPED_FIELDS = {
    "evo_prepare": "one of `request_key`, `goal`, `capabilities`",
    "evo_feedback": "one of `request_key`, `run_id`, `outcome`, `details`, `failure_class`",
    "evo_propose": (
        "one of `request_key`, `run_id`, `kind`, `parent_snapshot`, `hypothesis`, "
        "`applicability`, `counterexample`, `content`, `evidence_refs`, "
        "`required_capabilities`, `dependencies`"
    ),
    "evo_inspect": "`kind` or `id`",
}


def require(condition: bool, message: str) -> None:
    # Deliberately not a Python assert: checks must survive python -O.
    if not condition:
        raise AssertionError(message)


def compact(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def subject_id(tool: str, key: str, namespace: str, actor: str) -> str:
    """Current service ID contract, used to inspect a rejected key's object."""
    prefix = {"evo_prepare": "run", "evo_feedback": "feedback", "evo_propose": "candidate"}[tool]
    parts = [namespace, actor]
    if tool == "evo_prepare":
        parts.append("prepare")
    parts.append(key)
    return f"{prefix}-{hashlib.sha256(chr(0).join(parts).encode()).hexdigest()[:24]}"


class StdioHost:
    exit_counts: Counter[int] = Counter()

    def __init__(
        self, binary: Path, data: Path, label: str, namespace: str, actor: str,
        delimiter: bytes = b"\n", chunk_size: int | None = None, blank: bool = False,
    ) -> None:
        self.label = label
        self.namespace, self.actor = namespace, actor
        self.delimiter, self.chunk_size, self.blank = delimiter, chunk_size, blank
        self.next_id = 0
        self.buffer = bytearray()
        self.closed = False
        self.stderr = tempfile.TemporaryFile(mode="w+b")
        self.reader: selectors.BaseSelector | None = None
        self.writer: selectors.BaseSelector | None = None
        try:
            self.process = subprocess.Popen(
                [str(binary), "mcp", "--data", str(data),
                 "--namespace", namespace, "--actor", actor],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr, bufsize=0,
            )
            require(self.process.stdin is not None and self.process.stdout is not None, "missing pipes")
            self.reader = selectors.DefaultSelector()
            self.writer = selectors.DefaultSelector()
            os.set_blocking(self.process.stdin.fileno(), False)
            os.set_blocking(self.process.stdout.fileno(), False)
            self.reader.register(self.process.stdout, selectors.EVENT_READ)
            self.writer.register(self.process.stdin, selectors.EVENT_WRITE)
        except BaseException:
            try:
                if hasattr(self, "process"):
                    if self.process.stdin is not None:
                        self.process.stdin.close()
                    self.process.wait(timeout=TIMEOUT)
            except subprocess.TimeoutExpired:
                pass  # The original setup failure remains a failure; dispose reaps the child.
            finally:
                self.dispose()
            raise

    def diagnostic(self) -> str:
        return os.pread(self.stderr.fileno(), BUFFER_LIMIT, 0).decode("utf-8", errors="replace")

    def send_bytes(self, frame: bytes) -> None:
        require(not self.closed, "writing a closed MCP connection")
        deadline = time.monotonic() + TIMEOUT
        offset = 0
        while offset < len(frame):
            remaining = deadline - time.monotonic()
            require(remaining > 0, f"stdin deadline: {self.label}; {self.diagnostic()}")
            require(bool(self.writer.select(remaining)), f"stdin deadline: {self.label}")
            end = min(len(frame), offset + self.chunk_size) if self.chunk_size else len(frame)
            try:
                written = os.write(self.process.stdin.fileno(), memoryview(frame)[offset:end])
            except BlockingIOError:
                continue
            except BrokenPipeError as error:
                raise AssertionError(f"MCP stdin closed: {self.diagnostic()}") from error
            require(written > 0, "zero-byte MCP stdin write")
            offset += written

    def send(self, message: dict[str, Any], content_size: int | None = None) -> None:
        body = compact(message)
        if content_size is not None:
            require(content_size >= len(body), "padding size smaller than JSON")
            body += b" " * (content_size - len(body))
            require(json.loads(body) == message, "padding changed the request")
            print(
                f"MCP_FRAME {self.label} content_bytes={len(body)} "
                f"wire_bytes={len(body) + len(self.delimiter)} delimiter={self.delimiter!r}",
                flush=True,
            )
        frame = body + self.delimiter
        if self.blank:
            frame = b"\n\r\n" + frame + b"\r\n\n"
        self.send_bytes(frame)

    def request_message(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        self.next_id += 1
        return {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}

    def response(self, request_id: int) -> dict[str, Any]:
        deadline = time.monotonic() + TIMEOUT
        while True:
            newline = self.buffer.find(b"\n")
            if newline >= 0:
                line = bytes(self.buffer[:newline])
                del self.buffer[:newline + 1]
                message = json.loads(line)
                require(isinstance(message, dict), f"non-object MCP response: {message!r}")
                require(message.get("jsonrpc") == "2.0", f"wrong JSON-RPC version: {message!r}")
                require(message.get("id") == request_id, f"unexpected response ID: {message!r}")
                require(("result" in message) != ("error" in message), f"wrong RPC layer: {message!r}")
                return message
            remaining = deadline - time.monotonic()
            require(remaining > 0, f"stdout deadline: {self.label}; {self.diagnostic()}")
            require(bool(self.reader.select(remaining)), f"stdout deadline: {self.label}; {self.diagnostic()}")
            try:
                chunk = os.read(self.process.stdout.fileno(), 8192)
            except BlockingIOError:
                continue
            require(bool(chunk), f"EOF before response: {self.label}; {self.diagnostic()}")
            self.buffer.extend(chunk)
            require(len(self.buffer) <= BUFFER_LIMIT, "MCP stdout buffer exceeded bound")

    def rpc(
        self, method: str, params: dict[str, Any], content_size: int | None = None,
    ) -> dict[str, Any]:
        message = self.request_message(method, params)
        self.send(message, content_size)
        return self.response(message["id"])

    def result(
        self, method: str, params: dict[str, Any], content_size: int | None = None,
    ) -> dict[str, Any]:
        response = self.rpc(method, params, content_size)
        require("result" in response, f"unexpected JSON-RPC error: {response!r}")
        require(isinstance(response["result"], dict), f"non-object result: {response!r}")
        return response["result"]

    def handshake(self) -> None:
        observed = self.result("initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "rsia-stdio-contract", "version": "1.0.0"},
        })
        require(observed == INITIALIZE, f"observed handshake drift: {observed!r}")
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def finish(self) -> tuple[int, bytes, str]:
        """First EOF, then bounded drain/wait. Forced cleanup never counts as success."""
        require(not self.closed, "MCP process already finished")
        self.closed = True
        try:
            self.process.stdin.close()
            tail = bytearray(self.buffer)
            self.buffer.clear()
            deadline = time.monotonic() + TIMEOUT
            while True:
                remaining = deadline - time.monotonic()
                require(remaining > 0, f"EOF shutdown deadline: {self.label}; {self.diagnostic()}")
                require(bool(self.reader.select(remaining)), f"EOF drain deadline: {self.label}")
                try:
                    chunk = os.read(self.process.stdout.fileno(), 8192)
                except BlockingIOError:
                    continue
                if not chunk:
                    break
                tail.extend(chunk)
                require(len(tail) <= BUFFER_LIMIT, "MCP EOF stdout exceeded bound")
            try:
                code = self.process.wait(timeout=max(0.001, deadline - time.monotonic()))
            except subprocess.TimeoutExpired as error:
                raise AssertionError(f"process did not exit after EOF: {self.label}") from error
            stderr = self.diagnostic()
            self.exit_counts[code] += 1
            print(f"MCP_PROCESS {self.label} exit={code}" + (f" stderr={stderr!r}" if code else ""), flush=True)
            require(STARTUP in stderr.splitlines(), f"missing disabled startup posture: {stderr!r}")
            return code, bytes(tail), stderr
        finally:
            self.dispose()

    def dispose(self) -> None:
        try:
            if hasattr(self, "process") and self.process.poll() is None:
                print(f"MCP_CLEANUP {self.label} EOF failed; terminating owned process", flush=True)
                self.process.terminate()
                try:
                    self.process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=2)
        finally:
            if hasattr(self, "process"):
                for pipe in (self.process.stdin, self.process.stdout):
                    if pipe is not None:
                        pipe.close()
            for selector in (self.reader, self.writer):
                if selector is not None:
                    selector.close()
            self.stderr.close()

    def close(self) -> None:
        if self.closed:
            return
        code, tail, stderr = self.finish()
        require(code == 0, f"normal EOF exit={code}: {stderr!r}")
        require(tail == b"", f"unexpected stdout after final response: {tail!r}")


class McpStdioTests(unittest.TestCase):
    binary: Path

    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory(prefix="rsia-mcp-stdio-")
        self.addCleanup(self.directory.cleanup)
        self.data = Path(self.directory.name) / "rsia.sqlite3"
        self.spawn_count = 0

    def host(self, namespace: str = NAMESPACE, actor: str = ACTOR, **framing: Any) -> StdioHost:
        self.spawn_count += 1
        host = StdioHost(
            self.binary, self.data, f"{self._testMethodName}:{self.spawn_count}",
            namespace, actor, **framing,
        )
        self.addCleanup(host.close)
        host.handshake()
        return host

    def structured(self, result: dict[str, Any], error: bool = False) -> dict[str, Any]:
        self.assertEqual(set(result), {"content", "structuredContent", "isError"})
        self.assertIs(result["isError"], error)
        value = result["structuredContent"]
        self.assertIsInstance(value, dict)
        self.assertEqual(len(result["content"]), 1)
        self.assertEqual(set(result["content"][0]), {"type", "text"})
        self.assertEqual(result["content"][0]["type"], "text")
        self.assertEqual(json.loads(result["content"][0]["text"]), value)
        return value

    def call(self, host: StdioHost, tool: str, arguments: dict[str, Any], **frame: Any) -> dict[str, Any]:
        return host.result("tools/call", {"name": tool, "arguments": arguments}, **frame)

    def success(self, host: StdioHost, tool: str, arguments: dict[str, Any]) -> dict[str, Any]:
        result = self.call(host, tool, arguments)
        self.structured(result)
        return result

    def business_error(self, host: StdioHost, tool: str, arguments: dict[str, Any], code: str) -> None:
        result = self.call(host, tool, arguments)
        self.assertEqual(self.structured(result, error=True), {"error": {"code": code}})

    def inspect(self, host: StdioHost, kind: str, object_id: str) -> dict[str, Any]:
        return self.structured(self.success(host, "evo_inspect", {"kind": kind, "id": object_id}))

    def absent(self, host: StdioHost, tool: str, key: str) -> None:
        kind = {"evo_prepare": "run", "evo_feedback": "feedback", "evo_propose": "candidate"}[tool]
        self.business_error(host, "evo_inspect", {
            "kind": kind, "id": subject_id(tool, key, host.namespace, host.actor),
        }, "not_found")

    def unchanged(self, host: StdioHost, tool: str, value: dict[str, Any]) -> None:
        if tool == "evo_prepare":
            run = value["run"]
            projection = {key: run[key] for key in (
                "id", "owner", "snapshot_id", "status", "source", "created_at",
            )}
            self.assertEqual(self.inspect(host, "run", run["id"]), dict(projection, kind="run"))
        else:
            kind = "feedback" if tool == "evo_feedback" else "candidate"
            self.assertEqual(self.inspect(host, kind, value["id"]), value)

    def chain(self, host: StdioHost, goal: str = "locate config") -> dict[str, tuple[dict[str, Any], dict[str, Any]]]:
        prepare_args = {"request_key": "prepare", "goal": goal, "capabilities": []}
        prepare = self.success(host, "evo_prepare", prepare_args)
        prepared = self.structured(prepare)
        self.assertEqual(prepared["skills"], [])
        self.assertEqual(prepared["limitations"], [
            "model_transport_disabled", "no_active_release_zero_skill_baseline",
            "tool_only_cannot_claim_attached_or_used",
        ])
        run = prepared["run"]
        self.assertEqual((run["owner"], run["goal"], run["capabilities"]), (host.actor, goal, []))
        self.assertEqual((run["status"], run["source"], run["improver_version"]),
                         ("prepared", "trusted_host_snapshot", "unavailable"))
        feedback_args = {
            "request_key": "feedback", "run_id": run["id"], "outcome": "failure",
            "details": "self report", "failure_class": "reasoning",
        }
        feedback = self.success(host, "evo_feedback", feedback_args)
        recorded = self.structured(feedback)
        self.assertEqual(recorded["request"], feedback_args)
        self.assertEqual((recorded["owner"], recorded["verification"]),
                         (host.actor, "self_reported_unverified"))
        proposal_args = {
            "request_key": "proposal", "run_id": run["id"], "kind": "skill",
            "parent_snapshot": run["snapshot_id"], "hypothesis": "config precedence",
            "applicability": "reference host", "counterexample": "other host",
            "content": "read the higher priority config first", "evidence_refs": [recorded["id"]],
            "required_capabilities": [], "dependencies": [],
        }
        proposal = self.success(host, "evo_propose", proposal_args)
        candidate = self.structured(proposal)
        self.assertEqual(candidate["proposal"], proposal_args)
        self.assertEqual((candidate["owner"], candidate["state"], candidate["improver_version"]),
                         (host.actor, "proposed", "unavailable"))
        self.assertIsNone(candidate["approved_by"])
        self.assertIsNone(candidate["evaluation_id"])
        inspect_args = {"kind": "candidate", "id": candidate["id"]}
        inspected = self.success(host, "evo_inspect", inspect_args)
        self.assertEqual(self.structured(inspected), candidate)
        return {
            "evo_prepare": (prepare_args, prepare), "evo_feedback": (feedback_args, feedback),
            "evo_propose": (proposal_args, proposal), "evo_inspect": (inspect_args, inspected),
        }

    def test_handshake_four_v1_goldens(self) -> None:
        lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
        self.assertEqual([p["version"] for p in lock["package"] if p["name"] == "rmcp"], ["3.3.0"])
        host = self.host()
        print("MCP_OBSERVATION " + compact(INITIALIZE).decode(), flush=True)
        listing = host.result("tools/list", {})
        self.assertEqual(set(listing), {"tools"})
        tools = listing["tools"]
        self.assertEqual(len(tools), 4)
        by_name = {tool["name"]: tool for tool in tools}
        self.assertEqual(set(by_name), set(TOOLS))
        for name in TOOLS:
            golden = json.loads((ROOT / "fixtures/contracts/v1" / f"tool_{name}.json").read_bytes())
            self.assertEqual(by_name[name], golden)  # Enumeration and object-key order are immaterial.
            size = len(compact(by_name[name]))
            self.assertLessEqual(size, 2000)
            print(f"MCP_SCHEMA {name} compact_utf8_bytes={size}", flush=True)
        self.assertTrue(set(ADMIN_TOOLS).isdisjoint(by_name))

    def test_normal_prepare_feedback_propose_inspect(self) -> None:
        self.chain(self.host())

    def reject_field(self, tool: str, field: str) -> None:
        host = self.host()
        chain = self.chain(host)
        original, existing = chain[tool]
        legal = dict(original)
        if tool != "evo_inspect":
            legal["request_key"] = "rejected-key"
        injected = dict(legal, **{field: {"role": "admin", "namespace": "other-space", "actor": "intruder"}.get(field, "unknown")})
        result = self.call(host, tool, injected)
        self.assertEqual(result, {
            "isError": True,
            "content": [{"type": "text", "text": (
                f"failed to deserialize parameters: unknown field `{field}`, expected {TYPED_FIELDS[tool]}"
            )}],
        })
        if tool != "evo_inspect":
            self.absent(host, tool, legal["request_key"])
        else:
            self.assertEqual(self.inspect(host, "candidate", legal["id"]), self.structured(existing))
        first = self.success(host, tool, legal)
        self.assertEqual(self.success(host, tool, legal), first)
        if tool != "evo_inspect":
            self.unchanged(host, tool, self.structured(first))
        else:
            self.assertEqual(first, existing)

    def reject_prepare(self, change: dict[str, Any], code: str) -> None:
        host = self.host()
        legal = {"request_key": "rejected-key", "goal": "clean", "capabilities": []}
        self.business_error(host, "evo_prepare", dict(legal, **change), code)
        self.absent(host, "evo_prepare", legal["request_key"])
        first = self.success(host, "evo_prepare", legal)
        self.assertEqual(self.structured(first)["run"]["goal"], "clean")
        self.assertEqual(self.success(host, "evo_prepare", legal), first)
        self.unchanged(host, "evo_prepare", self.structured(first))

    def test_inspect_missing_is_business_not_found(self) -> None:
        host = self.host()
        chain = self.chain(host)
        self.business_error(host, "evo_inspect", {"kind": "candidate", "id": "missing"}, "not_found")
        args, original = chain["evo_inspect"]
        self.assertEqual(self.success(host, "evo_inspect", args), original)
        self.assertEqual(self.success(host, "evo_inspect", args), original)

    def reject_admin(self, name: str) -> None:
        host = self.host()
        args, original = self.chain(host)["evo_inspect"]
        response = host.rpc("tools/call", {"name": name, "arguments": {}})
        self.assertEqual(set(response), {"jsonrpc", "id", "error"})
        self.assertEqual(response["error"], {"code": -32602, "message": "tool not found"})
        self.assertEqual(self.success(host, "evo_inspect", args), original)

    def restart_operation(self, tool: str) -> None:
        first = self.host()
        chain = self.chain(first)
        first.close()  # A zero EOF exit also releases the data-directory lock.
        reconnected = self.host()
        args, original = chain[tool]
        self.assertEqual(self.success(reconnected, tool, args), original)
        field = {"evo_prepare": "goal", "evo_feedback": "details", "evo_propose": "content"}[tool]
        self.business_error(reconnected, tool, dict(args, **{field: "different payload"}), "conflict")
        self.unchanged(reconnected, tool, self.structured(original))
        self.assertEqual(self.success(reconnected, tool, args), original)

    def isolation(self, dimension: str) -> None:
        first = self.host()
        original = self.chain(first)
        first.close()
        other = self.host(
            namespace="other-space" if dimension == "namespace" else NAMESPACE,
            actor="other-agent" if dimension == "actor" else ACTOR,
        )
        for tool in TOOLS[:3]:
            self.absent(other, tool, original[tool][0]["request_key"])
        for kind, object_id in (
            ("run", self.structured(original["evo_prepare"][1])["run"]["id"]),
            ("feedback", self.structured(original["evo_feedback"][1])["id"]),
            ("candidate", self.structured(original["evo_propose"][1])["id"]),
        ):
            self.business_error(other, "evo_inspect", {"kind": kind, "id": object_id}, "not_found")
        changed = self.chain(other, goal="other identity payload")  # Same three request keys.
        for tool in TOOLS[:3]:
            before, after = self.structured(original[tool][1]), self.structured(changed[tool][1])
            self.assertNotEqual(before["run"]["id"] if tool == "evo_prepare" else before["id"],
                                after["run"]["id"] if tool == "evo_prepare" else after["id"])
            self.assertEqual(self.success(other, tool, changed[tool][0]), changed[tool][1])
        other.close()
        restored = self.host()
        for tool in TOOLS[:3]:
            self.assertEqual(self.success(restored, tool, original[tool][0]), original[tool][1])
            self.unchanged(restored, tool, self.structured(original[tool][1]))

    def valid_frame(self, delimiter: bytes, chunk_size: int | None, blank: bool, size: int | None) -> None:
        host = self.host(delimiter=delimiter, chunk_size=chunk_size, blank=blank)
        legal = {"request_key": "frame-key", "goal": "clean", "capabilities": []}
        first = self.call(host, "evo_prepare", legal, content_size=size)
        self.assertEqual(self.structured(first)["run"]["goal"], "clean")
        self.assertEqual(self.success(host, "evo_prepare", legal), first)
        self.unchanged(host, "evo_prepare", self.structured(first))

    def invalid_frame(self, kind: str) -> None:
        host = self.host()
        legal = {"request_key": "frame-key", "goal": "clean", "capabilities": []}
        message = host.request_message("tools/call", {"name": "evo_prepare", "arguments": dict(legal, goal="poison")})
        body = compact(message)
        if kind in ("overlong_lf", "overlong_crlf"):
            size, ending = (FRAME_LIMIT + 1, b"\n") if kind == "overlong_lf" else (FRAME_LIMIT, b"\r\n")
            padded = body + b" " * (size - len(body))
            self.assertEqual(json.loads(padded), message)
            frame = padded + ending
            self.assertEqual(len(frame), FRAME_LIMIT + 2)
            reason = "MCP JSON frame exceeds 65536 bytes"
        elif kind in ("nested_duplicate", "escaped_duplicate", "utf8_duplicate"):
            if kind == "nested_duplicate":
                frame = body.replace(b'"request_key":"frame-key"', b'"request_key":"frame-key","request_key":"frame-key"') + b"\n"
                duplicate = "request_key"
            elif kind == "escaped_duplicate":
                frame = body.replace(b'"goal":"poison"', b'"goal":"poison","g\\u006fal":"poison-last"') + b"\n"
                duplicate = "goal"
            else:
                frame = body.replace(b'"capabilities":[]', '"capabilities":[],"clé":1,"cl\\u00e9":2'.encode("utf-8")) + b"\n"
                self.assertIn("clé".encode("utf-8"), frame)
                self.assertIn(b"cl\\u00e9", frame)
                duplicate = "clé"
            duplicates = []

            def pairs(items: list[tuple[str, Any]]) -> dict[str, Any]:
                seen: dict[str, Any] = {}
                for key, value in items:
                    if key in seen:
                        duplicates.append(key)
                    seen[key] = value
                return seen

            decoded = json.loads(frame, object_pairs_hook=pairs)
            self.assertEqual(duplicates, [duplicate])
            self.assertEqual(decoded["params"]["arguments"]["request_key"], legal["request_key"])
            reason = f"duplicate JSON key: {duplicate}"
        elif kind == "illegal_json":
            frame = body[:-1] + b",}\n"
            with self.assertRaises(json.JSONDecodeError):
                json.loads(frame)
            reason = "trailing comma"
        elif kind == "eof_half_frame":
            frame = body[:-1]
            self.assertNotIn(b"\n", frame)
            with self.assertRaises(json.JSONDecodeError):
                json.loads(frame)
            reason = "unterminated MCP JSON frame"
        else:
            self.fail(f"unknown frame case: {kind}")
        print(f"MCP_BAD_FRAME {kind} wire_bytes={len(frame)} expected_reason={reason!r}", flush=True)
        host.send_bytes(frame)
        code, tail, stderr = host.finish()
        self.assertEqual(code, 1)
        self.assertEqual(tail, b"")  # This baseline exits, rather than issuing an RPC error.
        self.assertEqual(len(stderr.splitlines()), 2)
        self.assertEqual(stderr.splitlines()[0], STARTUP)
        diagnostic = stderr.splitlines()[1]
        if kind in ("overlong_lf", "overlong_crlf", "eof_half_frame"):
            self.assertEqual(diagnostic, f"Error: {reason}")
        else:
            self.assertTrue(diagnostic.startswith(f"Error: {reason} at line 1 column "), stderr)
        retry = self.host()
        self.absent(retry, "evo_prepare", legal["request_key"])
        first = self.success(retry, "evo_prepare", legal)
        self.assertEqual(self.structured(first)["run"]["goal"], "clean")
        self.assertEqual(self.success(retry, "evo_prepare", legal), first)
        self.unchanged(retry, "evo_prepare", self.structured(first))


def add_case(name: str, method: str, *arguments: Any) -> None:
    def case(self: McpStdioTests) -> None:
        getattr(self, method)(*arguments)
    case.__name__ = name
    setattr(McpStdioTests, name, case)


for tool in TOOLS:
    for field in ("unknown", "actor", "role", "namespace"):
        add_case(f"test_typed_{tool}_{field}", "reject_field", tool, field)
for name, change, code in (
    ("empty_goal", {"goal": ""}, "invalid_input"),
    ("malformed_capability", {"capabilities": ["bad capability"]}, "invalid_input"),
    ("ungranted_capability", {"capabilities": ["admin"]}, "forbidden"),
):
    add_case(f"test_business_prepare_{name}", "reject_prepare", change, code)
for name in ADMIN_TOOLS:
    add_case(f"test_admin_{name.replace('.', '_')}_rpc_not_found", "reject_admin", name)
for tool in TOOLS[:3]:
    add_case(f"test_restart_{tool}_replay_and_conflict", "restart_operation", tool)
for dimension in ("actor", "namespace"):
    add_case(f"test_cache_and_read_isolation_{dimension}", "isolation", dimension)
for name, delimiter, chunk, blank, size in (
    ("lf", b"\n", None, False, None),
    ("crlf", b"\r\n", None, False, None),
    ("blank_lines", b"\n", None, True, None),
    ("chunked", b"\n", 7, False, None),
    ("lf_content_65536_wire_65537", b"\n", None, False, FRAME_LIMIT),
    ("crlf_content_65535_wire_65537", b"\r\n", None, False, FRAME_LIMIT - 1),
):
    add_case(f"test_frame_valid_{name}", "valid_frame", delimiter, chunk, blank, size)
for name in (
    "overlong_lf", "overlong_crlf", "nested_duplicate", "escaped_duplicate",
    "utf8_duplicate", "illegal_json", "eof_half_frame",
):
    add_case(f"test_frame_reject_{name}_restart_clean_key", "invalid_frame", name)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, help="already-built rsia binary")
    parser.add_argument("--reverse", action="store_true", help="run cases in reverse order with fresh databases")
    args = parser.parse_args(argv)
    McpStdioTests.binary = args.binary.resolve(strict=True)
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(McpStdioTests)
    if args.reverse:
        suite = unittest.TestSuite(reversed(list(suite)))
    StdioHost.exit_counts.clear()
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    code = 0 if result.wasSuccessful() else 1
    failed = {test.id() for test, _ in result.failures + result.errors}
    print("MCP_STDIO_SUMMARY " + json.dumps({
        "tests": result.testsRun, "passed": result.testsRun - len(failed) - len(result.skipped),
        "failures": len(result.failures), "errors": len(result.errors), "skipped": len(result.skipped),
        "process_exits": dict(sorted(StdioHost.exit_counts.items())), "exit": code,
        "scope": "local disabled/reference-host; no real Claude or cancellation claim",
    }, sort_keys=True), flush=True)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
