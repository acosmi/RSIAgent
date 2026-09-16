"""One-shot, hash-bound import of this repository's locally verified source."""
import hashlib, io, os, pathlib, shutil, subprocess, tarfile
ROOT = pathlib.Path.cwd().resolve()
BRANCH = 'implementation/v3-h00-reconcile-20260916'
if os.environ.get('GITHUB_REPOSITORY') != 'acosmi/RSIAgent' or os.environ.get('GITHUB_REF_NAME') != BRANCH:
    raise SystemExit('repository/branch mismatch')
parts = sorted((ROOT / '.delivery/h00').glob('*.bin'))
if [p.name for p in parts] != [f'{n:03}.bin' for n in range(17)]:
    raise SystemExit('missing or duplicate chunk')
raw = b''.join(p.read_bytes() for p in parts)
if len(raw) != 66036 or hashlib.sha256(raw).hexdigest() != 'e5aa13a11dc8414cf95578f0b34457fe1eba4f0a318fc03b5c2b0b5b364e63c8':
    raise SystemExit('source payload checksum mismatch')
root_files = {'.gitattributes','.gitignore','Cargo.toml','Cargo.lock','rust-toolchain.toml'}
with tarfile.open(fileobj=io.BytesIO(raw), mode='r:xz') as archive:
    members = archive.getmembers()
    if len(members) != 45 or sum(m.size for m in members) > 4 * 1024 * 1024:
        raise SystemExit('source limits exceeded')
    names = set()
    staged = []
    for member in members:
        path = pathlib.PurePosixPath(member.name)
        if not member.isfile() or path.is_absolute() or '..' in path.parts or member.name in names:
            raise SystemExit('unsafe source entry')
        names.add(member.name)
        if member.name not in root_files and path.parts[0] not in {'crates','apps','scripts','examples','.github'}:
            raise SystemExit('unexpected source path')
        if member.mode not in (0o644, 0o755):
            raise SystemExit('unexpected source permissions')
        content = archive.extractfile(member).read()
        if len(content) != member.size:
            raise SystemExit('truncated source entry')
        if path.parts[0] == '.github':
            continue
        target = ROOT.joinpath(*path.parts)
        if any(p.is_symlink() for p in [target, *target.parents]):
            raise SystemExit('symlink in destination')
        staged.append((target,content,member.mode))
    for path,content,mode in staged:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        path.chmod(mode)
shutil.rmtree(ROOT / '.delivery/h00')
subprocess.run(['git','config','user.name','github-actions[bot]'],check=True)
subprocess.run(['git','config','user.email','41898282+github-actions[bot]@users.noreply.github.com'],check=True)
subprocess.run(['git','add','-A'],check=True)
subprocess.run(['git','commit','-m','H00: reconcile verified 0.2.0 Rust runtime; preserve current README'],check=True)
subprocess.run(['git','push','origin',f'HEAD:refs/heads/{BRANCH}'],check=True)
print('source_commit=' + subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip())
