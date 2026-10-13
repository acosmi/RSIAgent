#!/usr/bin/env python3
"""Closed production Store::open inventory. No execution or database access.

This lexically checks reviewed source call sites and guard order. It is not a
proof about arbitrary Rust macro expansion or hostile changes to the checker.
"""
from __future__ import annotations
import argparse
import re
from pathlib import Path

RAW_START = re.compile('(?:br|cr|r)(#*)"')
CHAR_START = re.compile("'(?:\\\\(?:u\\{[0-9a-fA-F]+\\}|x[0-9a-fA-F]{2}|.)|[^'\\\\])'")

SITES = {
    ("apps/rsia/src/main.rs", "bootstrap"): 1,
    ("crates/evo-engine/src/bin/formal_local_host.rs", "run"): 1,
    ("crates/evo-engine/src/bin/formal_local_host.rs", "authority"): 1,
}


def blank(text: str) -> str:
    return "".join("\n" if c == "\n" else " " for c in text)


def lexical_code(text: str) -> str:
    out = list(text)
    i = 0
    while i < len(text):
        end = None
        if text.startswith("//", i):
            end = text.find("\n", i)
            if end < 0: end = len(text)
        elif text.startswith("/*", i):
            depth, cursor = 1, i + 2
            while cursor < len(text) and depth:
                if text.startswith("/*", cursor): depth += 1; cursor += 2
                elif text.startswith("*/", cursor): depth -= 1; cursor += 2
                else: cursor += 1
            if depth: raise ValueError("unterminated block comment")
            end = cursor
        else:
            raw = RAW_START.match(text, i)
            if raw:
                delimiter = '"' + raw.group(1)
                stop = text.find(delimiter, raw.end())
                if stop < 0: raise ValueError("unterminated raw string")
                end = stop + len(delimiter)
            elif text[i] == '"':
                cursor = i + 1
                while cursor < len(text):
                    if text[cursor] == "\\": cursor += 2
                    elif text[cursor] == '"': end = cursor + 1; break
                    else: cursor += 1
                if end is None: raise ValueError("unterminated string")
            elif text[i] == "'":
                char = CHAR_START.match(text, i)
                if char: end = char.end()
        if end is not None:
            out[i:end] = blank(text[i:end])
            i = end
        else: i += 1
    return "".join(out)


def block_end(code: str, start: int) -> int:
    depth = 0
    for i in range(start, len(code)):
        if code[i] == "{": depth += 1
        elif code[i] == "}":
            depth -= 1
            if depth == 0: return i + 1
    raise ValueError("unbalanced source block")


def production_code(text: str) -> str:
    code = lexical_code(text)
    pattern = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
    while match := pattern.search(code):
        body = code.find("{", match.end())
        semi = code.find(";", match.end())
        if semi >= 0 and (body < 0 or semi < body): end = semi + 1
        elif body >= 0: end = block_end(code, body)
        else: raise ValueError("unsupported cfg(test) item")
        code = code[:match.start()] + blank(code[match.start():end]) + code[end:]
    return code


def functions(code: str) -> list[tuple[str, int, int]]:
    result = []
    for match in re.finditer(r"\bfn\s+([A-Za-z_]\w*)\s*(?:<[^{}]*?>)?\s*\(", code):
        body = code.find("{", match.end())
        semi = code.find(";", match.end())
        if body >= 0 and (semi < 0 or body < semi):
            result.append((match.group(1), body, block_end(code, body)))
    return result


def check_tree(root: Path) -> list[str]:
    errors = []
    counts = {site: 0 for site in SITES}
    contents = {}
    paths = sorted(list(root.glob("crates/*/src/**/*.rs")) + list(root.glob("apps/*/src/**/*.rs")))
    for path in paths:
        relative = path.relative_to(root).as_posix()
        try: code = production_code(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, ValueError) as exc:
            errors.append(f"{relative}: cannot inspect source: {exc}"); continue
        contents[relative] = code
    names = {"Store"}
    joined = "\n".join(contents.values())
    while True:
        previous = set(names)
        for name in previous:
            names.update(re.findall(r"\b" + name + r"\s+as\s+([A-Za-z_]\w*)", joined))
            names.update(re.findall(r"\btype\s+([A-Za-z_]\w*)\s*=\s*(?:[A-Za-z_]\w*\s*::\s*)*" + name + r"\s*;", joined))
        if names == previous: break
    regex = r"\b(" + "|".join(sorted(names)) + r")\s*(?:>\s*)?::\s*open\b"
    for relative, code in contents.items():
        spans = functions(code)
        for match in re.finditer(regex, code):
            owners = [(name, start, end) for name, start, end in spans if start < match.start() < end]
            owner = min(owners, key=lambda x: x[2]-x[1]) if owners else ("<outside function>",0,len(code))
            key = (relative, owner[0])
            if match.group(1) != "Store" or key not in SITES or not re.match(r"\s*\(", code[match.end():]):
                errors.append(f"{relative}:{owner[0]}: unregistered or aliased Store::open"); continue
            counts[key] += 1
            body = code[owner[1]:owner[2]]
            offset = match.start() - owner[1]
            if relative.endswith("formal_local_host.rs"):
                gate = re.search(r"\bmount_gate\s*\(", body)
                if gate is None or gate.start() >= offset:
                    errors.append(f"{relative}:{owner[0]}: open precedes shared gate/lock")
                normalized_body = re.sub(r"\s+", "", body)
                argument = "&database" if owner[0] == "authority" else "database"
                guarded = "let(_data_lock,gate)=mount_gate(std::path::Path::new(" + argument + "),None).await"
                opening = "letstore=Store::open(std::path::Path::new(" + argument + ")).await?;"
                if guarded not in normalized_body or opening not in normalized_body:
                    errors.append(f"{relative}:{owner[0]}: gate and open must use the same database")
                if len(re.findall(r"\b_data_lock\b", body)) != 1:
                    errors.append(f"{relative}:{owner[0]}: guard must remain held to function return")
                if gate is not None and body[:gate.start()].count("{") - body[:gate.start()].count("}") != 1:
                    errors.append(f"{relative}:{owner[0]}: guard must be owned by the function scope")
                if not re.search(r"\bgate\s*\.\s*admit\s*\(", body[offset:]):
                    errors.append(f"{relative}:{owner[0]}: missing admission")
                if not re.search(r"\bstore\s*\.\s*close\s*\(\s*\)\s*\.\s*await", body[offset:]):
                    errors.append(f"{relative}:{owner[0]}: missing close before guard release")
                if owner[0] == "authority":
                    socket = re.search(r"UnixListener\s*::\s*bind\s*\(", body)
                    if socket is not None and (gate is None or socket.start() < gate.start()):
                        errors.append(f"{relative}: authority socket precedes shared gate/lock")
    for site, expected in counts.items():
        if expected != SITES[site]: errors.append(f"{site}: expected {SITES[site]} open, found {expected}")
    cli = contents.get("apps/rsia/src/main.rs", "")
    mounts = re.findall(r"mount_gate\s*\(\s*&data[^;]*;\s*let\s+\(\s*service\s*,\s*prepare_config\s*\)\s*=\s*bootstrap\s*\(\s*&data", cli)
    if len(mounts) != 2: errors.append("CLI serve/stdio must both mount before bootstrap")
    if len(re.findall(r"\bbootstrap\s*\(", cli)) != 3:
        errors.append("CLI bootstrap must have only the two registered callers and its definition")
    if "evo_engine::data_directory_lock::mount_gate" not in cli:
        errors.append("CLI must use the shared mount gate")
    shared = contents.get("crates/evo-engine/src/data_directory_lock.rs", "")
    normalized = re.sub(r"\s+", "", shared)
    required = ["gate.evaluate().await?", "DataDirectoryLock::acquire(data)", "letdecision=gate.evaluate().await?", "data_directory(data)", "file.try_lock()", "directory.canonicalize()?", "directory.join(DATA_LOCK_FILE)", ".truncate(false)"]
    if any(token not in normalized for token in required): errors.append("shared admission/lock contract is absent")
    if normalized.find("gate.evaluate().await?") >= normalized.find("DataDirectoryLock::acquire(data)") or normalized.find("DataDirectoryLock::acquire(data)") >= normalized.find("letdecision=gate.evaluate().await?"):
        errors.append("shared preflight/lock/recheck order differs")
    storage = contents.get("crates/evo-storage/src/lib.rs", "")
    open_bodies = [storage[start:end] for name, start, end in functions(storage) if name == "open"]
    closing = "ifletErr(error)=sqlx::migrate!().run(&pool).await{pool.close().await;returnErr(internal(error));}"
    if len(open_bodies) != 1:
        errors.append("storage must have one reviewed Store::open implementation")
    else:
        body = re.sub(r"\s+", "", open_bodies[0])
        if ".connect_lazy_with(opts)" not in body or closing not in body:
            errors.append("Store::open must await pool close on connection or migration error")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, required=True)
    args = parser.parse_args()
    errors = check_tree(args.repo_root)
    for error in errors: print(error)
    if errors: return 1
    print("PRODUCTION_STORE_OPEN_OK sites=3 shared_lock=true")
    return 0

if __name__ == "__main__": raise SystemExit(main())
