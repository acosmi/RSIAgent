#!/usr/bin/env python3
"""Read-only checks for the repository's finite build-input contract (V113.a/c)."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import stat
import subprocess
import sys
import tomllib
from pathlib import Path

TOOLCHAIN = "1.98.1"
ACTIONS = {
    "actions/checkout": "11d5960a326750d5838078e36cf38b85af677262",
    "actions/setup-python": "a26af69be951a213d495a4c3e4e4022e16d87065",
    "actions/upload-artifact": "ea165f8d65b6e75b540449e92b4886f43607fa02",
}
BUILD_WORKFLOWS = ("ci.yml", "dependency-audit.yml", "offline-build-kit.yml")
LOCK_GATE = "\n".join((
    "test -f Cargo.lock", "test ! -L Cargo.lock",
    "git cat-file -e HEAD:Cargo.lock", "git diff --exit-code HEAD -- Cargo.lock",
))
INPUT_GATE = "python3 scripts/check_build_inputs.py"
RUST_INSTALL = (
    f"rustup toolchain install {TOOLCHAIN} --profile minimal "
    "--component rustfmt --component clippy"
)
CARGO_LOCKED = {"fetch", "metadata", "check", "test", "clippy", "build", "install"}
CARGO_READ_ONLY = {"fmt", "audit", "--version", "-V"}


def _read_regular(path: Path) -> bytes:
    if not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError(f"not a regular file: {path.name}")
    return path.read_bytes()


def _git(root: Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", "-C", str(root), *args], capture_output=True, timeout=15, check=False
    )
    if result.returncode:
        raise ValueError("cannot read committed build input: " + " ".join(args))
    return result.stdout


def _yaml_literal(text: str) -> str:
    text = text.strip()
    if text.startswith("'"):
        cursor, value = 1, ""
        while cursor < len(text):
            if text[cursor] == "'":
                if cursor + 1 < len(text) and text[cursor + 1] == "'":
                    value += "'"
                    cursor += 2
                    continue
                tail = text[cursor + 1:].strip()
                if tail and not tail.startswith("#"):
                    raise ValueError("unsupported YAML scalar suffix")
                return value
            value += text[cursor]
            cursor += 1
        raise ValueError("unsupported quoted YAML scalar")
    if text.startswith('"'):
        try:
            value, used = json.JSONDecoder().raw_decode(text)
        except ValueError as error:
            raise ValueError("unsupported quoted YAML scalar") from error
        tail = text[used:].strip()
        if not isinstance(value, str) or (tail and not tail.startswith("#")):
            raise ValueError("unsupported YAML scalar suffix")
        return value
    return re.split(r"\s+#", text, maxsplit=1)[0].strip()


def _parse_workflow(text: str) -> tuple[list[str], list[dict[str, object]]]:
    """Finite single-job block steps/with/env layout; unsupported YAML refuses."""
    lines = text.splitlines()
    jobs, steps = [], []
    in_jobs = in_steps = False
    current = section = None
    i = 0
    while i < len(lines):
        line = lines[i]
        i += 1
        item = line.strip()
        if not item or item.startswith("#"):
            continue
        if "\t" in line or re.search(r"(?:^|\s)[&*][A-Za-z0-9_-]+", line) or re.search(r"(?:^|\s)!!?\w", line):
            raise ValueError("unsupported workflow indentation/anchor/alias/tag")
        if re.match(r"\s*(?:-\s*)?\?", line) or re.search(r"['\"](?:uses|run|steps|jobs|env)['\"]\s*:", line) or re.match(r"\s*-\s*[\[{]", line):
            raise ValueError("unsupported workflow structural shape")
        indent = len(line) - len(line.lstrip())
        if indent in {0, 4}:
            header = re.match(r"""^("(?:[^"\\]|\\.)*"|'(?:[^']|'')*'|[A-Za-z_][\w-]*)\s*:""", item)
            if header and _yaml_literal(header[1]) == "defaults":
                raise ValueError("unsupported workflow/job execution default")
            if item.startswith(('"', "'")):
                raise ValueError("unsupported quoted workflow/job key")
        if item == "jobs:" and indent == 0:
            if in_jobs:
                raise ValueError("unsupported duplicate jobs mapping")
            in_jobs = True
            continue
        if in_jobs and indent == 2:
            m = re.fullmatch(r"([A-Za-z_][\w-]*):", item)
            if m is None:
                raise ValueError("unsupported job structure")
            jobs.append(m[1])
            in_steps, current = False, None
            continue
        if re.match(r"(?:steps|jobs|env)\s*:", item) and item.split(":", 1)[1].strip():
            raise ValueError("unsupported workflow structural shape")
        if re.match(r"steps\s*:", item):
            if indent != 4 or not jobs or in_steps:
                raise ValueError("unsupported job structure")
            in_steps = True
            continue
        if not in_steps:
            continue
        if indent == 6 and item.startswith("- "):
            current, section = {}, None
            steps.append(current)
            field = item[2:]
        elif indent == 8 and current is not None:
            section, field = None, item
        elif indent == 10 and current is not None and section in {"with", "env"}:
            m = re.fullmatch(r"([A-Za-z_][\w-]*)\s*:\s*(.*)", item)
            if m is None or m[1] in current[section]:
                raise ValueError("unsupported duplicate/nested step attribute")
            current[section][m[1]] = _yaml_literal(m[2])
            continue
        else:
            raise ValueError("unsupported step/job indentation or field")
        m = re.fullmatch(r"([A-Za-z_][\w-]*)\s*:\s*(.*)", field)
        if m is None or m[1] in current:
            raise ValueError("unsupported duplicate/step field")
        key, value = m.groups()
        if key in {"with", "env"}:
            if value:
                raise ValueError("unsupported workflow structural shape")
            current[key], section = {}, key
        elif key == "run" and value == "|":
            body = []
            while i < len(lines) and (not lines[i].strip() or len(lines[i]) - len(lines[i].lstrip()) > indent):
                body.append(lines[i])
                i += 1
            margin = min((len(x) - len(x.lstrip()) for x in body if x.strip()), default=indent + 2)
            current[key] = "\n".join(x[margin:] if x.strip() else "" for x in body).strip()
        elif key == "run" and (value.startswith(("|", ">")) or not value):
            raise ValueError("unsupported run block")
        else:
            current[key] = _yaml_literal(value)
    if len(jobs) != 1:
        raise ValueError("unsupported job structure")
    return jobs, steps


def workflow_inputs(text: str) -> tuple[list[str], list[str]]:
    _, steps = _parse_workflow(text)
    return [s["uses"] for s in steps if "uses" in s], [s["run"] for s in steps if "run" in s]


def _step_digest(step: dict[str, object]) -> str:
    return hashlib.sha256(json.dumps(step, sort_keys=True).encode()).hexdigest()


def _shell_errors(run: str) -> list[str]:
    # Exact reviewed step digests cover existing heredocs and shell expansions.
    # New steps support only literal simple commands, with POSIX quote decoding.
    errors = []
    for line in run.replace("\\\n", "").splitlines():
        if any(token in line for token in ("$", "`", "\x00")):
            errors.append("unsupported dynamic shell command or toolchain substitution")
            continue
        try:
            lexer = shlex.shlex(line, posix=True, punctuation_chars=True)
            lexer.whitespace_split, lexer.commenters = True, "#"
            tokens = list(lexer)
        except ValueError:
            return ["unsupported shell quoting"]
        if not tokens:
            continue
        controls = {";", "&&", "||", "|", "&", "(", ")", "<", ">", ">>", "<<"}
        if any(t in controls for t in tokens):
            errors.append("unsupported shell control or redirection")
            tokens = tokens[:next(i for i, t in enumerate(tokens) if t in controls)]
        if tokens[0].startswith("RUSTUP_TOOLCHAIN="):
            if tokens.pop(0) != "RUSTUP_TOOLCHAIN=" + TOOLCHAIN:
                errors.append("shell toolchain override is not exact")
        if tokens[:2] == ["rustup", "run"]:
            if len(tokens) < 4 or tokens[2] != TOOLCHAIN:
                errors.append("rustup run selects an unpinned toolchain")
                continue
            tokens = tokens[3:]
        if not tokens or tokens.pop(0) != "cargo":
            errors.append("unsupported shell executable")
            continue
        if tokens and tokens[0].startswith("+"):
            if tokens.pop(0) != "+" + TOOLCHAIN:
                errors.append("cargo selects an unpinned toolchain")
        if not tokens:
            errors.append("unsupported Cargo command")
            continue
        command, *args = tokens
        if command == "generate-lockfile":
            errors.append("automatic lock generation is forbidden")
        if command in CARGO_LOCKED:
            if "--locked" not in args:
                errors.append(f"cargo {command} requires --locked")
        elif command not in CARGO_READ_ONLY:
            errors.append(f"unsupported Cargo command: {command}")
    return errors


# Complete reviewed steps include condition/shell/with/env attributes and body.
# Changing a complex build script requires review of its new digest in the PR.
REVIEWED_STEPS = {
    'ci.yml': frozenset((
        '05b5016d0dbda5cd85c06c4fd8d37d25d42c61e127abcd6c2c6900c1b317b8e2',
        '069d5d29a124985e9667c65720f7bbe772ee732395de084430928f0da6b7f21d',
        '0afa372251664252e9f6e06ecc7f9428317caf769e5b21dbcac6a64ec3b6d124',
        '0c5ce7d02ebbc4fc5bd88fc5c49a612dbd158508a4c75dd04a2305bf4b878e73',
        '22f39a9ffa1461a359edf3dd872d8333133939bbb8624a7764d6b60dc509decc',
        '483f544e1f470761204097a0457ca004c309516d2cb87fa72dd50616c19cd8bd',
        '50c3901dd8979bd533772017d91c14501e39dd61d885163aead5c533748e667f',
        '52e84492baaad095b9d64104a36498c519623b784b500aaba1e012d037380fd6',
        '693467b8ec5ee7b087bfbe85468ad611be5dbad1863912861438b6accf0e9e09',
        '6f929a372c6f48e98aa3a7ec031ec1b3d0c59118bbc72f112197ce12dec9ce2d',
        '74afa132755d81e2f1a04a41cd9b69cfbee3a6bf3e10284bf849dad8a3106515',
        '758f2f3b3ebae29ef6818d2aa9b0eb006d0b015d0e3d32d2212f45be89e87f21',
        '8b982ad2e5049c5ba521e6eb2d20f2c59cd048a001d5abb6cf6b1b889860f81d',
        'a9b6c103140046e27ec459a17606ef0bec0ec5a9c778495d2a28dd58ca1b0f28',
        'be7fca0a63238911dc4902147243d5e805472f14dbc0d011a11bcab0eaa21ea8',
        'c8a4ec17087dcb4588d39a0b583af55b84978a346c2ebe2ceedf60e91ef927af',
        'ddf38305ea4a215feddb6646e136aec5f74e93ec761f71f90f6d5070508a430f',
        'dfd36b50b3f0532b2c653b8dc31c7c1fee7dcbb0883ec347a91aad9c8f4a1fb7',
        'ec4df22d439ddaa8b5ec35ee753434bd5baa5a8a10df33865f8d507c93c63026',
        'f5a004bcd710de40a1805e0cff5e7a0b29037f5cb27c4fc8fef44bf95d912e5c',
        'fc5a7074a7f127d88a251e9c3821823eb8aa7b427e33c5ae59dcb4832a3d9d66',
    )),
    'dependency-audit.yml': frozenset((
        '05b5016d0dbda5cd85c06c4fd8d37d25d42c61e127abcd6c2c6900c1b317b8e2',
        '069d5d29a124985e9667c65720f7bbe772ee732395de084430928f0da6b7f21d',
        '2583feef0d32af80f8acb9bf662555fb659665a7edb0823f39a8f462b23d4779',
        '483f544e1f470761204097a0457ca004c309516d2cb87fa72dd50616c19cd8bd',
        '7b3ca07bc6ae0987e6820f0fe074f40a2931c9e6a5500c2da4a9bd21e52d241a',
        'bdd1cf521c3530c50bac7df331b870bca81763b5b5e4e247f8589128ac60070e',
        'c62d4a160a47679bb413e5daea6777bc7e3c6494df96b6cc041afb7d23780626',
        'c8a4ec17087dcb4588d39a0b583af55b84978a346c2ebe2ceedf60e91ef927af',
        'e4f1441ca090df40f71496c7e802e18479dffb81a63bcf4e6997a92c3be828f2',
        'ec4df22d439ddaa8b5ec35ee753434bd5baa5a8a10df33865f8d507c93c63026',
        'f4ae743e7253e1f7c543345a54018c5362231578804a8cdec129acbbcadeb005',
        'f5a004bcd710de40a1805e0cff5e7a0b29037f5cb27c4fc8fef44bf95d912e5c',
    )),
    'notify-merge.yml': frozenset((
        '61b68522421c3aba2b8197a1263215fa46824c0c4bb97f2e9d57a1f41adc88cb',
    )),
    'offline-build-kit.yml': frozenset((
        '05b5016d0dbda5cd85c06c4fd8d37d25d42c61e127abcd6c2c6900c1b317b8e2',
        '0a547bcb4c97842f36c146eae71afc8e439dd2fd4e7cb2889bde0c54234027d9',
        '47988a559fadd60810d186b646f1e4978314f492cc833b35c94b0a788ae2c5d7',
        '483f544e1f470761204097a0457ca004c309516d2cb87fa72dd50616c19cd8bd',
        'c8a4ec17087dcb4588d39a0b583af55b84978a346c2ebe2ceedf60e91ef927af',
        'e2343eb2a823313c9728de08c51bef5e8867cfcf8733e9dcb8b2a2184bd57287',
        'ec4df22d439ddaa8b5ec35ee753434bd5baa5a8a10df33865f8d507c93c63026',
        'f5a004bcd710de40a1805e0cff5e7a0b29037f5cb27c4fc8fef44bf95d912e5c',
    )),
}


# These original CI gates must exist once and keep their relative order.
# Their digests bind the complete execution attributes, environment and script.
REQUIRED_CI_GATES = (('Workspace smoke', '693467b8ec5ee7b087bfbe85468ad611be5dbad1863912861438b6accf0e9e09'), ('Quality gate', '22f39a9ffa1461a359edf3dd872d8333133939bbb8624a7764d6b60dc509decc'))


def _workflow_errors(name: str, text: str) -> list[str]:
    errors = []
    overrides = [l for l in text.splitlines() if re.match(r"\s*['\"]?RUSTUP_TOOLCHAIN['\"]?\s*:", l)]
    if name in BUILD_WORKFLOWS and overrides != [f"      RUSTUP_TOOLCHAIN: '{TOOLCHAIN}'"]:
        errors.append(f"{name}: unsupported duplicate/step toolchain override")
    if name in BUILD_WORKFLOWS and any(re.match(r"    (?:if|continue-on-error|defaults)\s*:", l) for l in text.splitlines()):
        errors.append(f"{name}: unsupported job execution condition/default")
    try:
        jobs, steps = _parse_workflow(text)
    except ValueError as error:
        return errors + [f"{name}: {error}"]
    if name in BUILD_WORKFLOWS:
        if jobs != [dict(zip(BUILD_WORKFLOWS, ("verify", "audit", "kit")))[name]]:
            errors.append(f"{name}: unsupported job structure")
        prefix = [
            {"uses": "actions/checkout@" + ACTIONS["actions/checkout"]},
            {"name": "Require committed lock", "run": LOCK_GATE},
            {"uses": "actions/setup-python@" + ACTIONS["actions/setup-python"], "with": {"python-version": "3.12"}},
            {"name": "Build input contract", "id": "build_inputs", "run": INPUT_GATE},
            {"name": "Exact Rust toolchain", "run": RUST_INSTALL + "\nrustc --version --verbose\ncargo --version\npython3 scripts/check_build_inputs.py --check-runtime"},
        ]
        for i, expected in enumerate(prefix):
            if i >= len(steps) or steps[i] != expected:
                reason = "committed lock gate must be the first run step" if i == 1 else "critical step/condition/shell/Python precondition differs"
                errors.append(f"{name}: {reason}")
    if name == "ci.yml":
        digests = [_step_digest(step) for step in steps]
        positions = []
        for gate_name, digest in REQUIRED_CI_GATES:
            if digests.count(digest) != 1:
                errors.append(f"{name}: unsupported required {gate_name} differs or is missing/duplicated")
            else:
                positions.append(digests.index(digest))
        if len(positions) == len(REQUIRED_CI_GATES) and positions != sorted(positions):
            errors.append(f"{name}: unsupported required CI gate order")
    for step in steps:
        if "uses" in step:
            repo, sep, commit = str(step["uses"]).partition("@")
            if not sep or ACTIONS.get(repo) != commit:
                errors.append(f"{name}: unapproved or mutable Action reference: {step['uses']}")
        if _step_digest(step) in REVIEWED_STEPS.get(name, frozenset()):
            continue
        if "uses" in step:
            errors.append(f"{name}: unsupported Action step attributes")
        elif set(step) not in ({"run"}, {"name", "run"}):
            errors.append(f"{name}: unsupported step execution attributes")
        elif "run" in step:
            errors.extend(f"{name}: {e}" for e in _shell_errors(str(step["run"])))
        else:
            errors.append(f"{name}: unsupported step")
    return errors


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate build manifest key")
        result[key] = value
    return result


def inspect_tree(root: Path, *, runtime: bool = False, kit: Path | None = None) -> tuple[list[str], dict[str, object]]:
    errors: list[str] = []
    info: dict[str, object] = {"schema_version": "rsia.build_inputs.v1", "toolchain": TOOLCHAIN, "actions": ACTIONS}
    try:
        head = _git(root, "rev-parse", "--verify", "HEAD").decode().strip()
        lock = _read_regular(root / "Cargo.lock")
        entry = _git(root, "ls-tree", head, "--", "Cargo.lock").decode().strip()
        if not entry.startswith("100644 blob ") or not entry.endswith("\tCargo.lock"):
            raise ValueError("Cargo.lock must be a committed ordinary file")
        if lock != _git(root, "show", f"{head}:Cargo.lock"):
            raise ValueError("Cargo.lock differs from the pinned source HEAD")
        if b"\r\n" in lock:
            raise ValueError("Cargo.lock contains CRLF")
        info.update(source_sha=head, cargo_lock_sha256=hashlib.sha256(lock).hexdigest())
    except (OSError, UnicodeError, ValueError, subprocess.TimeoutExpired) as error:
        errors.append(str(error))
    try:
        data = tomllib.loads(_read_regular(root / "rust-toolchain.toml").decode())
        if set(data) != {"toolchain"} or not isinstance(data["toolchain"], dict):
            raise ValueError("unsupported toolchain configuration")
        toolchain = data["toolchain"]
        if set(toolchain) != {"channel", "components", "profile"}:
            raise ValueError("unsupported toolchain fields")
        if toolchain["channel"] != TOOLCHAIN or toolchain["profile"] != "minimal":
            raise ValueError(f"toolchain must be exact {TOOLCHAIN} with minimal profile")
        if toolchain["components"] != ["rustfmt", "clippy"]:
            raise ValueError("toolchain must retain rustfmt and clippy")
        override = os.environ.get("RUSTUP_TOOLCHAIN")
        if override and override != TOOLCHAIN:
            raise ValueError("RUSTUP_TOOLCHAIN overrides the pinned toolchain")
    except (OSError, UnicodeError, ValueError, KeyError) as error:
        errors.append(str(error))
    directory = root / ".github/workflows"
    try:
        paths = sorted(list(directory.glob("*.yml")) + list(directory.glob("*.yaml")))
        if not set(BUILD_WORKFLOWS).issubset({p.name for p in paths}):
            errors.append("required build workflow missing")
        for path in paths:
            errors.extend(_workflow_errors(path.name, _read_regular(path).decode()))
    except (OSError, UnicodeError, ValueError) as error:
        errors.append(str(error))
    if runtime and not errors:
        # rustup run without --install refuses a missing toolchain; never installs.
        try:
            result = subprocess.run(
                ["rustup", "run", TOOLCHAIN, "rustc", "--version", "--verbose"],
                capture_output=True, text=True, timeout=15, check=False,
            )
            fields = dict(line.split(": ", 1) for line in result.stdout.splitlines() if ": " in line)
            if result.returncode or fields.get("release") != TOOLCHAIN or not re.fullmatch(r"[0-9a-f]{40}", fields.get("commit-hash", "")) or not fields.get("host"):
                raise ValueError("exact installed Rust toolchain runtime not verified")
            info["rustc"] = {k: fields[k] for k in ("release", "commit-hash", "host")}
        except (OSError, ValueError, subprocess.TimeoutExpired) as error:
            errors.append(str(error))
    if kit is not None and not errors:
        try:
            manifest = json.loads(_read_regular(kit / "build-inputs.json"), object_pairs_hook=_unique_object)
            if not isinstance(manifest, dict) or set(manifest) != {"schema_version", "source_sha", "cargo_lock_sha256", "toolchain", "actions", "rustc"}:
                raise ValueError("unsupported offline build manifest shape")
            for key in ("schema_version", "source_sha", "cargo_lock_sha256", "toolchain", "actions"):
                if manifest.get(key) != info.get(key):
                    raise ValueError(f"offline build manifest {key} differs from source")
            compiler = manifest.get("rustc", {})
            if not isinstance(compiler, dict) or compiler.get("release") != TOOLCHAIN or not re.fullmatch(r"[0-9a-f]{40}", compiler.get("commit-hash", "")) or not isinstance(compiler.get("host"), str) or not compiler["host"]:
                raise ValueError("offline build manifest lacks exact compiler identity")
            if runtime and compiler != info["rustc"]:
                raise ValueError("offline compiler identity differs from installed toolchain")
            if hashlib.sha256(_read_regular(kit / "Cargo.lock")).hexdigest() != info["cargo_lock_sha256"]:
                raise ValueError("offline Cargo.lock differs from source lock digest")
        except (OSError, ValueError, TypeError, KeyError) as error:
            errors.append(str(error))
    return errors, info


def build_input_errors(root: Path) -> list[str]:
    return inspect_tree(root)[0]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--check-runtime", action="store_true")
    parser.add_argument("--check-kit", type=Path)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    errors, info = inspect_tree(args.repo_root, runtime=args.check_runtime, kit=args.check_kit)
    if errors:
        for error in errors:
            print(f"BUILD_INPUTS_FAIL: {error}", file=sys.stderr)
        return 1
    print(json.dumps(info, sort_keys=True) if args.json else "BUILD_INPUTS_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
