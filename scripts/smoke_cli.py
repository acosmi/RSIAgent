#!/usr/bin/env python3
"""Real process smoke for CLI exit behavior, HTTP auth, management wiring, and MCP stdio.

The smoke uses the disabled model transport and a zero-skill baseline. It does
not call a provider and does not claim model quality or benefit. The management
checks only prove that the CLI, HTTP surface, and dispatcher agree on the
current request contracts and terminal states; no replay ever succeeds here
because no replay pool is registered.
"""
from __future__ import annotations

import hashlib
import json
import os
import selectors
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import traceback
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

AGENT_TOKEN = "smoke-agent-token-0123456789"
HOST_TOKEN = "smoke-host-token-01234567890"
ADMIN_TOKEN = "smoke-admin-token-0123456789"

REPLAY_RUN_SCHEMA = "rsia.management.replay_run.v1"
# Digest of a replay pool that is never registered. This is the same value the
# engine test `replay_run_missing_pool_fails_cleanly` builds with
# `d("non-existent-pool")` (evo_core::hash is SHA-256 hex).
MISSING_POOL_DIGEST = hashlib.sha256(b"non-existent-pool").hexdigest()
TERMINAL_STATES = {"blocked", "failed", "cancelled", "succeeded"}
# E16.5 startup posture (stderr only; stdout is the MCP JSON-RPC channel).
STARTUP_NORMAL = (
    "rsia startup: code_execution=disabled sandbox=unavailable recovery=normal"
)
# No CLI switch can enable code execution, a sandbox or external network.
FORBIDDEN_SWITCHES = [
    "--allow-code-execution",
    "--sandbox",
    "--code-execution",
    "--enable-code-execution",
    "--allow-external-network",
]


def fail(message: str) -> None:
    raise RuntimeError(message)


def replay_run_request(request_key: str, pool_digest: str) -> dict[str, Any]:
    """Complete `ReplayRunRequest` (crates/evo-engine/src/dispatch.rs).

    Field values mirror `sample_profile`, `ElasticPolicyV1::default()` and
    `ExplorationCapsV1::online()` as used by the management tests in
    crates/evo-engine/tests/dispatch_management.rs. Every nested struct is
    `deny_unknown_fields`, so the shape must match exactly.
    """
    return {
        "schema_version": REPLAY_RUN_SCHEMA,
        "request_key": request_key,
        "pool_digest": pool_digest,
        "partition": "select",
        "policy": {
            "schema_version": "rsia.elastic_priority.v1",
            "significant_gain_micros": 20000,
            "stagnation_abs_gain_micros": 5000,
            "stagnation_window": 2,
            "max_focus_actions": 2,
            "fairness_wait_rounds": 4,
        },
        "profile": {
            "simulation_version": "rsia.unit_probe_barrier.v1",
            "objective": "pareto_attainment_v2",
            "w_sim": 1,
            "probe_budget": 2,
            "horizon": 2,
            "lambda_work_micros": 50000,
            "lambda_round_micros": 50000,
            "fixed_seed": 9,
            "global_recovery_dispatch_limit": 1,
            "pool_digest": pool_digest,
            "purpose": "development",
            "target_runtime_profile": "simulation-only",
        },
        "caps": {
            "schema_version": "rsia.exploration_caps.v1",
            "w_online": 1,
            "max_nodes": 12,
            "max_depth": 4,
            "max_repair_dispatches_per_episode": 1,
        },
    }


def cli_manage(
    binary: Path, base: str, operation: str, *extra: str
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(binary), "manage", operation, "--url", base,
         "--auth-token", ADMIN_TOKEN, *extra],
        capture_output=True, text=True, timeout=5,
    )


def cli_submit(
    binary: Path, base: str, root: Path, name: str, operation: str, payload: dict[str, Any]
) -> subprocess.CompletedProcess[str]:
    request_file = root / f"{name}.json"
    request_file.write_text(json.dumps(payload))
    return cli_manage(binary, base, operation, "--request-file", str(request_file))


def cli_wait_terminal(binary: Path, base: str, job_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + 10
    while True:
        status = cli_manage(binary, base, "job.status", "--job-id", job_id)
        if status.returncode != 0:
            fail(f"CLI management status failed: {status.stderr}")
        job = json.loads(status.stdout)
        if job.get("state") in TERMINAL_STATES:
            return job
        if time.monotonic() >= deadline:
            fail(f"CLI management job did not reach a terminal state: {job}")
        time.sleep(0.02)


def free_port() -> int:
    # The managed test sandbox can forbid Python socket creation while still
    # allowing the Rust child process to bind loopback. Use a high per-process
    # port; failure remains visible through the child's early exit.
    return 30000 + (os.getpid() % 20000)


def http_post(
    base: str,
    path: str,
    body: dict[str, Any],
    token: str | None,
    extra_headers: dict[str, str] | None = None,
) -> tuple[int, dict[str, Any]]:
    headers = {"content-type": "application/json"}
    if token:
        headers["authorization"] = f"Bearer {token}"
    headers.update(extra_headers or {})
    request = urllib.request.Request(
        f"{base}{path}",
        data=json.dumps(body, separators=(",", ":")).encode(),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=3) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read())


def http_post_raw(
    base: str, path: str, body: str, token: str
) -> tuple[int, dict[str, Any]]:
    request = urllib.request.Request(
        f"{base}{path}",
        data=body.encode(),
        headers={
            "content-type": "application/json",
            "authorization": f"Bearer {token}",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=3) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read())


def wait_http(base: str) -> None:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            status, _ = http_post(
                base,
                "/v1/tools/prepare",
                {"request_key": "probe", "goal": "probe", "capabilities": []},
                None,
            )
            if status == 401:
                return
        except (OSError, urllib.error.URLError):
            time.sleep(0.05)
    fail("HTTP process did not become ready")


def smoke_http(binary: Path, root: Path) -> None:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    data = root / "http.sqlite3"
    process = subprocess.Popen(
        [
            str(binary),
            "serve",
            "--bind",
            f"127.0.0.1:{port}",
            "--data",
            str(data),
            "--namespace",
            "smoke",
            "--actor",
            "agent-a",
            "--auth-token",
            AGENT_TOKEN,
            "--host-token",
            HOST_TOKEN,
            "--admin-token",
            ADMIN_TOKEN,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    serve_stderr = ""
    try:
        wait_http(base)
        second = subprocess.run(
            [
                str(binary),
                "serve",
                "--bind",
                f"127.0.0.1:{port + 1}",
                "--data",
                str(data),
                "--auth-token",
                AGENT_TOKEN,
            ],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if second.returncode == 0 or "already locked" not in second.stderr:
            fail("second HTTP process acquired the same data directory")
        second_mcp = subprocess.run(
            [str(binary), "mcp", "--data", str(data)],
            input="",
            capture_output=True,
            text=True,
            timeout=5,
        )
        if second_mcp.returncode == 0 or "already locked" not in second_mcp.stderr:
            fail("MCP process acquired the HTTP process data directory")
        # Blocked feature: meta.start has no consumer yet (curriculum.step is
        # wired to the persistent curriculum consumer since AG-015). The
        # dispatcher must persist an accurate `blocked` terminal instead of
        # failing or hanging (see
        # future_operations_persist_accurate_blocked_jobs_and_reconnect).
        submitted = cli_submit(
            binary, base, root, "blocked-meta", "meta.start",
            {
                "schema_version": "rsia.management.meta_start.v1",
                "request_key": "cli-blocked-1",
            },
        )
        if submitted.returncode != 0:
            fail(f"CLI blocked-feature submit failed: {submitted.stderr}")
        blocked_job = json.loads(submitted.stdout)
        blocked = cli_wait_terminal(binary, base, blocked_job["id"])
        if blocked.get("state") != "blocked" or blocked.get("step") != "blocked_feature":
            fail(f"CLI management did not preserve blocked terminal: {blocked}")
        if blocked.get("error_code") != "meta.start_consumer_unavailable":
            fail(f"CLI blocked terminal carried the wrong error_code: {blocked}")
        if blocked.get("id") != blocked_job["id"]:
            fail("CLI management status did not return the persisted blocked job")

        # Real replay.run wiring: a complete, schema-valid request whose pool
        # was never registered is accepted at submit and fails cleanly inside
        # the job (see replay_run_missing_pool_fails_cleanly). This exercises
        # the CLI -> HTTP -> dispatcher -> consumer path without a model call.
        replay_payload = replay_run_request("cli-replay-missing-pool", MISSING_POOL_DIGEST)
        submitted = cli_submit(
            binary, base, root, "replay-missing-pool", "replay.run", replay_payload
        )
        if submitted.returncode != 0:
            fail(f"CLI replay.run submit failed: {submitted.stderr}")
        replay_job = json.loads(submitted.stdout)
        if replay_job.get("state") != "queued" or replay_job.get("operation") != "replay.run":
            fail(f"CLI replay.run submit did not queue the job: {replay_job}")
        replay = cli_wait_terminal(binary, base, replay_job["id"])
        if replay.get("state") != "failed" or replay.get("step") != "failed":
            fail(f"CLI replay.run with a missing pool did not fail cleanly: {replay}")
        if replay.get("error_code") != "not_found":
            fail(f"CLI replay.run missing-pool error_code was wrong: {replay}")
        if replay.get("id") != replay_job["id"]:
            fail("CLI management status did not return the persisted replay job")
        resubmitted = cli_submit(
            binary, base, root, "replay-missing-pool-again", "replay.run", replay_payload
        )
        if resubmitted.returncode != 0:
            fail(f"CLI replay.run resubmit failed: {resubmitted.stderr}")
        if json.loads(resubmitted.stdout).get("id") != replay_job["id"]:
            fail(f"CLI replay.run resubmit was not idempotent: {resubmitted.stdout}")

        # Submit-time rejection: the same payload with one unknown field must
        # be refused before any job exists. The HTTP surface answers 400
        # invalid_input; the CLI echoes that body and exits non-zero.
        unknown_payload = dict(replay_payload, extra_field="unexpected")
        rejected = cli_submit(
            binary, base, root, "replay-unknown-field", "replay.run", unknown_payload
        )
        if rejected.returncode == 0 or "HTTP 400" not in rejected.stderr:
            fail(
                "CLI replay.run unknown field was not rejected: "
                f"exit={rejected.returncode} stderr={rejected.stderr!r}"
            )
        if json.loads(rejected.stdout).get("error", {}).get("code") != "invalid_input":
            fail(f"CLI replay.run rejection body was not invalid_input: {rejected.stdout!r}")
        status, body = http_post(base, "/v1/manage/replay.run", unknown_payload, ADMIN_TOKEN)
        if status != 400 or body.get("error", {}).get("code") != "invalid_input":
            fail(f"HTTP replay.run unknown field was not rejected: {status} {body}")
        incomplete = {"schema_version": REPLAY_RUN_SCHEMA, "request_key": "cli-replay-incomplete"}
        status, body = http_post(base, "/v1/manage/replay.run", incomplete, ADMIN_TOKEN)
        if status != 400 or body.get("error", {}).get("code") != "invalid_input":
            fail(f"HTTP incomplete replay.run was not rejected: {status} {body}")
        prepare = {"request_key": "prepare-1", "goal": "locate config", "capabilities": []}
        if http_post(base, "/v1/tools/prepare", prepare, None)[0] != 401:
            fail("anonymous HTTP request was not rejected")
        if http_post(base, "/v1/tools/prepare", prepare, "wrong-token-0123456789")[0] != 401:
            fail("wrong HTTP token was not rejected")
        if (
            http_post(
                base,
                "/v1/tools/prepare",
                prepare,
                AGENT_TOKEN,
                {"x-role": "admin"},
            )[0]
            != 403
        ):
            fail("identity header injection was not rejected")
        injected = dict(prepare, request_key="prepare-role", role="admin")
        if http_post(base, "/v1/tools/prepare", injected, AGENT_TOKEN)[0] != 403:
            fail("body role injection was not rejected")
        if (
            http_post_raw(
                base,
                "/v1/tools/prepare",
                '{"request_key":"prepare-1","request_key":"prepare-1","goal":"poison","capabilities":[]}',
                AGENT_TOKEN,
            )[0]
            != 400
        ):
            fail("duplicate request key was not rejected")
        if (
            http_post_raw(
                base,
                "/v1/host/trace",
                '{"schema_version":"rsia.optimization.source.v1","record":{"id":"trace-a","id":"trace-b"}}',
                HOST_TOKEN,
            )[0]
            != 400
        ):
            fail("nested duplicate trace key was not rejected")
        status, prepared = http_post(base, "/v1/tools/prepare", prepare, AGENT_TOKEN)
        if status != 200 or prepared.get("skills") != []:
            fail(f"zero-skill prepare failed: {status} {prepared}")
        run_id = prepared["run"]["id"]
        changed = dict(prepare, goal="different")
        if http_post(base, "/v1/tools/prepare", changed, AGENT_TOKEN)[0] != 409:
            fail("same idempotency key with different body was not rejected")

        status, snapshot = http_post(
            base, "/v1/host/snapshot", {"run_id": run_id}, HOST_TOKEN
        )
        if status != 200 or snapshot["projection"]["instructions"] != []:
            fail(f"trusted host snapshot failed: {status} {snapshot}")
        status, application = http_post(
            base,
            "/v1/host/application",
            {
                "run_id": run_id,
                "actual_request_material": snapshot["request_material"],
                "environment_digest": snapshot["environment_digest"],
                "host_surface_digest": snapshot["host_surface_digest"],
                "host_capabilities_digest": snapshot["host_capabilities_digest"],
                "offered": [],
                "attached": [],
                "used": [],
                "execution_receipt_id": None,
                "truncated": False,
            },
            HOST_TOKEN,
        )
        if status != 200 or application.get("receipt") is not None:
            fail(f"zero-skill receipt failed: {status} {application}")
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
        if process.stderr is not None:
            serve_stderr = process.stderr.read()
    # The startup gate reports the code-execution and sandbox posture before
    # the service listens, for an ordinary (never restored) data directory.
    if STARTUP_NORMAL not in serve_stderr.splitlines():
        fail(f"serve did not report its startup posture on stderr: {serve_stderr!r}")


class McpClient:
    def __init__(self, process: subprocess.Popen[str]) -> None:
        self.process = process
        self.selector = selectors.DefaultSelector()
        assert process.stdout is not None
        self.selector.register(process.stdout, selectors.EVENT_READ)

    def send(self, message: dict[str, Any]) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
        self.process.stdin.flush()

    def send_raw(self, message: str) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(message + "\n")
        self.process.stdin.flush()

    def request(self, request_id: int, method: str, params: dict[str, Any]) -> dict[str, Any]:
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            events = self.selector.select(timeout=0.5)
            if not events:
                if self.process.poll() is not None:
                    fail(f"MCP process exited {self.process.returncode}")
                continue
            line = self.process.stdout.readline()
            if not line:
                continue
            message = json.loads(line)
            if message.get("id") == request_id:
                if "error" in message:
                    fail(f"MCP error for {method}: {message['error']}")
                return message["result"]
        fail(f"MCP timeout for {method}")


def mcp_tool(client: McpClient, request_id: int, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
    result = client.request(
        request_id, "tools/call", {"name": name, "arguments": arguments}
    )
    if result.get("isError"):
        fail(f"MCP tool {name} returned error: {result}")
    return result["structuredContent"]


def smoke_mcp(binary: Path, root: Path) -> None:
    process = subprocess.Popen(
        [
            str(binary),
            "mcp",
            "--data",
            str(root / "mcp.sqlite3"),
            "--namespace",
            "smoke",
            "--actor",
            "stdio-agent",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    mcp_stderr = ""
    try:
        client = McpClient(process)
        client.request(
            1,
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "rsia-smoke", "version": "1.0.0"},
            },
        )
        client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools = client.request(2, "tools/list", {})["tools"]
        names = [tool["name"] for tool in tools]
        if sorted(names) != sorted(
            ["evo_prepare", "evo_feedback", "evo_propose", "evo_inspect"]
        ):
            fail(f"unexpected MCP tool list: {names}")
        prepared = mcp_tool(
            client,
            3,
            "evo_prepare",
            {"request_key": "prepare-1", "goal": "locate config", "capabilities": []},
        )
        run_id = prepared["run"]["id"]
        snapshot_id = prepared["run"]["snapshot_id"]
        feedback = mcp_tool(
            client,
            4,
            "evo_feedback",
            {
                "request_key": "feedback-1",
                "run_id": run_id,
                "outcome": "failure",
                "details": "self report",
                "failure_class": "reasoning",
            },
        )
        proposal = mcp_tool(
            client,
            5,
            "evo_propose",
            {
                "request_key": "proposal-1",
                "run_id": run_id,
                "kind": "skill",
                "parent_snapshot": snapshot_id,
                "hypothesis": "config precedence",
                "applicability": "reference host",
                "counterexample": "other host",
                "content": "read the higher priority config first",
                "evidence_refs": [feedback["id"]],
                "required_capabilities": [],
                "dependencies": [],
            },
        )
        inspected = mcp_tool(
            client,
            6,
            "evo_inspect",
            {"kind": "candidate", "id": proposal["id"]},
        )
        if inspected.get("state") != "proposed":
            fail(f"MCP inspect did not return proposed candidate: {inspected}")
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
        if process.stderr is not None:
            mcp_stderr = process.stderr.read()
    # stdout carried only JSON-RPC frames (McpClient parses every line); the
    # posture line must have gone to stderr.
    if STARTUP_NORMAL not in mcp_stderr.splitlines():
        fail(f"mcp did not report its startup posture on stderr: {mcp_stderr!r}")


def smoke_mcp_duplicate(binary: Path, root: Path) -> None:
    data = root / "mcp-duplicate.sqlite3"
    command = [
        str(binary),
        "mcp",
        "--data",
        str(data),
        "--namespace",
        "smoke",
        "--actor",
        "stdio-agent",
    ]
    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    client = McpClient(process)
    client.request(
        1,
        "initialize",
        {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "rsia-smoke", "version": "1.0.0"},
        },
    )
    client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    client.send_raw(
        '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"evo_prepare","arguments":{"request_key":"raw-dup","request_key":"raw-dup","goal":"poison","capabilities":[]}}}'
    )
    try:
        code = process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
        fail("MCP duplicate-key process did not reject the frame")
    if code == 0:
        fail("MCP duplicate-key process exited successfully")

    retry = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    try:
        retry_client = McpClient(retry)
        retry_client.request(
            1,
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "rsia-smoke", "version": "1.0.0"},
            },
        )
        retry_client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        prepared = mcp_tool(
            retry_client,
            2,
            "evo_prepare",
            {"request_key": "raw-dup", "goal": "clean", "capabilities": []},
        )
        if prepared["run"]["goal"] != "clean":
            fail("MCP duplicate-key frame wrote business state")
    finally:
        retry.terminate()
        try:
            retry.wait(timeout=5)
        except subprocess.TimeoutExpired:
            retry.kill()
            retry.wait(timeout=5)


def tree_digest(directory: Path) -> dict[str, str]:
    """Relative path -> sha256 of every file under `directory` (links recorded as such)."""
    result: dict[str, str] = {}
    for path in sorted(directory.rglob("*")):
        relative = path.relative_to(directory).as_posix()
        if path.is_symlink():
            result[relative] = f"symlink:{os.readlink(path)}"
        elif path.is_dir():
            result[relative] = "dir"
        else:
            result[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


def v2_restore_receipt() -> str:
    """A receipt with the structure restore_backup.py writes (rsia.restore_receipt.v2)."""
    return json.dumps(
        {
            "schema_version": "rsia.restore_receipt.v2",
            "backup_manifest_sha256": "a" * 64,
            "replayed_revoke_events": 0,
            "isolated": False,
            "trusted_anchor": {
                "path_local_only": "/nonexistent/anchor.sqlite3",
                "watermarks": {"smoke": {"seq": 1, "digest": "b" * 64}},
                "control_plane_digest": "c" * 64,
                "verified_at": "2026-09-30T00:00:00+00:00",
                "scope": "smoke structure only",
            },
            "receipt_scope": "local_only_not_for_export",
        }
    ) + "\n"


def smoke_recovery_gate(binary: Path, root: Path) -> None:
    """Startup recovery gate and deployment gate through the real process (E16.5)."""
    source = root / "http.sqlite3"
    if not source.is_file():
        fail("the HTTP smoke database is missing")
    port = free_port() + 2

    def serve(data: Path, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(binary), "serve", "--bind", f"127.0.0.1:{port}",
             "--data", str(data), "--namespace", "smoke", *extra],
            capture_output=True, text=True, timeout=10,
        )

    def mcp(data: Path, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(binary), "mcp", "--data", str(data), "--namespace", "smoke", *extra],
            input="", capture_output=True, text=True, timeout=10,
        )

    # No switch can enable code execution, a sandbox or external network.
    for command in ("serve", "mcp"):
        for switch in FORBIDDEN_SWITCHES:
            refused = subprocess.run(
                [str(binary), command, switch],
                input="", capture_output=True, text=True, timeout=10,
            )
            if refused.returncode != 2 or "unexpected argument" not in refused.stderr:
                fail(
                    f"{command} {switch} was not refused as an unknown argument: "
                    f"exit={refused.returncode} stderr={refused.stderr!r}"
                )

    # A restored-looking directory (an existing database and a v2-structured
    # restore.json) without an anchor is quarantined and left untouched.
    restored = root / "restored-shape"
    restored.mkdir()
    shutil.copyfile(source, restored / "rsia.sqlite3")
    (restored / "restore.json").write_text(v2_restore_receipt())
    before = tree_digest(restored)
    for label, run in (("serve", serve), ("mcp", mcp)):
        quarantined = run(restored / "rsia.sqlite3")
        if (
            quarantined.returncode == 0
            or "recovery_quarantine" not in quarantined.stderr
            or "not mounting" not in quarantined.stderr
        ):
            fail(
                f"{label}: a restored directory without an anchor was not quarantined: "
                f"exit={quarantined.returncode} stderr={quarantined.stderr!r}"
            )
        if "code_execution=disabled" in quarantined.stderr:
            fail(f"{label}: a quarantined start reported a posture line")
        if tree_digest(restored) != before:
            fail(f"{label}: a quarantined start wrote to the data directory")

    # An anchor is only accepted for a restored directory that was not admitted.
    plain = root / "never-restored"
    plain.mkdir()
    shutil.copyfile(source, plain / "rsia.sqlite3")
    before = tree_digest(plain)
    for label, run in (("serve", serve), ("mcp", mcp)):
        rejected = run(plain / "rsia.sqlite3", "--trusted-revocations-db", str(source))
        if rejected.returncode == 0 or "startup_rejected" not in rejected.stderr:
            fail(
                f"{label}: an anchor for an unrestored directory was not rejected: "
                f"exit={rejected.returncode} stderr={rejected.stderr!r}"
            )
        if tree_digest(plain) != before:
            fail(f"{label}: a rejected start wrote to the data directory")
    # ... and not even a directory that does not exist yet is created.
    absent = root / "never-created"
    rejected = serve(absent / "rsia.sqlite3", "--trusted-revocations-db", str(source))
    if rejected.returncode == 0 or "startup_rejected" not in rejected.stderr:
        fail(f"an anchor for a missing directory was not rejected: {rejected.stderr!r}")
    if absent.exists():
        fail("a rejected start created the data directory")


def restore_inputs(base: Path, source: Path) -> tuple[Path, Path, Path]:
    """Build a live control plane, a complete backup of it and an empty revoke delta.

    `source` is a database the real MCP process wrote. A restore needs a revoke
    watermark for every namespace, which a service that never revoked anything
    does not have yet, so one is recorded before the backup is taken.
    """
    live_dir = base / "live"
    live_dir.mkdir()
    live = live_dir / "rsia.sqlite3"
    # Opening it recovers the WAL the terminated process left behind.
    with sqlite3.connect(source) as connection:
        namespaces = [
            row[0]
            for row in connection.execute(
                "SELECT namespace FROM objects UNION SELECT namespace FROM audit "
                "UNION SELECT namespace FROM root_budget_namespaces ORDER BY namespace"
            )
        ]
        if not namespaces:
            fail("the MCP smoke database has no namespace")
        for namespace in namespaces:
            digest = hashlib.sha256(f"smoke-initial-{namespace}".encode()).hexdigest()
            connection.execute(
                "INSERT OR IGNORE INTO revoke_watermark(namespace,seq,digest) VALUES(?,1,?)",
                (namespace, digest),
            )
        connection.commit()
        connection.execute("VACUUM INTO ?", (str(live),))
    backup = base / "backup"
    backup.mkdir()
    shutil.copyfile(live, backup / "rsia.sqlite3")
    database = (backup / "rsia.sqlite3").read_bytes()
    with sqlite3.connect(live) as connection:
        migrations = [
            {"version": version, "description": description,
             "checksum_hex": bytes(checksum).hex(), "success": bool(success)}
            for version, description, checksum, success in connection.execute(
                "SELECT version,description,checksum,success FROM _sqlx_migrations ORDER BY version"
            )
        ]
        marks = [
            {"namespace": namespace, "seq": seq, "digest": digest}
            for namespace, seq, digest in connection.execute(
                "SELECT namespace,seq,digest FROM revoke_watermark ORDER BY namespace"
            )
        ]
    manifest = json.dumps(
        {
            "schema_version": "rsia.backup.v2",
            "complete": True,
            "database": {
                "path": "rsia.sqlite3",
                "sha256": hashlib.sha256(database).hexdigest(),
                "bytes": len(database),
            },
            "blobs": [],
            "migrations": migrations,
            "watermarks": marks,
            "created_at": int(time.time()),
        },
        indent=2,
    ).encode()
    (backup / "backup-manifest.json").write_bytes(manifest)
    delta = base / "delta.json"
    delta.write_text(
        json.dumps(
            {
                "schema_version": "rsia.revoke_delta.v1",
                "base_manifest_sha256": hashlib.sha256(manifest).hexdigest(),
                "namespaces": [
                    {
                        "namespace": mark["namespace"],
                        "base_seq": mark["seq"],
                        "base_digest": mark["digest"],
                        "events": [],
                        "latest_seq": mark["seq"],
                        "latest_digest": mark["digest"],
                    }
                    for mark in marks
                ],
            }
        )
    )
    return live, backup, delta


def restore_into(base: Path, name: str, live: Path, backup: Path, delta: Path) -> Path:
    """Run the real restore script; returns the new data directory."""
    destination = base / name
    restored = subprocess.run(
        [
            sys.executable,
            str(Path(__file__).resolve().parent / "restore_backup.py"),
            "--backup", str(backup),
            "--dest", str(destination),
            "--revoke-delta", str(delta),
            "--trusted-revocations-db", str(live),
        ],
        capture_output=True, text=True, timeout=60,
    )
    if restored.returncode != 0 or "RESTORE_OK" not in restored.stdout:
        fail(f"restore_backup.py failed: {restored.stdout!r} {restored.stderr!r}")
    return destination


def stop(process: subprocess.Popen[str]) -> str:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
    return process.stderr.read() if process.stderr is not None else ""


def smoke_restore_admission(binary: Path, root: Path) -> None:
    """A real restore is verified against its anchor, admitted once, and re-started.

    Everything goes through the real binary and the real restore script: the
    restored directory is refused while the anchor has moved on, verified and
    admitted on its first start (HTTP and MCP entry points alike), and afterwards
    starts without any anchor.
    """
    source = root / "mcp.sqlite3"
    if not source.is_file():
        fail("the MCP smoke database is missing")
    base = root / "restore-e2e"
    base.mkdir()
    base = base.resolve()  # restore_backup.py refuses symlinks in a supplied path
    live, backup, delta = restore_inputs(base, source)
    serve_dir = restore_into(base, "restored-serve", live, backup, delta)
    mcp_dir = restore_into(base, "restored-mcp", live, backup, delta)
    stale_dir = restore_into(base, "restored-stale", live, backup, delta)
    port = free_port() + 3
    url = f"http://127.0.0.1:{port}"

    def start_serve(data: Path, *extra: str) -> subprocess.Popen[str]:
        return subprocess.Popen(
            [str(binary), "serve", "--bind", f"127.0.0.1:{port}", "--data", str(data),
             "--namespace", "smoke", "--actor", "agent-a", "--auth-token", AGENT_TOKEN,
             *extra],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )

    def posture(label: str) -> str:
        return f"rsia startup: code_execution=disabled sandbox=unavailable recovery={label}"

    def check_admission(directory: Path, receipt: bytes) -> None:
        if (directory / "restore.json").read_bytes() != receipt:
            fail("admission rewrote restore.json")
        admission_path = directory / "restore.admission.json"
        if not admission_path.is_file():
            fail(f"no admission record after a verified start: {sorted(p.name for p in directory.iterdir())}")
        admission = json.loads(admission_path.read_text())
        expected = {
            "schema_version": "rsia.restore_admission.v1",
            "receipt_sha256": hashlib.sha256(receipt).hexdigest(),
            "anchor_path_local_only": str(live),
            "namespaces": ["smoke"],
            "scope": "local_only_not_for_export",
        }
        for key, value in expected.items():
            if admission.get(key) != value:
                fail(f"admission {key} is {admission.get(key)!r}, expected {value!r}")
        if not isinstance(admission.get("admitted_at"), int) or len(admission.get("verified_facts_digest", "")) != 64:
            fail(f"admission record is malformed: {admission}")
        if sorted(p.name for p in directory.iterdir() if p.name.startswith(".restore.admission")):
            fail("a temporary admission file was left behind")

    # 1. HTTP entry point: verified against the anchor, then admitted before serving.
    receipt = (serve_dir / "restore.json").read_bytes()
    process = start_serve(serve_dir / "rsia.sqlite3", "--trusted-revocations-db", str(live))
    try:
        wait_http(url)
    finally:
        stderr = stop(process)
    if posture("restored_verified") not in stderr.splitlines():
        fail(f"serve did not report a verified restore: {stderr!r}")
    check_admission(serve_dir, receipt)

    # 2. The admitted directory starts without any anchor ...
    process = start_serve(serve_dir / "rsia.sqlite3")
    try:
        wait_http(url)
    finally:
        stderr = stop(process)
    if posture("admitted_previously") not in stderr.splitlines():
        fail(f"serve did not report an admitted directory: {stderr!r}")
    # ... and refuses one.
    before = tree_digest(serve_dir)
    refused = subprocess.run(
        [str(binary), "serve", "--bind", f"127.0.0.1:{port}", "--data", str(serve_dir / "rsia.sqlite3"),
         "--trusted-revocations-db", str(live)],
        capture_output=True, text=True, timeout=10,
    )
    if refused.returncode == 0 or "startup_rejected" not in refused.stderr:
        fail(f"an anchor for an admitted directory was not rejected: {refused.stderr!r}")
    if tree_digest(serve_dir) != before:
        fail("a rejected start wrote to an admitted data directory")

    # 3. MCP entry point: the admission lands before stdin is even read.
    receipt = (mcp_dir / "restore.json").read_bytes()
    started = subprocess.run(
        [str(binary), "mcp", "--data", str(mcp_dir / "rsia.sqlite3"), "--namespace", "smoke",
         "--trusted-revocations-db", str(live)],
        input="", capture_output=True, text=True, timeout=30,
    )
    if posture("restored_verified") not in started.stderr.splitlines():
        fail(f"mcp did not report a verified restore: {started.stderr!r}")
    if started.stdout.strip():
        fail(f"mcp wrote to stdout before any request: {started.stdout!r}")
    check_admission(mcp_dir, receipt)

    # 4. An anchor that moved on after the restore quarantines the directory, untouched.
    with sqlite3.connect(live) as connection:
        connection.execute(
            "UPDATE revoke_watermark SET seq=2,digest=?",
            (hashlib.sha256(b"revoked-after-restore").hexdigest(),),
        )
    before = tree_digest(stale_dir)
    stale = subprocess.run(
        [str(binary), "serve", "--bind", f"127.0.0.1:{port}", "--data", str(stale_dir / "rsia.sqlite3"),
         "--trusted-revocations-db", str(live)],
        capture_output=True, text=True, timeout=30,
    )
    if (
        stale.returncode == 0
        or "recovery_quarantine" not in stale.stderr
        or "advanced since restore (namespace smoke: seq 1 → 2)" not in stale.stderr
    ):
        fail(f"a restore behind its anchor was not quarantined: exit={stale.returncode} {stale.stderr!r}")
    if tree_digest(stale_dir) != before:
        fail("a quarantined restore was written to")
    if (stale_dir / "restore.admission.json").exists() or (stale_dir / ".rsia.lock").exists():
        fail("a quarantined restore gained an admission record or a lock file")


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: smoke_cli.py <rsia-binary>", file=sys.stderr)
        return 2
    binary = Path(argv[1]).resolve()
    if not binary.is_file():
        print(f"missing binary: {binary}", file=sys.stderr)
        return 1
    help_run = subprocess.run([str(binary), "--help"], capture_output=True, text=True)
    if help_run.returncode != 0 or "serve" not in help_run.stdout or "mcp" not in help_run.stdout:
        print("CLI help does not expose serve and mcp", file=sys.stderr)
        return 1
    secret = "must-not-appear-in-help-0123456789"
    help_environment = os.environ.copy()
    help_environment["RSIA_HTTP_TOKEN"] = secret
    help_environment["RSIA_MANAGEMENT_TOKEN"] = secret
    serve_help = subprocess.run(
        [str(binary), "serve", "--help"],
        capture_output=True,
        text=True,
        env=help_environment,
    )
    if serve_help.returncode != 0 or secret in (serve_help.stdout + serve_help.stderr):
        print("CLI help exposed an environment token", file=sys.stderr)
        return 1
    unusual_operation = 'unknown"\noperation'
    unusual = subprocess.run(
        [str(binary), "manage", unusual_operation, "--auth-token", ADMIN_TOKEN],
        capture_output=True,
        text=True,
    )
    try:
        unusual_json = json.loads(unusual.stderr.splitlines()[0])
    except (IndexError, json.JSONDecodeError):
        print("unknown management operation did not emit valid JSON", file=sys.stderr)
        return 1
    if unusual.returncode == 0 or unusual_json.get("operation") != unusual_operation:
        print("unknown management operation JSON changed the input", file=sys.stderr)
        return 1
    try:
        with tempfile.TemporaryDirectory(prefix="rsia-e07-smoke-") as directory:
            root = Path(directory)
            smoke_http(binary, root)
            smoke_mcp(binary, root)
            smoke_mcp_duplicate(binary, root)
            smoke_recovery_gate(binary, root)
            smoke_restore_admission(binary, root)
    except Exception as error:
        traceback.print_exc()
        print(f"SMOKE_CLI_FAILED: {error}", file=sys.stderr)
        return 1
    print("SMOKE_CLI_OK model_transport=disabled benefit_claimed=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
