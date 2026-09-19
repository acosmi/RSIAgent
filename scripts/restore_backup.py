#!/usr/bin/env python3
"""Offline, anchored restore. Unknown schemas or stale accounting remain isolated."""
from __future__ import annotations
import argparse
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
import uuid

BACKUP_SCHEMA = "rsia.backup.v2"
DELTA_SCHEMA = "rsia.revoke_delta.v1"
KNOWN_MIGRATIONS = [1, 2, 4, 5, 6]

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

def current_migrations():
    folder = Path(__file__).resolve().parents[1]/"crates/evo-storage/migrations"
    found = {}
    for path in folder.glob("*.sql"):
        version = int(path.name.split("_",1)[0])
        if version in found:
            raise IsolationError("duplicate code migration")
        found[version] = hashlib.sha384(path.read_bytes()).hexdigest()
    if sorted(found) != KNOWN_MIGRATIONS:
        raise IsolationError("unsupported current migration set")
    return found

def verify_db_schema(con, expected):
    rows = con.execute("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version").fetchall()
    if {v: bytes(c).hex() for v,c,ok in rows if ok==1} != expected or len(rows)!=len(expected):
        raise IsolationError("database migration checksums differ from current code")
    if con.execute("PRAGMA integrity_check").fetchone() != ("ok",):
        raise IsolationError("database integrity failed")

def verify_manifest(backup):
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
    if [m["version"] for m in migrations] != KNOWN_MIGRATIONS:
        raise IsolationError("unsupported manifest migrations")
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
    with readonly(backup/"rsia.sqlite3") as con:
        verify_db_schema(con,expected)
        if dict((n,(s,d)) for n,s,d in con.execute("SELECT namespace,seq,digest FROM revoke_watermark")) != watermarks:
            raise IsolationError("manifest/database watermark mismatch")
    return manifest,raw,watermarks

def protected_facts(con,namespaces):
    # Conservative restore: later consumption requires a new backup; never reset a ledger.
    facts=[]
    for ns,kind,id,body in con.execute("SELECT namespace,kind,id,body FROM objects ORDER BY namespace,kind,id"):
        if ns not in namespaces:
            continue
        value=read_json(body)
        schema=value.get("schema_version","")
        if kind in ("budget","reservation","evaluation","receipt") or schema in (
            "rsia.typed_artifact_envelope.v1","rsia.exploration_artifact_envelope.v1",
            "rsia.optimization.stage_fact.v1","rsia.budget_call_ref.v1"):
            facts.append((ns,kind,id,value))
    # Monetary state and irreversible dispatch counters must also agree.
    tables=[r[0] for r in con.execute("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'root_budget%'")]
    for table in sorted(tables):
        rows=con.execute('SELECT * FROM "'+table+'"').fetchall()
        facts.append((table,sorted(rows,key=repr)))
    return facts

def verify_delta(path,manifest_raw,base,anchor,backup,database=None):
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
    with readonly(anchor) as trusted, readonly(database or backup/"rsia.sqlite3") as old:
        trusted.execute("BEGIN")
        verify_db_schema(trusted,current_migrations())
        accounting=protected_facts(trusted,base)
        if accounting!=protected_facts(old,base):
            raise IsolationError("consumed query/alpha/dispatch/accounting facts changed; make a fresh backup")
        for entry in entries:
            ns=entry["namespace"]; seq,digest=base[ns]
            latest=trusted.execute("SELECT seq,digest FROM revoke_watermark WHERE namespace=?",(ns,)).fetchone()
            if latest is None or latest[0]<seq or (latest[0]==seq and latest[1]!=digest):
                raise IsolationError("missing or stale trusted revocation anchor")
            if (entry.get("base_seq"),entry.get("base_digest"))!=(seq,digest) or (entry.get("latest_seq"),entry.get("latest_digest"))!=latest:
                raise IsolationError("delta does not reach trusted latest watermark")
            latest_watermarks[ns]={"seq":latest[0],"digest":latest[1]}
            authoritative=[]
            for id,raw in trusted.execute("SELECT id,body FROM objects WHERE namespace=? AND kind='tombstone'",(ns,)):
                t=read_json(raw)
                required={"id","schema_version","source_kind","reason","watermark_seq","watermark_digest","created_at"}
                if set(t)!=required or t["schema_version"]!="rsia.revoke_tombstone.v1" or t["id"]!=id or t["source_kind"] not in ("run","artifact"):
                    raise IsolationError("unknown trusted tombstone schema")
                anchored_tombstones.append((ns,id,t))
                if t["watermark_seq"]<=seq:
                    row=old.execute("SELECT body FROM objects WHERE namespace=? AND kind='tombstone' AND id=?",(ns,id)).fetchone()
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
        "verified_at":datetime.now(timezone.utc).isoformat(),
        "scope":"Operator designates this SQLite path as the current trusted control plane. Verification covers backup namespaces, all anchored revocations and non-rewound consumed facts at this observed snapshot. It cannot authenticate a stale copy falsely designated as current; administrator switching remains separate."}

def replay_delta(database,events):
    with sqlite3.connect(database) as con:
        con.execute("BEGIN IMMEDIATE")
        for e in events:
            body={"id":e["source_id"],"schema_version":"rsia.revoke_tombstone.v1","source_kind":e["source_kind"],"reason":e["reason"],"watermark_seq":e["seq"],"watermark_digest":e["digest"],"created_at":e["created_at"]}
            con.execute("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,'tombstone',?,'restore',?) ON CONFLICT(namespace,kind,id) DO UPDATE SET body=excluded.body,revision=objects.revision+1",(e["namespace"],e["source_id"],json.dumps(body)))
            con.execute("UPDATE revoke_watermark SET seq=?,digest=? WHERE namespace=?",(e["seq"],e["digest"],e["namespace"]))
            identity=json.dumps([e["namespace"],e["source_kind"],e["source_id"],e["seq"],e["digest"]],ensure_ascii=False,separators=(",",":")).encode()
            job_id="revoke-"+hashlib.sha256(identity).hexdigest()[:32]
            con.execute("INSERT INTO revoke_cleanup_jobs(namespace,job_id,source_kind,source_id,watermark_seq,watermark_digest,state,created_at,updated_at) VALUES(?,?,?,?,?,?,'pending',?,?)",(e["namespace"],job_id,e["source_kind"],e["source_id"],e["seq"],e["digest"],e["created_at"],e["created_at"]))
            for kind in ("__budget_scan","__world_scan",e["source_kind"]):
                con.execute("INSERT INTO revoke_cleanup_frontier(namespace,job_id,node_kind,node_id) VALUES(?,?,?,?)",(e["namespace"],job_id,kind,e["source_id"]))
            con.execute("INSERT INTO revoke_cleanup_events(namespace,job_id,node_kind,node_id,event_kind,event_at,details) VALUES(?,?,?,?,?,?,?)",(e["namespace"],job_id,e["source_kind"],e["source_id"],"logical_block_restored",e["created_at"],json.dumps({"watermark_seq":e["seq"],"watermark_digest":e["digest"]})))

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

def main():
    parser=argparse.ArgumentParser()
    for name in ("backup","dest","revoke-delta"):
        parser.add_argument("--"+name,required=True)
    parser.add_argument("--trusted-revocations-db")
    args=parser.parse_args();backup=Path(args.backup);dest=Path(args.dest)
    if os.path.lexists(dest):
        print("dest exists; refusing overwrite",file=sys.stderr);return 2
    temp=dest.parent/f".{dest.name}.restore-{uuid.uuid4().hex}"
    try:
        reject_path_links(dest.parent)
        manifest,raw,base=verify_manifest(backup)
        events,anchor_receipt=verify_delta(Path(args.revoke_delta),raw,base,args.trusted_revocations_db,backup)
        temp.mkdir()
        for entry in [manifest["database"],*manifest["blobs"]]:
            source=safe_member(backup,entry["path"]);target=temp/entry["path"]
            target.parent.mkdir(parents=True,exist_ok=True)
            # Open final component with O_NOFOLLOW and verify the copied bytes again.
            fd=open_member(backup,entry["path"])
            with os.fdopen(fd,"rb") as reader, target.open("xb") as writer:
                info=os.fstat(reader.fileno())
                if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1:
                    raise IsolationError("member changed during copy")
                shutil.copyfileobj(reader,writer);writer.flush();os.fsync(writer.fileno())
            if target.stat().st_size!=entry["bytes"] or sha256_file(target)!=entry["sha256"]:
                raise IsolationError("copied member digest differs")
        # Re-read the independent anchor immediately before applying/publishing.
        latest_events,latest_anchor=verify_delta(Path(args.revoke_delta),raw,base,args.trusted_revocations_db,backup,temp/"rsia.sqlite3")
        if events!=latest_events or anchor_receipt["control_plane_digest"]!=latest_anchor["control_plane_digest"]:
            raise IsolationError("trusted anchor changed during restore")
        anchor_receipt=latest_anchor
        replay_delta(temp/"rsia.sqlite3",events)
        with (temp/"restore.json").open("x") as receipt:
            receipt.write(json.dumps({"schema_version":"rsia.restore_receipt.v2","backup_manifest_sha256":hashlib.sha256(raw).hexdigest(),"replayed_revoke_events":len(events),"isolated":False,"trusted_anchor":anchor_receipt,"receipt_scope":"local_only_not_for_export"})+"\n")
            receipt.flush();os.fsync(receipt.fileno())
        for directory,_,_ in os.walk(temp,topdown=False):
            directory_fd=os.open(directory,os.O_RDONLY|os.O_DIRECTORY)
            try: os.fsync(directory_fd)
            finally: os.close(directory_fd)
        publish_exclusive(temp,dest)
    except (IsolationError,OSError,sqlite3.Error,ValueError,KeyError,TypeError) as error:
        shutil.rmtree(temp,ignore_errors=True)
        print(f"ISOLATE {error}; not mounting",file=sys.stderr);return 1
    print(f"RESTORE_OK events={len(events)}");return 0

if __name__=="__main__":
    raise SystemExit(main())
