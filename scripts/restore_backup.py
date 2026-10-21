#!/usr/bin/env python3
"""Offline, anchored restore. Unknown schemas or stale accounting remain isolated."""
from __future__ import annotations
import argparse
import fcntl
import struct
import subprocess
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import shutil
import sqlite3
import stat
import sys
import time
import uuid

BACKUP_SCHEMA = "rsia.backup.v2"
DELTA_SCHEMA = "rsia.revoke_delta.v1"
KNOWN_MIGRATIONS = [1, 2, 4, 5, 6, 7]
# A backup may stop at a strict prefix of KNOWN_MIGRATIONS only once it holds the
# root budget ledger: protected_facts compares the root_budget% tables 0005 creates.
ROOT_BUDGET_MIGRATION = 5
MIGRATIONS_RECORD_SCHEMA = "rsia.restore_migrations.v1"

class IsolationError(Exception):
    pass

def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise IsolationError("duplicate JSON object key")
        result[key] = value
    return result

def reject_nonstandard_number(value):
    raise IsolationError(f"non-finite JSON number: {value}")

def finite_json(value):
    if isinstance(value,float) and not math.isfinite(value):
        raise IsolationError("non-finite JSON number")
    if isinstance(value,dict):
        for item in value.values(): finite_json(item)
    elif isinstance(value,list):
        for item in value: finite_json(item)
    return value

def read_json(raw):
    return finite_json(json.loads(raw, object_pairs_hook=unique_object,
        parse_constant=reject_nonstandard_number))

def exact_object(value,keys,name):
    if not isinstance(value,dict) or set(value)!=set(keys):
        raise IsolationError(f"invalid {name} fields")

def exact_int(value,name,minimum=0):
    if type(value) is not int or value<minimum:
        raise IsolationError(f"invalid {name}")
    return value

def exact_string(value,name,allow_empty=False):
    if not isinstance(value,str) or (not allow_empty and not value):
        raise IsolationError(f"invalid {name}")
    return value

def exact_hex(value,length,name):
    exact_string(value,name)
    if len(value)!=length or any(c not in "0123456789abcdef" for c in value):
        raise IsolationError(f"invalid {name}")
    return value

def verify_file_entry(entry,name):
    exact_object(entry,("path","sha256","bytes"),name)
    exact_string(entry["path"],f"{name} path")
    exact_hex(entry["sha256"],64,f"{name} sha256")
    exact_int(entry["bytes"],f"{name} bytes")

def verify_migration_entry(entry):
    exact_object(entry,("version","description","checksum_hex","success"),"migration")
    exact_int(entry["version"],"migration version",1)
    exact_string(entry["description"],"migration description")
    exact_hex(entry["checksum_hex"],96,"migration checksum")
    if entry["success"] is not True:
        raise IsolationError("migration is not successful")

def verify_watermark_entry(entry):
    exact_object(entry,("namespace","seq","digest"),"watermark")
    exact_string(entry["namespace"],"watermark namespace")
    exact_int(entry["seq"],"watermark seq")
    exact_hex(entry["digest"],64,"watermark digest")

def verify_delta_event(event):
    exact_object(event,("seq","previous_digest","digest","source_kind","source_id",
        "tombstone_digest","reason","created_at"),"delta event")
    exact_int(event["seq"],"delta event seq",1)
    exact_hex(event["previous_digest"],64,"delta previous digest")
    exact_hex(event["digest"],64,"delta digest")
    if event["source_kind"] not in ("run","artifact"): raise IsolationError("unsupported delta source kind")
    exact_string(event["source_id"],"delta source id")
    exact_hex(event["tombstone_digest"],64,"tombstone digest")
    exact_string(event["reason"],"delta reason")
    exact_int(event["created_at"],"delta created_at")

def load_json(path):
    raw = path.read_bytes()
    value = read_json(raw)
    if not isinstance(value, dict):
        raise IsolationError("JSON document must be an object")
    return value, raw

def reject_path_links(path):
    absolute=Path(os.path.abspath(path))
    current=Path(absolute.anchor)
    for part in absolute.parts[1:]:
        current=current/part
        if stat.S_ISLNK(current.lstat().st_mode):
            raise IsolationError("symlink in supplied path ancestry")

def regular(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise IsolationError(f"linked or non-regular member: {path.name}")
    return info

def safe_member(root, relative):
    reject_path_links(root)
    if not isinstance(relative, str) or not relative or "\\" in relative:
        raise IsolationError("invalid member path")
    pure = PurePosixPath(relative)
    if pure.is_absolute() or any(p in ("", ".", "..") for p in relative.split("/")) or str(pure) != relative:
        raise IsolationError("noncanonical member path")
    path = root
    if not stat.S_ISDIR(root.lstat().st_mode):
        raise IsolationError("backup root is not a real directory")
    for index, part in enumerate(pure.parts):
        path = path / part
        info = path.lstat()
        if index < len(pure.parts)-1:
            if not stat.S_ISDIR(info.st_mode):
                raise IsolationError("linked or non-directory member ancestor")
        else:
            regular(path)
    return path

def open_member(root, relative):
    safe_member(root,relative)
    parts=PurePosixPath(relative).parts
    fd=os.open(root,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
    try:
        for part in parts[:-1]:
            child=os.open(part,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd)
            os.close(fd);fd=child
        return os.open(parts[-1],os.O_RDONLY|os.O_NOFOLLOW,dir_fd=fd)
    finally:
        os.close(fd)

def sha256_file(path):
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()

def readonly(path):
    reject_path_links(path)
    regular(path)
    con = sqlite3.connect(path.resolve().as_uri()+"?mode=ro", uri=True)
    con.execute("PRAGMA query_only=ON")
    return con

def sqlx_description(file_name):
    # sqlx-core 0.8.6 migrate/source.rs resolve_blocking, which sqlx::migrate! also uses:
    # split once at "_", strip the type suffix with trim_end_matches, then "_" -> " ".
    rest = file_name.split("_",1)[1]
    suffix = ".up.sql" if rest.endswith(".up.sql") else ".down.sql" if rest.endswith(".down.sql") else ".sql"
    while rest.endswith(suffix):
        rest = rest[:-len(suffix)]
    return rest.replace("_"," ")

def code_migrations():
    """version -> (sqlx description, file bytes, SHA-384 hex): each file read once."""
    folder = Path(__file__).resolve().parents[1]/"crates/evo-storage/migrations"
    found = {}
    for path in folder.glob("*.sql"):
        version = int(path.name.split("_",1)[0])
        if version in found:
            raise IsolationError("duplicate code migration")
        raw = path.read_bytes()
        found[version] = (sqlx_description(path.name), raw, hashlib.sha384(raw).hexdigest())
    if sorted(found) != KNOWN_MIGRATIONS:
        raise IsolationError("unsupported current migration set")
    return found

def current_migrations():
    return {version: checksum for version,(_,_,checksum) in code_migrations().items()}

def verify_db_schema(con, expected):
    rows = [(m["version"],bytes.fromhex(m["checksum_hex"]),m["success"]) for m in con["migrations"]] if isinstance(con,dict) else con.execute("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version").fetchall()
    if {v: bytes(c).hex() for v,c,ok in rows if ok==1} != expected or len(rows)!=len(expected):
        raise IsolationError("database migration checksums differ from current code")
    if (con["integrity"] if isinstance(con,dict) else [v[0] for v in con.execute("PRAGMA integrity_check")]) != ["ok"]:
        raise IsolationError("database integrity failed")

def missing_migrations(versions):
    """The versions a backup on `versions` lacks, in order; [] for the current set.

    Only the current set or a strict prefix of it that already holds the root
    budget ledger (0005) is restorable. A gap, an unknown or future version and an
    older prefix stay isolated."""
    if not versions or versions != KNOWN_MIGRATIONS[:len(versions)]:
        raise IsolationError("unsupported manifest migrations: not a prefix of the current migration set")
    if versions[-1] < ROOT_BUDGET_MIGRATION:
        raise IsolationError("backup predates the root budget ledger (0005); protected facts cannot be verified")
    return KNOWN_MIGRATIONS[len(versions):]

def verify_backup_migrations(con, versions, expected):
    """The backup database holds exactly the manifest's ledger, every row successful
    with the current file's checksum, and passes the integrity check."""
    rows = [(m["version"],m["success"]) for m in con["migrations"]] if isinstance(con,dict) else con.execute("SELECT version,success FROM _sqlx_migrations ORDER BY version").fetchall()
    if [version for version,_ in rows] != versions:
        raise IsolationError("backup database migrations differ from its manifest")
    if any(success!=1 for _,success in rows):
        raise IsolationError("backup database has an unsuccessful migration")
    verify_db_schema(con,{version: expected[version] for version in versions})

def verify_manifest_files(backup):
    manifest, raw = load_json(safe_member(backup,"backup-manifest.json"))
    exact_object(manifest,("schema_version","complete","database","blobs","migrations",
        "watermarks","created_at"),"backup manifest")
    if manifest.get("schema_version")!=BACKUP_SCHEMA or manifest.get("complete") is not True:
        raise IsolationError("incomplete or unsupported backup")
    exact_int(manifest["created_at"],"backup created_at")
    verify_file_entry(manifest["database"],"database entry")
    if not isinstance(manifest["blobs"],list) or not isinstance(manifest["migrations"],list) or not isinstance(manifest["watermarks"],list):
        raise IsolationError("manifest collections must be arrays")
    for entry in manifest["blobs"]: verify_file_entry(entry,"blob entry")
    for entry in manifest["migrations"]: verify_migration_entry(entry)
    for entry in manifest["watermarks"]: verify_watermark_entry(entry)
    expected = current_migrations()
    migrations = manifest.get("migrations",[])
    versions = [m["version"] for m in migrations]
    # The current set, or a strict prefix the restore upgrades before publication.
    missing_migrations(versions)
    if any(m.get("success") is not True or m.get("checksum_hex")!=expected[m["version"]] for m in migrations):
        raise IsolationError("manifest migration checksums differ from current code")
    entries = [manifest["database"], *manifest["blobs"]]
    if entries[0].get("path")!="rsia.sqlite3":
        raise IsolationError("invalid database member")
    seen = {"backup-manifest.json"}
    for index,entry in enumerate(entries):
        relative = entry["path"]
        path = safe_member(backup,relative)
        if relative in seen or (index and (len(PurePosixPath(relative).parts)!=3 or not relative.startswith("blobs/"))):
            raise IsolationError("duplicate or invalid backup member")
        seen.add(relative)
        if regular(path).st_size != entry["bytes"] or sha256_file(path)!=entry["sha256"]:
            raise IsolationError("member bytes or digest mismatch")
        if index and path.name != entry["sha256"]:
            raise IsolationError("blob filename differs from content digest")
    # Reject unlisted files and every linked directory, rather than copying a tree.
    allowed_dirs = {str(parent) for name in seen for parent in PurePosixPath(name).parents if str(parent)!="."}
    for directory, dirs, files in os.walk(backup,followlinks=False):
        for name in dirs:
            p=Path(directory)/name
            if not stat.S_ISDIR(p.lstat().st_mode) or p.relative_to(backup).as_posix() not in allowed_dirs:
                raise IsolationError("linked or unlisted backup directory")
        for name in files:
            p=Path(directory)/name
            regular(p)
            if p.relative_to(backup).as_posix() not in seen:
                raise IsolationError("unlisted backup file")
    watermarks={}
    for row in manifest["watermarks"]:
        ns,seq,digest=row["namespace"],row["seq"],row["digest"]
        if ns in watermarks:
            raise IsolationError("invalid backup watermark")
        watermarks[ns]=(seq,digest)
    if not watermarks:
        raise IsolationError("missing backup watermark")
    return manifest,raw,watermarks

def verify_manifest(backup,snapshot):
    manifest,raw,watermarks=verify_manifest_files(backup)
    versions=[m["version"] for m in manifest["migrations"]]
    verify_backup_migrations(snapshot,versions,current_migrations())
    if {n:(s,d) for n,s,d in snapshot["watermarks"]}!=watermarks:
        raise IsolationError("manifest/database watermark mismatch")
    return manifest,raw,watermarks

def protected_facts(con,namespaces):
    # Conservative restore: later consumption requires a new backup; never reset a ledger.
    # The schema list below is mirrored by evo-storage lifecycle.rs (PROTECTED_SCHEMA_VERSIONS
    # plus PROTECTED_ACTION_SCHEMA_VERSIONS); the tests assert that the two stay equal.
    facts=[]
    selected = con["objects"] if isinstance(con,dict) else con.execute("SELECT namespace,kind,id,body FROM objects ORDER BY namespace,kind,id")
    for ns,kind,id,body in selected:
        if ns not in namespaces:
            continue
        value=read_json(body)
        schema=value.get("schema_version","")
        if kind in ("budget","reservation","evaluation","receipt") or schema in (
            "rsia.typed_artifact_envelope.v1","rsia.exploration_artifact_envelope.v1",
            "rsia.optimization.stage_fact.v1","rsia.budget_call_ref.v1",
            "rsia.budget_call_e16_ref.v1",
            "rsia.e16.export_attempt.v1","rsia.e16.delivery_audit.v1",
            "rsia.e16.export_attempt.v2","rsia.e16.delivery_audit.v2",
            "rsia.monitoring.consolidation_claim.v1","rsia.monitoring.consolidation_run.v1",
            "rsia.monitoring.consolidation_staging.v1","rsia.monitoring.consolidation_proposal.v1",
            "rsia.monitoring.environment_drift.v1",
            "rsia.practice_registration.v1","rsia.practice_registration_binding.v1",
            "rsia.practice_attempt_set.v1","rsia.curriculum_artifact_envelope.v1",
            "rsia.management_job.v1","rsia.replay_economic_artifact_envelope.v1"):
            facts.append((ns,kind,id,value))
    # Monetary state and irreversible dispatch counters must also agree.
    native_tables=dict(con["root_budget_tables"]) if isinstance(con,dict) else None
    tables=list(native_tables) if native_tables is not None else [r[0] for r in con.execute("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'root_budget%'")]
    for table in sorted(tables):
        rows=[tuple(_native_cell(v) for v in row) for row in native_tables[table]] if native_tables is not None else con.execute('SELECT * FROM "'+table+'"').fetchall()
        facts.append((table,sorted(rows,key=repr)))
    return facts

def verify_delta(path,manifest_raw,base,anchor,backup,trusted,old):
    if anchor is None:
        raise IsolationError("--trusted-revocations-db is required")
    anchor=Path(anchor)
    if anchor.resolve().is_relative_to(backup.resolve()) or os.path.samefile(anchor,backup/"rsia.sqlite3") or (anchor.parent/"backup-manifest.json").exists():
        raise IsolationError("revocation anchor must be independent of backup")
    delta,_=load_json(path)
    exact_object(delta,("schema_version","base_manifest_sha256","namespaces"),"revoke delta")
    exact_hex(delta["base_manifest_sha256"],64,"delta base manifest digest")
    if delta.get("schema_version")!=DELTA_SCHEMA or delta.get("base_manifest_sha256")!=hashlib.sha256(manifest_raw).hexdigest():
        raise IsolationError("delta schema or backup binding differs")
    entries=delta.get("namespaces",[])
    if not isinstance(entries,list): raise IsolationError("delta namespaces must be an array")
    for entry in entries:
        exact_object(entry,("namespace","base_seq","base_digest","events","latest_seq","latest_digest"),"delta namespace")
        exact_string(entry["namespace"],"delta namespace id")
        exact_int(entry["base_seq"],"delta base seq")
        exact_hex(entry["base_digest"],64,"delta base digest")
        exact_int(entry["latest_seq"],"delta latest seq")
        exact_hex(entry["latest_digest"],64,"delta latest digest")
        if not isinstance(entry["events"],list): raise IsolationError("delta events must be an array")
        for event in entry["events"]: verify_delta_event(event)
    if len(entries)!=len(base) or {e.get("namespace") for e in entries}!=set(base):
        raise IsolationError("delta namespaces differ")
    replay=[]
    latest_watermarks={}
    anchored_tombstones=[]
    verify_db_schema(trusted,current_migrations())
    accounting=protected_facts(trusted,base)
    if accounting!=protected_facts(old,base):
        raise IsolationError("consumed query/alpha/dispatch/accounting facts changed; make a fresh backup")
    for entry in entries:
        ns=entry["namespace"]; seq,digest=base[ns]
        latest=_native_watermark(trusted,ns)
        if latest is None or latest[0]<seq or (latest[0]==seq and latest[1]!=digest):
            raise IsolationError("missing or stale trusted revocation anchor")
        if (entry.get("base_seq"),entry.get("base_digest"))!=(seq,digest) or (entry.get("latest_seq"),entry.get("latest_digest"))!=latest:
            raise IsolationError("delta does not reach trusted latest watermark")
        latest_watermarks[ns]={"seq":latest[0],"digest":latest[1]}
        authoritative=[]
        for namespace,kind,id,raw in trusted["objects"]:
            if namespace!=ns or kind!="tombstone": continue
            t=read_json(raw)
            required={"id","schema_version","source_kind","reason","watermark_seq","watermark_digest","created_at"}
            if set(t)!=required or t["schema_version"]!="rsia.revoke_tombstone.v1" or t["id"]!=id or t["source_kind"] not in ("run","artifact"):
                raise IsolationError("unknown trusted tombstone schema")
            anchored_tombstones.append((ns,id,t))
            if t["watermark_seq"]<=seq:
                row=_native_object(old,ns,"tombstone",id)
                if row is None or read_json(row[0])!=t:
                    raise IsolationError("backup omits an already consumed revocation")
            else:
                authoritative.append((t,hashlib.sha256(raw.encode()).hexdigest()))
        authoritative.sort(key=lambda pair:pair[0]["watermark_seq"])
        expected=[];previous=digest
        for t,td in authoritative:
            expected.append({"seq":t["watermark_seq"],"previous_digest":previous,"digest":t["watermark_digest"],"source_kind":t["source_kind"],"source_id":t["id"],"tombstone_digest":td,"reason":t["reason"],"created_at":t["created_at"]})
            previous=t["watermark_digest"]
        if [e["seq"] for e in expected]!=list(range(seq+1,latest[0]+1)) or previous!=latest[1] or entry.get("events")!=expected:
            raise IsolationError("delta is incomplete or differs from trusted revocation events")
        replay.extend({"namespace":ns,**event} for event in expected)
    snapshot={"watermarks":latest_watermarks,"tombstones":sorted(anchored_tombstones,key=lambda row:(row[0],row[1])),"accounting":accounting}
    snapshot_bytes=json.dumps(snapshot,sort_keys=True,separators=(",",":"),default=lambda v:bytes(v).hex()).encode()
    return replay,{"path_local_only":str(anchor.resolve()),"watermarks":latest_watermarks,
        "control_plane_digest":hashlib.sha256(snapshot_bytes).hexdigest(),
        "verified_at":_admitted_datetime(trusted["wall"]).isoformat(),
        "scope":"Operator designates this SQLite path as the current trusted control plane. Verification covers backup namespaces, all anchored revocations and non-rewound consumed facts at this observed snapshot. It cannot authenticate a stale copy falsely designated as current; administrator switching remains separate."}

def replay_delta(database,events):
    raise IsolationError("SQLite replay requires the fixed native restore-copy operation")

def apply_missing_migrations(database,versions):
    if not missing_migrations(versions): return []
    raise IsolationError("SQLite migration requires the fixed native restore-copy operation")

def publish_exclusive(source,dest):
    libc=ctypes.CDLL(None,use_errno=True)
    if sys.platform=="darwin":
        result=libc.renamex_np(os.fsencode(source),os.fsencode(dest),4)
    elif sys.platform.startswith("linux") and hasattr(libc,"renameat2"):
        result=libc.renameat2(-100,os.fsencode(source),-100,os.fsencode(dest),1)
    else:
        raise IsolationError("atomic no-overwrite directory publication unavailable")
    if result:
        raise OSError(ctypes.get_errno(),"exclusive directory publication failed")


def _native_cell(cell):
    exact_object(cell,("type",) if cell.get("type")=="null" else ("type","value"),"native cell")
    kind=cell["type"]; value=cell.get("value")
    if kind=="null": return None
    if kind=="integer":
        if type(value) is not int or not -(1<<63)<=value<(1<<63): raise IsolationError("invalid native integer")
        return value
    if kind=="real_bits":
        exact_int(value,"native REAL bits")
        if value>=1<<64: raise IsolationError("invalid native REAL bits")
        return struct.unpack(">d",value.to_bytes(8,"big"))[0]
    if kind=="text": return exact_string(value,"native TEXT",True)
    if kind=="blob":
        if not isinstance(value,list) or any(type(v) is not int or not 0<=v<=255 for v in value):
            raise IsolationError("invalid native BLOB")
        return bytes(value)
    raise IsolationError("unknown native cell")

def _native_rows(snapshot,name):
    rows=snapshot[name]
    if not isinstance(rows,list): raise IsolationError("invalid native rows")
    return rows

def _native_watermark(snapshot,namespace):
    rows=[(seq,digest) for ns,seq,digest in _native_rows(snapshot,"watermarks") if ns==namespace]
    if len(rows)>1: raise IsolationError("duplicate native watermark")
    return rows[0] if rows else None

def _native_object(snapshot,namespace,kind,id):
    rows=[(body,) for ns,k,i,body in _native_rows(snapshot,"objects") if (ns,k,i)==(namespace,kind,id)]
    if len(rows)>1: raise IsolationError("duplicate native object")
    return rows[0] if rows else None

def _admitted_datetime(sample):
    exact_object(sample,("seconds","nanos"),"native wall")
    seconds=exact_int(sample["seconds"],"native wall seconds")
    nanos=exact_int(sample["nanos"],"native wall nanos")
    if seconds>=(1<<63) or nanos>=(1<<128) or nanos//1_000_000_000!=seconds:
        raise IsolationError("invalid native wall parts")
    try:
        return datetime.fromtimestamp(seconds,timezone.utc).replace(microsecond=(nanos%1_000_000_000)//1000)
    except (ValueError,OverflowError,OSError):
        raise IsolationError("unrepresentable native wall")

def _native_call(reader,operation,payload):
    reject_path_links(reader)
    before=regular(reader)
    if not reader.is_absolute() or before.st_uid!=os.geteuid() or before.st_mode&0o022 or not before.st_mode&0o111:
        raise IsolationError("insecure native reader")
    identity=(before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns,sha256_file(reader))
    nonce=str(uuid.uuid4())
    request=json.dumps({"version":1,"nonce":nonce,"payload":payload},ensure_ascii=False,separators=(",",":")).encode()+b"\n"
    flag={"restore_read":"--rsia-private-restore-read-v1","restore_copy":"--rsia-private-restore-copy-v1"}[operation]
    child=subprocess.Popen([str(reader),flag],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env={})
    try:
        output,error=child.communicate(request)
    except BaseException:
        try: child.kill()
        except ProcessLookupError: pass
        # communicate drains both actual pipes and waits for this actual child.
        child.communicate()
        raise
    after=regular(reader)
    if identity!=(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns,sha256_file(reader)):
        raise IsolationError("native reader changed")
    if not output.endswith(b"\n") or output.count(b"\n")!=1:
        raise IsolationError("truncated or trailing native output")
    reply=read_json(output)
    exact_object(reply,("version","nonce","pid","build","operation","request_sha256","result","error"),"native result")
    exact_hex(reply["build"],64,"native build")
    exact_int(reply["version"],"native version")
    exact_int(reply["pid"],"native pid",1)
    if (reply["version"],reply["nonce"],reply["pid"],reply["operation"],reply["request_sha256"])!=(1,nonce,child.pid,operation,hashlib.sha256(request).hexdigest()):
        raise IsolationError("native result binding differs")
    if child.returncode!=0 or reply["error"] is not None:
        fault=reply["error"]
        code=fault.get("code","invalid") if isinstance(fault,dict) else "invalid"
        raise IsolationError("native restore refused: "+code)
    if error or not isinstance(reply["result"],dict):
        raise IsolationError("invalid native terminal")
    return reply["result"]

def _native_read(reader,configs,database,extra=(),related=None):
    reject_path_links(database);regular(database)
    if related is not None:
        reject_path_links(related);regular(related)
    snapshot=_native_call(reader,"restore_read",{
        "path":list(os.fsencode(database.resolve())),"prepare_configs":configs,
        "extra_occurrences":list(extra),
        "related_path":list(os.fsencode(related.resolve())) if related is not None else None})
    exact_object(snapshot,("native_version","native_source_id","integrity","migrations","watermarks","objects","root_budget_tables","wall"),"native snapshot")
    if not snapshot["native_version"] or not snapshot["native_source_id"] or snapshot["integrity"]!=["ok"]:
        raise IsolationError("native snapshot refused")
    _admitted_datetime(snapshot["wall"])
    return snapshot

class _PrivateNativeCopy:
    """Created only after our actual native child's zero exit and two pipe EOFs."""
    def __init__(self,result):
        exact_object(result,("private_path","directory","database","applied","source_versions"),"private copy")
        value=result["private_path"]
        if not isinstance(value,list) or any(type(v) is not int or not 0<=v<=255 for v in value):
            raise IsolationError("invalid private path bytes")
        self.path=Path(os.fsdecode(bytes(value)))
        exact_object(result["directory"],("dev","ino","uid","mode"),"private directory")
        exact_object(result["database"],("dev","ino","bytes","sha256"),"private database")
        for key in ("dev","ino","uid","mode"): exact_int(result["directory"][key],"private directory "+key)
        for key in ("dev","ino","bytes"): exact_int(result["database"][key],"private database "+key)
        exact_hex(result["database"]["sha256"],64,"private database hash")
        self.directory=result["directory"]; self.database=result["database"]
        self.applied=result["applied"]; self.fd=None
        reject_path_links(self.path)
        self.fd=os.open(self.path,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
        try:
            self.check()
            info=regular(self.path/"rsia.sqlite3")
            if (info.st_dev,info.st_ino,info.st_size,sha256_file(self.path/"rsia.sqlite3"))!=(self.database["dev"],self.database["ino"],self.database["bytes"],self.database["sha256"]):
                raise IsolationError("native private database changed")
        except BaseException:
            self.close()
            raise
    def check(self):
        info=os.fstat(self.fd); current=self.path.lstat()
        if (info.st_dev,info.st_ino)!=(current.st_dev,current.st_ino) or (info.st_dev,info.st_ino,info.st_uid,stat.S_IMODE(info.st_mode))!=(self.directory["dev"],self.directory["ino"],os.geteuid(),0o700):
            raise IsolationError("native private directory changed")
    def before_fs(self,reader,configs,anchor,extra=()):
        self.check()
        # This sole O_EXCL private-copy FS owner holds real SH stability;
        # native DB writers still require their original exclusive lock.
        fcntl.flock(self.fd,fcntl.LOCK_SH|fcntl.LOCK_NB)
        try:
            self.check()
            snapshot=_native_read(reader,configs,self.path/"rsia.sqlite3",extra,anchor)
            self.check()
            return snapshot["wall"]
        except BaseException:
            self.unlock()
            raise
    def unlock(self):
        if self.fd is not None: fcntl.flock(self.fd,fcntl.LOCK_UN)
    def close(self):
        if self.fd is not None:
            os.close(self.fd); self.fd=None

def main():
    os.umask(0o077)
    parser=argparse.ArgumentParser()
    for name in ("backup","dest","revoke-delta","native-reader","prepare-configs-file"):
        parser.add_argument("--"+name,required=True)
    parser.add_argument("--trusted-revocations-db")
    args=parser.parse_args();backup=Path(args.backup);dest=Path(args.dest)
    if os.path.lexists(dest):
        print("dest exists; refusing overwrite",file=sys.stderr);return 2
    owner=None
    try:
        if sys.platform not in ("darwin","linux"): raise IsolationError("unsupported restore platform")
        reject_path_links(dest.parent)
        reader=Path(args.native_reader)
        configs_path=Path(args.prepare_configs_file)
        reject_path_links(configs_path);regular(configs_path)
        configs=read_json(configs_path.read_bytes())
        if not isinstance(configs,list): raise IsolationError("prepare originals must be an array")
        manifest,raw,base=verify_manifest_files(backup)
        if not args.trusted_revocations_db: raise IsolationError("--trusted-revocations-db is required")
        anchor=Path(args.trusted_revocations_db)
        backup_snapshot=_native_read(reader,configs,backup/"rsia.sqlite3",(manifest["created_at"],))
        verify_manifest(backup,backup_snapshot)
        trusted=_native_read(reader,configs,anchor,(manifest["created_at"],),backup/"rsia.sqlite3")
        events,anchor_receipt=verify_delta(Path(args.revoke_delta),raw,base,anchor,backup,trusted,backup_snapshot)
        delta,_=load_json(Path(args.revoke_delta))
        result=_native_call(reader,"restore_copy",{
            "backup":list(os.fsencode(backup.resolve())),
            "dest":list(os.fsencode(Path(os.path.abspath(dest)))),
            "anchor":list(os.fsencode(anchor.resolve())),
            "manifest_sha256":hashlib.sha256(raw).hexdigest(),
            "delta":delta,"prepare_configs":configs})
        owner=_PrivateNativeCopy(result)
        # Replayed bytes are still private. The original latest-anchor and
        # accounting assertions still precede every public receipt/publication.
        old=_native_read(reader,configs,owner.path/"rsia.sqlite3",(),anchor)
        trusted=_native_read(reader,configs,anchor,(),owner.path/"rsia.sqlite3")
        latest_events,latest_anchor=verify_delta(Path(args.revoke_delta),raw,base,anchor,backup,trusted,old)
        if events!=latest_events or anchor_receipt["control_plane_digest"]!=latest_anchor["control_plane_digest"]:
            raise IsolationError("trusted anchor changed during restore")
        anchor_receipt=latest_anchor
        applied=owner.applied
        owner.before_fs(reader,configs,anchor)
        try:
            fd=os.open(owner.path/"restore.json",os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
            with os.fdopen(fd,"w") as receipt:
                receipt.write(json.dumps({"schema_version":"rsia.restore_receipt.v2","backup_manifest_sha256":hashlib.sha256(raw).hexdigest(),"replayed_revoke_events":len(events),"isolated":False,"trusted_anchor":anchor_receipt,"receipt_scope":"local_only_not_for_export"})+"\n")
                receipt.flush();os.fsync(receipt.fileno())
        finally: owner.unlock()
        if applied:
            owner.before_fs(reader,configs,anchor)
            try:
                fd=os.open(owner.path/"restore.migrations.json",os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
                with os.fdopen(fd,"w") as record:
                    record.write(json.dumps({"schema":MIGRATIONS_RECORD_SCHEMA,"source_versions":[m["version"] for m in manifest["migrations"]],"applied":applied,"current_versions":KNOWN_MIGRATIONS})+"\n")
                    record.flush();os.fsync(record.fileno())
            finally: owner.unlock()
        for directory,_,_ in os.walk(owner.path,topdown=False):
            owner.before_fs(reader,configs,anchor)
            try:
                directory_fd=os.open(directory,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
                try: os.fsync(directory_fd)
                finally: os.close(directory_fd)
            finally: owner.unlock()
        owner.before_fs(reader,configs,anchor)
        try:
            publish_exclusive(owner.path,dest)
            owner.path=dest
            parent_fd=os.open(dest.parent,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
            try: os.fsync(parent_fd)
            finally: os.close(parent_fd)
        finally: owner.unlock()
    except (IsolationError,OSError,sqlite3.Error,ValueError,KeyError,TypeError,OverflowError) as error:
        # A clock/floor refusal grants no filesystem cleanup admission.
        # Keep the unpublished private copy; a later explicit cleanup must qualify.
        print(f"ISOLATE {error}; not mounting",file=sys.stderr);return 1
    finally:
        if owner is not None: owner.close()
    print(f"RESTORE_OK events={len(events)} migrations_applied={','.join(str(m['version']) for m in applied) or 'none'}");return 0

if __name__=="__main__":
    raise SystemExit(main())
