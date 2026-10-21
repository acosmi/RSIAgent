#!/usr/bin/env python3
"""Finite non-Rust declaration, approved CI egress, and actual console evidence."""
from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath

SCHEMA = "rsia.non_rust_delivery.v1"
RECEIPT_SCHEMA = "rsia.non_rust_delivery_receipt.v1"
REGISTER = "docs/non-rust-delivery-register.json"
WORKFLOW_PATH = ".github/workflows/non-rust-delivery.yml"
NODE_VERSION = "26.10.0"
RESULT_MARKER = "RSIA_NON_RUST_RESULT "
RECEIPT_MARKER = "RSIA_NON_RUST_RECEIPT "
BEGIN_MARKER = "RSIA_NON_RUST_BEGIN "
FRONTEND_PATHS = (
    'frontend/status-character/README.md',
    'frontend/status-character/bridge.mjs',
    'frontend/status-character/character.css',
    'frontend/status-character/character.mjs',
    'frontend/status-character/entry.mjs',
    'frontend/status-character/index.html',
    'frontend/status-character/page.css',
    'frontend/status-character/qa/fixture.css',
    'frontend/status-character/qa/fixture.mjs',
    'frontend/status-character/qa/index.html',
    'frontend/status-character/status-source.mjs',
    'frontend/status-character/tests/contracts.test.mjs',
    'frontend/status-character/v3-scene.mjs',
    'frontend/status-character/view-model.mjs',
)
PYTHON_PATHS = (
    'scripts/budget_report.py',
    'scripts/check_build_inputs.py',
    'scripts/check_dependency_register.py',
    'scripts/check_production_store_open.py',
    'scripts/check_support_scope.py',
    'scripts/estimate_sample_size.py',
    'scripts/inventory_baseline_tests.py',
    'scripts/non_rust_delivery.py',
    'scripts/restore_backup.py',
    'scripts/smoke.py',
    'scripts/smoke_cli.py',
    'scripts/smoke_workspace.py',
    'scripts/statistics_reference.py',
    'scripts/test_budget_report.py',
    'scripts/test_check_build_inputs.py',
    'scripts/test_check_dependency_register.py',
    'scripts/test_check_production_store_open.py',
    'scripts/test_check_support_scope.py',
    'scripts/test_mcp_stdio.py',
    'scripts/test_non_rust_delivery.py',
    'scripts/test_restore_backup.py',
)
CI_PATHS = (
    '.github/workflows/ci.yml',
    '.github/workflows/dependency-audit.yml',
    '.github/workflows/non-rust-delivery.yml',
    '.github/workflows/notify-merge.yml',
    '.github/workflows/offline-build-kit.yml',
    'docs/ci-egress-register.json',
    'docs/non-rust-delivery-register.json',
)
NODE_SOURCES = tuple(path for path in FRONTEND_PATHS if path.endswith(".mjs"))
WORKFLOW = "name: Non-Rust delivery\non:\n  push:\n    branches: [main]\n    paths: ['frontend/**', 'scripts/**', '.github/workflows/**', 'docs/non-rust-delivery-register.json', 'docs/ci-egress-register.json']\n  pull_request:\n    paths: ['frontend/**', 'scripts/**', '.github/workflows/**', 'docs/non-rust-delivery-register.json', 'docs/ci-egress-register.json']\n  workflow_dispatch:\npermissions:\n  contents: read\njobs:\n  frontend:\n    runs-on: ubuntu-latest\n    timeout-minutes: 15\n    env:\n      PYTHONOPTIMIZE: '0'\n      PYTHONDONTWRITEBYTECODE: '1'\n    steps:\n      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262\n      - uses: actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065\n        with:\n          python-version: '3.12'\n      - uses: actions/setup-node@249970729cb0ef3589644e2896645e5dc5ba9c38\n        with:\n          node-version: '26.10.0'\n          package-manager-cache: false\n      - name: Non-Rust commands and complete console receipt\n        run: python3 -B scripts/non_rust_delivery.py\n"
APPROVED_NOTIFY_SHA256 = "468dfeef8130ca28043cb755df1651f82db56af1f154bc60233bc06d6a654eaa"
APPROVED_NOTIFY = {'id': 'notify_merge', 'workflow': '.github/workflows/notify-merge.yml', 'workflow_sha256': '468dfeef8130ca28043cb755df1651f82db56af1f154bc60233bc06d6a654eaa', 'job': 'notify', 'step_index': 0, 'purpose': 'Existing merge notification to the repository owner automation', 'recipient': 'https://api2.cursor.sh/automations/webhook/aa6a72de-742e-5146-978d-b5a01ee6c101', 'trigger': 'pull_request closed, only if merged == true', 'method': 'POST', 'content_type': 'application/json', 'data_scope': 'Complete GitHub pull_request event JSON from GITHUB_EVENT_PATH; includes PR title/body, author, repository and commit metadata', 'secret_names': ['GROKBOT_WEBHOOK_KEY'], 'confirmed': True, 'confirmation': {'authority': 'direct_user_confirmation_current_session', 'observed_at': '2026-10-12T23:05:53.222058+00:00', 'scope': 'existing unchanged merge notification and its registration'}}
APPROVED_NOTIFY_RUN = 'curl -sf -X POST "https://api2.cursor.sh/automations/webhook/aa6a72de-742e-5146-978d-b5a01ee6c101" \\\n  -H "Authorization: Bearer ${{ secrets.GROKBOT_WEBHOOK_KEY }}" \\\n  -H "Content-Type: application/json" \\\n  --data-binary @"$GITHUB_EVENT_PATH"'


def _unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate non-Rust manifest key")
        result[key] = value
    return result


def _json(raw):
    return json.loads(raw, object_pairs_hook=_unique)


def _sha(raw):
    return hashlib.sha256(raw).hexdigest()


def _regular(root, relative):
    parts = PurePosixPath(relative).parts
    if not parts or relative != "/".join(parts) or any(p in ("..", ".") for p in parts) or relative.startswith("/"):
        raise ValueError("invalid registered relative path")
    current = root
    for part in parts[:-1]:
        current = current / part
        if not stat.S_ISDIR(current.lstat().st_mode):
            raise ValueError("non-Rust source directory is not an ordinary directory")
    path = root.joinpath(*parts)
    if not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError("non-Rust source is not a regular file: " + relative)
    return path.read_bytes()


def commands():
    return (
        [("node-version", ["node", "--version"])]
        + [("node-check-" + str(i + 1), ["node", "--check", path]) for i, path in enumerate(NODE_SOURCES)]
        + [
            ("node-test", ["node", "--test", "frontend/status-character/tests/contracts.test.mjs"]),
            ("python-discover", ["python3", "-B", "-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"]),
            ("build-inputs", ["python3", "-B", "scripts/check_build_inputs.py"]),
        ]
    )


def source_commands():
    # Fixed read-only Git argv; neither environment nor receipt supplies a command.
    git = ["git", "-c", "core.hooksPath=" + os.devnull, "-c", "core.fsmonitor=false"]
    return [
        ("source-root", git + ["rev-parse", "--show-toplevel"]),
        ("source-head", git + ["rev-parse", "--verify", "HEAD"]),
        ("source-status", git + ["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
        ("source-register", git + ["show", "HEAD:" + REGISTER]),
    ]


def _source_environment():
    # Also keep future test Git fixtures independent of global/system hooks/config.
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_CONFIG_SYSTEM=os.devnull, GIT_OPTIONAL_LOCKS="0",
               GIT_NO_REPLACE_OBJECTS="1", PYTHONDONTWRITEBYTECODE="1")
    return env


def _source_identity(root, rows, github_sha):
    """Identity is the actual clean preflight checkout, not GITHUB_SHA."""
    errors = []
    complete = [row["state"] == "completed" and type(row["exit_code"]) is int and row["exit_code"] == 0 for row in rows]
    if not all(complete):
        errors.append("source Git preflight did not complete successfully")
    if complete[0] and _decode_payload(rows[0]["stdout"]) != os.fsencode(str(root)) + b"\n":
        errors.append("source Git root differs from the actual runner checkout")
    head = None
    if complete[1]:
        raw = _decode_payload(rows[1]["stdout"])
        if re.fullmatch(rb"[0-9a-f]{40}\n", raw):
            head = raw[:-1].decode("ascii")
        else:
            errors.append("source Git HEAD is not one complete commit identity")
    if complete[2] and _decode_payload(rows[2]["stdout"]):
        errors.append("source checkout is dirty; only a clean preflight checkout is eligible")
    if github_sha is not None and github_sha != head:
        errors.append("GITHUB_SHA differs from actual checkout HEAD")
    register_sha = _sha(_decode_payload(rows[3]["stdout"])) if complete[3] else None
    return head, register_sha, errors


def _trusted_source(receipt, *, expected_source_sha, expected_register_sha256):
    # These required inputs must come from an independently verified commit and
    # its register blob, never from fields in the untrusted receipt itself.
    if not isinstance(expected_source_sha, str) or not re.fullmatch(r"[0-9a-f]{40}", expected_source_sha):
        raise ValueError("an independently trusted source commit is required")
    if not isinstance(expected_register_sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", expected_register_sha256):
        raise ValueError("an independently trusted committed register digest is required")
    for key, expected in (("source_sha", expected_source_sha), ("register_sha256", expected_register_sha256)):
        if receipt[key] is not None and receipt[key] != expected:
            raise ValueError(key + " differs from the independently trusted source")
        if receipt[key] is None and not receipt["preflight_errors"]:
            raise ValueError("successful preflight has no bound " + key)


def declaration():
    return {
        "schema_version": SCHEMA,
        "workflow": WORKFLOW_PATH,
        "job": "frontend",
        "entrypoint": ["python3", "-B", "scripts/non_rust_delivery.py"],
        "node_version": NODE_VERSION,
        "python_version": "3.12",
        "groups": {
            "frontend": {"paths": list(FRONTEND_PATHS), "command_ids": [name for name, _ in commands() if name.startswith("node-")]},
            "python": {"paths": list(PYTHON_PATHS), "command_ids": ["python-discover", "build-inputs"]},
            "ci": {"paths": list(CI_PATHS), "command_ids": ["build-inputs"]},
        },
        "evidence": {
            "declaration_only": True,
            "actual_results": "separate runtime receipt, never prefilled",
            "console": "complete raw streams as base64 with run/attempt URL, job name and fixed command markers",
            "local_logs": "temporary job copies; no artifact upload or cloud-retention claim",
            "python_discover": "MCP may be skipped without RSIA_BINARY; actual skip diagnostics and counts remain in raw stderr",
            "new_verified_subscopes": [],
            "cargo_target_credit": False,
            "platform_artifact_classification": "existing three Rust workflow steps only; pending secondary acceptance; no notify authority borrowed",
        },
    }


def workflow_errors(text):
    return [] if text == WORKFLOW else ["non-rust-delivery.yml: fixed frontend job, commands, triggers or Actions differ"]


def _inventory(root, directory, suffix=None):
    start = root
    for part in PurePosixPath(directory).parts:
        start = start / part
        if not stat.S_ISDIR(start.lstat().st_mode):
            raise ValueError("non-Rust source root is not an ordinary directory")
    paths = []
    for here, directories, files in os.walk(start, followlinks=False):
        for name in directories:
            if not stat.S_ISDIR((Path(here) / name).lstat().st_mode):
                raise ValueError("symlink/non-directory in non-Rust sources")
        for name in files:
            path = Path(here) / name
            if suffix is None or path.suffix == suffix:
                relative = path.relative_to(root).as_posix()
                _regular(root, relative)
                paths.append(relative)
    return sorted(paths)


def inspect_contract(root):
    """No subprocesses or writes. Rust step parsing remains in check_build_inputs."""
    root = Path(root)
    errors = []
    try:
        actual = _json(_regular(root, REGISTER))
        if json.dumps(actual, sort_keys=True) != json.dumps(declaration(), sort_keys=True):
            raise ValueError("non-Rust declaration differs from the finite path/command/evidence contract")
        if _inventory(root, "frontend/status-character") != list(FRONTEND_PATHS):
            raise ValueError("frontend source inventory differs from the registered complete paths")
        if _inventory(root, "scripts", ".py") != list(PYTHON_PATHS):
            raise ValueError("Python source inventory differs from the registered complete paths")
        directory = root / ".github/workflows"
        if not stat.S_ISDIR((root / ".github").lstat().st_mode) or not stat.S_ISDIR(directory.lstat().st_mode):
            raise ValueError("workflow directory is not an ordinary directory")
        members = sorted(p.relative_to(root).as_posix() for p in directory.iterdir())
        expected = [p for p in CI_PATHS if p.startswith(".github/workflows/")]
        if members != expected:
            raise ValueError("unknown or missing workflow in non-Rust/egress inventory")
        for path in CI_PATHS:
            _regular(root, path)
        errors.extend(workflow_errors(_regular(root, WORKFLOW_PATH).decode()))
    except (OSError, ValueError, TypeError, UnicodeError) as error:
        errors.append(str(error))
    try:
        egress = _json(_regular(root, "docs/ci-egress-register.json"))
        if json.dumps(egress, sort_keys=True) != json.dumps({"schema_version": "rsia.ci_egress_register.v1", "entries": [APPROVED_NOTIFY]}, sort_keys=True):
            raise ValueError("CI egress register differs from the sole independently approved notify entry")
        text = _regular(root, APPROVED_NOTIFY["workflow"])
        actual_sha = _sha(text)
        if actual_sha != APPROVED_NOTIFY_SHA256 or actual_sha != egress["entries"][0]["workflow_sha256"]:
            raise ValueError("notify workflow differs from the independently approved complete bytes")
        # The old finite parser still checks the actual job/step. Header/trigger/
        # condition are independently bound by the complete approved byte digest.
        from check_build_inputs import _parse_workflow
        jobs, steps = _parse_workflow(text.decode())
        if jobs != [APPROVED_NOTIFY["job"]] or steps != [{"run": APPROVED_NOTIFY_RUN}] or APPROVED_NOTIFY["step_index"] != 0:
            raise ValueError("notify job/step/recipient/body/secret differs from its approval")
    except (OSError, ValueError, TypeError, KeyError, UnicodeError) as error:
        errors.append(str(error))
    return errors


def console_location(environ):
    location = {
        "kind": "local-console",
        "run_url": None,
        "attempt_url": None,
        "job_name": None,
        "job_url": None,
        "job_locator": "open the actual run/attempt and select the recorded job; numeric job URL is unavailable",
        "command_marker": BEGIN_MARKER.rstrip(),
        "local_copy_retention": "temporary",
    }
    if environ.get("GITHUB_ACTIONS") == "true":
        server = environ.get("GITHUB_SERVER_URL", "")
        repository = environ.get("GITHUB_REPOSITORY", "")
        run = environ.get("GITHUB_RUN_ID", "")
        attempt = environ.get("GITHUB_RUN_ATTEMPT", "")
        job = environ.get("GITHUB_JOB", "")
        if not (
            server == "https://github.com"
            and re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository)
            and re.fullmatch(r"[1-9][0-9]*", run)
            and re.fullmatch(r"[1-9][0-9]*", attempt)
            and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_-]*", job)
        ):
            raise ValueError("actual GitHub console identity is incomplete")
        url = server + "/" + repository + "/actions/runs/" + run
        location.update(kind="github-console", run_url=url, attempt_url=url + "/attempts/" + attempt, job_name=job)
    return location


def _write_new(path, raw):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        view = memoryview(raw)
        while view:
            written = os.write(fd, view)
            if written <= 0:
                raise OSError("incomplete evidence write")
            view = view[written:]
        os.fsync(fd)
    finally:
        os.close(fd)


def _payload(raw, path):
    return {"path": path, "bytes": len(raw), "sha256": _sha(raw), "base64": base64.b64encode(raw).decode("ascii")}


def _summary(raw):
    text = raw.decode("utf-8", errors="replace")
    count = re.findall(r"^Ran ([0-9]+) tests? in ", text, re.M)
    ok = re.findall(r"^OK(?: \(skipped=([0-9]+)\))?$", text, re.M)
    return {
        "tests": int(count[0]) if len(count) == 1 else None,
        "skipped": (int(ok[0]) if ok[0] else 0) if len(ok) == 1 else None,
        "terminal_ok": len(ok) == 1,
    }


def _emit(marker, value, console):
    print(marker + json.dumps(value, sort_keys=True, separators=(",", ":")), file=console, flush=True)


def _staging(root):
    parent = root
    for name in ("out", "non-rust-delivery"):
        parent = parent / name
        try:
            parent.mkdir(mode=0o700)
        except FileExistsError:
            pass
        if not stat.S_ISDIR(parent.lstat().st_mode):
            raise ValueError("local evidence parent is not an ordinary directory")
    return Path(tempfile.mkdtemp(prefix="run-", dir=parent))


def run_delivery(root, *, console=None):
    """Only the fixed command list is executable; no shell or caller argv entry."""
    root = Path(root).resolve()
    console = sys.stdout if console is None else console
    rows = [{"id": name, "argv": argv, "cwd": str(root), "state": "not_run", "exit_code": None, "launch_error": None, "stdout": None, "stderr": None} for name, argv in commands()]
    receipt = {
        "schema_version": RECEIPT_SCHEMA,
        "cwd": str(root),
        "register_sha256": None,
        "source_sha": None,
        "github_sha": os.environ.get("GITHUB_SHA"),
        "source_git": [{"id": name, "argv": argv, "cwd": str(root), "state": "not_run", "exit_code": None, "launch_error": None, "stdout": None, "stderr": None} for name, argv in source_commands()],
        "location": None,
        "commands": rows,
        "preflight_errors": [],
        "receipt_errors": [],
        "python_unittest_summary": None,
        "local_receipt_path": None,
        "new_verified_subscopes": [],
        "exit_code": 1,
    }
    staging = None
    try:
        receipt["location"] = console_location(os.environ)
        receipt["preflight_errors"] = inspect_contract(root)
        if sys.version_info[:2] != (3, 12) or sys.flags.optimize != 0:
            receipt["preflight_errors"].append("runner requires Python 3.12 without optimization")
        if not receipt["preflight_errors"]:
            staging = _staging(root)
            receipt["local_receipt_path"] = (staging / "receipt.json").relative_to(root).as_posix()
            env = _source_environment()
            for row in receipt["source_git"]:
                _emit(BEGIN_MARKER, {"id": row["id"], "argv": row["argv"], "cwd": row["cwd"], "location": receipt["location"]}, console)
                try:
                    result = subprocess.run(row["argv"], cwd=root, env=env, stdin=subprocess.DEVNULL, capture_output=True, check=False)
                except OSError as error:
                    row.update(state="launch_failed", launch_error=str(error))
                    _emit(RESULT_MARKER, row, console)
                    break
                row.update(state="completed", exit_code=result.returncode)
                for stream, raw in (("stdout", result.stdout), ("stderr", result.stderr)):
                    path = staging / (row["id"] + "." + stream)
                    relative = path.relative_to(root).as_posix()
                    row[stream] = _payload(raw, relative)
                    try:
                        _write_new(path, raw)
                    except OSError as error:
                        receipt["receipt_errors"].append(relative + ": " + str(error))
                _emit(RESULT_MARKER, row, console)
                if result.returncode != 0 or receipt["receipt_errors"]:
                    break
            head, register_sha, source_errors = _source_identity(root, receipt["source_git"], receipt["github_sha"])
            receipt.update(source_sha=head, register_sha256=register_sha)
            receipt["preflight_errors"].extend(source_errors)
            if register_sha is not None and _regular(root, REGISTER) != _decode_payload(receipt["source_git"][-1]["stdout"]):
                receipt["preflight_errors"].append("working declaration differs from the register blob at actual HEAD")
        if not receipt["preflight_errors"] and not receipt["receipt_errors"]:
            for row in rows:
                _emit(BEGIN_MARKER, {"id": row["id"], "argv": row["argv"], "cwd": row["cwd"], "location": receipt["location"]}, console)
                try:
                    result = subprocess.run(row["argv"], cwd=root, env=env, stdin=subprocess.DEVNULL, capture_output=True, check=False)
                except OSError as error:
                    row.update(state="launch_failed", launch_error=str(error))
                    _emit(RESULT_MARKER, row, console)
                    break
                row.update(state="completed", exit_code=result.returncode)
                for stream, raw in (("stdout", result.stdout), ("stderr", result.stderr)):
                    path = staging / (row["id"] + "." + stream)
                    relative = path.relative_to(root).as_posix()
                    row[stream] = _payload(raw, relative)
                    try:
                        _write_new(path, raw)
                    except OSError as error:
                        receipt["receipt_errors"].append(relative + ": " + str(error))
                if row["id"] == "python-discover":
                    receipt["python_unittest_summary"] = _summary(result.stderr)
                    summary = receipt["python_unittest_summary"]
                    if result.returncode == 0 and (not summary["terminal_ok"] or not summary["tests"]):
                        receipt["receipt_errors"].append("Python discover lacks a nonempty actual OK terminal")
                if row["id"] == "node-version" and result.returncode == 0 and result.stdout.strip() != ("v" + NODE_VERSION).encode():
                    receipt["receipt_errors"].append("actual Node version differs from " + NODE_VERSION)
                _emit(RESULT_MARKER, row, console)
                if result.returncode != 0 or receipt["receipt_errors"]:
                    break
            receipt["exit_code"] = 0 if all(row["state"] == "completed" and type(row["exit_code"]) is int and row["exit_code"] == 0 for row in rows) and not receipt["receipt_errors"] else 1
    except (OSError, ValueError, TypeError, UnicodeError) as error:
        receipt["preflight_errors"].append(str(error))
    if staging is not None:
        try:
            _write_new(staging / "receipt.json", json.dumps(receipt, sort_keys=True, indent=2).encode() + b"\n")
        except OSError as error:
            receipt["exit_code"] = 1
            receipt["receipt_errors"].append("receipt.json: " + str(error))
    _emit(RECEIPT_MARKER, receipt, console)
    return receipt["exit_code"], receipt


def _decode_payload(value):
    if not isinstance(value, dict) or set(value) != {"path", "bytes", "sha256", "base64"}:
        raise ValueError("unsupported stream evidence fields")
    try:
        raw = base64.b64decode(value["base64"], validate=True)
    except (ValueError, TypeError) as error:
        raise ValueError("invalid stream base64") from error
    if type(value["bytes"]) is not int or value["bytes"] != len(raw) or value["sha256"] != _sha(raw):
        raise ValueError("stream evidence bytes/hash mismatch")
    return raw


def restore_console(console, *, expected_source_sha, expected_register_sha256):
    """Recover all streams, bound to caller-supplied independently trusted pins."""
    lines = console.splitlines()
    finals = [_json(line[len(RECEIPT_MARKER):]) for line in lines if line.startswith(RECEIPT_MARKER)]
    if len(finals) != 1:
        raise ValueError("complete console must contain one final receipt")
    receipt = finals[0]
    expected_fields = {"schema_version", "cwd", "register_sha256", "source_sha", "github_sha", "source_git", "location", "commands", "preflight_errors", "receipt_errors", "python_unittest_summary", "local_receipt_path", "new_verified_subscopes", "exit_code"}
    if not isinstance(receipt, dict) or set(receipt) != expected_fields or receipt["schema_version"] != RECEIPT_SCHEMA or receipt["new_verified_subscopes"] != []:
        raise ValueError("unsupported receipt or fabricated verification credit")
    location = receipt["location"]
    fields = {"kind", "run_url", "attempt_url", "job_name", "job_url", "job_locator", "command_marker", "local_copy_retention"}
    if not isinstance(location, dict) or set(location) != fields or location["job_url"] is not None or location["local_copy_retention"] != "temporary" or location["command_marker"] != BEGIN_MARKER.rstrip():
        raise ValueError("console location invents an artifact or job URL")
    if location["kind"] == "local-console":
        if any(location[key] is not None for key in ("run_url", "attempt_url", "job_name")):
            raise ValueError("local console cannot claim a hosted CI run")
    elif location["kind"] == "github-console":
        if not isinstance(location["run_url"], str) or not re.fullmatch(r"https://github.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/actions/runs/[1-9][0-9]*", location["run_url"]) or not isinstance(location["attempt_url"], str) or not re.fullmatch(re.escape(location["run_url"]) + r"/attempts/[1-9][0-9]*", location["attempt_url"]) or not isinstance(location["job_name"], str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_-]*", location["job_name"]):
            raise ValueError("invalid actual run/attempt/job locator")
    else:
        raise ValueError("unknown console location")
    _trusted_source(receipt, expected_source_sha=expected_source_sha, expected_register_sha256=expected_register_sha256)
    source_rows = receipt["source_git"]
    fixed_source = source_commands()
    if not isinstance(source_rows, list) or len(source_rows) != len(fixed_source):
        raise ValueError("source Git checks differ from the fixed read-only preflight")
    observed_source, recovered_source = [], {}
    source_stopped = False
    for row, (name, argv) in zip(source_rows, fixed_source):
        if not isinstance(row, dict) or set(row) != {"id", "argv", "cwd", "state", "exit_code", "launch_error", "stdout", "stderr"} or row["id"] != name or row["argv"] != argv or row["cwd"] != receipt["cwd"]:
            raise ValueError("source Git argv or checkout differs from fixed preflight")
        if row["state"] == "not_run":
            if not receipt["preflight_errors"] and not receipt["receipt_errors"]:
                raise ValueError("unrun source Git check has no prior failure")
            if any(row[key] is not None for key in ("exit_code", "launch_error", "stdout", "stderr")):
                raise ValueError("unrun source Git checks must retain null results")
            source_stopped = True
            continue
        if source_stopped:
            raise ValueError("source Git check follows an unrun or failed check")
        observed_source.append(row)
        if row["state"] == "launch_failed":
            if row["exit_code"] is not None or not isinstance(row["launch_error"], str) or row["stdout"] is not None or row["stderr"] is not None:
                raise ValueError("Git launch refusal is not a numeric exit")
            source_stopped = True
            continue
        if row["state"] != "completed" or type(row["exit_code"]) is not int or row["launch_error"] is not None:
            raise ValueError("invalid actual source Git result")
        recovered_source[name] = {stream: _decode_payload(row[stream]) for stream in ("stdout", "stderr")}
        source_stopped = row["exit_code"] != 0
    if observed_source:
        head, register_sha, source_errors = _source_identity(receipt["cwd"], source_rows, receipt["github_sha"])
    else:
        head, register_sha, source_errors = None, None, ["source preflight was not run"]
    if receipt["source_sha"] != head or receipt["register_sha256"] != register_sha:
        raise ValueError("claimed source differs from original Git preflight streams")
    if source_errors and not receipt["preflight_errors"]:
        raise ValueError("failed or dirty source preflight cannot be eligible")
    rows = receipt["commands"]
    fixed = commands()
    if not isinstance(rows, list) or len(rows) != len(fixed):
        raise ValueError("receipt command list differs from the fixed entry")
    results = [_json(line[len(RESULT_MARKER):]) for line in lines if line.startswith(RESULT_MARKER)]
    observed = list(observed_source)
    recovered = dict(recovered_source)
    skipped = False
    stopped = bool(receipt["preflight_errors"])
    for row, (name, argv) in zip(rows, fixed):
        if not isinstance(row, dict) or set(row) != {"id", "argv", "cwd", "state", "exit_code", "launch_error", "stdout", "stderr"} or row["id"] != name or row["argv"] != argv or row["cwd"] != receipt["cwd"]:
            raise ValueError("receipt command identity differs from fixed entry")
        if row["state"] == "not_run":
            if not stopped and not receipt["receipt_errors"]:
                raise ValueError("unrun command has no recorded prior failure")
            skipped = True
            if any(row[key] is not None for key in ("exit_code", "launch_error", "stdout", "stderr")):
                raise ValueError("unrun command must retain null results")
            continue
        if skipped or stopped:
            raise ValueError("executed command follows an unrun or failed command")
        observed.append(row)
        if row["state"] == "launch_failed":
            if row["exit_code"] is not None or not isinstance(row["launch_error"], str) or row["stdout"] is not None or row["stderr"] is not None:
                raise ValueError("launch refusal is not a numeric child exit")
            stopped = True
            continue
        if row["state"] != "completed" or type(row["exit_code"]) is not int or row["launch_error"] is not None:
            raise ValueError("invalid actual child result")
        recovered[name] = {stream: _decode_payload(row[stream]) for stream in ("stdout", "stderr")}
        stopped = row["exit_code"] != 0 or (name == "node-version" and recovered[name]["stdout"].strip() != ("v" + NODE_VERSION).encode())
    begins = [_json(line[len(BEGIN_MARKER):]) for line in lines if line.startswith(BEGIN_MARKER)]
    if begins != [{"id": row["id"], "argv": row["argv"], "cwd": row["cwd"], "location": location} for row in observed]:
        raise ValueError("console begin markers are missing or altered")
    if results != observed:
        raise ValueError("console command results are missing, altered or duplicated")
    python = recovered.get("python-discover")
    if receipt["python_unittest_summary"] != (_summary(python["stderr"]) if python is not None else None):
        raise ValueError("Python skip/result summary differs from original stderr")
    passed = all(row["state"] == "completed" and row["exit_code"] == 0 for row in source_rows + rows) and not receipt["preflight_errors"] and not receipt["receipt_errors"]
    if passed and (not receipt["python_unittest_summary"]["terminal_ok"] or not receipt["python_unittest_summary"]["tests"]):
        passed = False
    if passed and recovered["node-version"]["stdout"].strip() != ("v" + NODE_VERSION).encode():
        passed = False
    if type(receipt["exit_code"]) is not int or receipt["exit_code"] != (0 if passed else 1):
        raise ValueError("receipt exit differs from actual child outcomes")
    return receipt, recovered


def verify_local_copies(root, receipt, *, expected_source_sha, expected_register_sha256):
    """Expected pins are trusted independently of both console and local copies."""
    root = Path(root)
    _trusted_source(receipt, expected_source_sha=expected_source_sha, expected_register_sha256=expected_register_sha256)
    if _sha(_regular(root, REGISTER)) != expected_register_sha256:
        raise ValueError("local declaration differs from the independently trusted committed register")
    if any(row["state"] != "not_run" for row in receipt["source_git"]):
        head, register_sha, errors = _source_identity(str(root), receipt["source_git"], receipt["github_sha"])
        if receipt["source_sha"] != head or receipt["register_sha256"] != register_sha or (errors and not receipt["preflight_errors"]):
            raise ValueError("local source claims differ from original Git preflight streams")
    if receipt["local_receipt_path"] is not None:
        if _json(_regular(root, receipt["local_receipt_path"])) != receipt:
            raise ValueError("local receipt differs from the complete console receipt")
    for row in receipt["source_git"] + receipt["commands"]:
        if row["state"] == "completed":
            for stream in ("stdout", "stderr"):
                value = row[stream]
                path = value["path"]
                if not isinstance(path, str) or not path.startswith("out/non-rust-delivery/run-"):
                    raise ValueError("stream path is not a temporary local copy")
                if _regular(root, path) != _decode_payload(value):
                    raise ValueError("temporary local log differs from console original")


def main():
    if sys.argv[1:]:
        print("NON_RUST_DELIVERY_FAIL: this fixed entry accepts no arguments", file=sys.stderr)
        return 1
    code, _ = run_delivery(Path(__file__).resolve().parents[1])
    return code


if __name__ == "__main__":
    raise SystemExit(main())
