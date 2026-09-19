#!/usr/bin/env python3
"""Real process smoke for CLI exit behavior, HTTP auth, and MCP stdio.

The smoke uses the disabled model transport and a zero-skill baseline. It does
not call a provider and does not claim model quality or benefit.
"""
from __future__ import annotations

import json
import os
import selectors
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


def fail(message: str) -> None:
    raise RuntimeError(message)


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
        request_file = root / "blocked-replay.json"
        request_file.write_text(json.dumps({
            "schema_version": "rsia.management.replay_run.v1",
            "request_key": "cli-blocked-1",
        }))
        submitted = subprocess.run(
            [str(binary), "manage", "replay.run", "--url", base,
             "--auth-token", ADMIN_TOKEN, "--request-file", str(request_file)],
            capture_output=True, text=True, timeout=5,
        )
        if submitted.returncode != 0:
            fail(f"CLI management submit failed: {submitted.stderr}")
        job = json.loads(submitted.stdout)
        final = None
        for _ in range(100):
            status = subprocess.run(
                [str(binary), "manage", "job.status", "--url", base,
                 "--auth-token", ADMIN_TOKEN, "--job-id", job["id"]],
                capture_output=True, text=True, timeout=5,
            )
            if status.returncode != 0:
                fail(f"CLI management status failed: {status.stderr}")
            final = json.loads(status.stdout)
            if final.get("state") in {"blocked", "failed", "cancelled", "succeeded"}:
                break
            time.sleep(0.01)
        if final is None or final.get("state") != "blocked":
            fail(f"CLI management did not preserve blocked terminal: {final}")
        if final.get("id") != job["id"]:
            fail("CLI management status did not return the persisted job")
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
    except Exception as error:
        traceback.print_exc()
        print(f"SMOKE_CLI_FAILED: {error}", file=sys.stderr)
        return 1
    print("SMOKE_CLI_OK model_transport=disabled benefit_claimed=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
