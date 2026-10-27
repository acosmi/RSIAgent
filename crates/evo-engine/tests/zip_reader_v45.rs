#[path = "support/zip_builder.rs"]
mod zip_builder;

use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::packages::{
    AssetPackageManifest, E16SourceRef, MAX_FILE, MAX_TOTAL, PackageEntry, PackageMember,
    PackageState, PersistentPackageStore, StagePackageRequest, StagedAssetState,
    import_external_package, reject_member, reject_package,
};
use evo_engine::zip_reader::*;
use evo_storage::Store;
use std::io::{Read, copy, sink};
use std::path::{Component, Path, PathBuf};
use zip_builder::{Entry, build, golden, manifest_for, package, set_manifest, set16, set32};

fn assert_code(label: &str, archive: &[u8], expected: &str) {
    match read_restricted_zip(archive) {
        Err(Error::Invalid(actual)) => assert_eq!(actual, expected, "{label}"),
        actual => panic!("{label}: expected {expected}, got {actual:?}"),
    }
}

fn changed_member(change: impl FnOnce(&mut Entry)) -> Vec<u8> {
    let mut entries = golden(0);
    change(&mut entries[2]);
    build(&entries).bytes
}

fn changed_manifest(change: impl FnOnce(&mut AssetPackageManifest)) -> Vec<u8> {
    let mut entries = golden(0);
    let mut manifest = manifest_for(&entries[1..]);
    change(&mut manifest);
    set_manifest(&mut entries, &manifest);
    build(&entries).bytes
}

fn zip_request(key: &str, archive: Vec<u8>, sources: &[E16SourceRef]) -> StageRestrictedZipRequest {
    StageRestrictedZipRequest {
        archive,
        request_key: key.into(),
        source_refs: sources.to_vec(),
        baseline_digest: hash(b"baseline"),
        local_digest: hash(b"local"),
        upstream_digest: Some(hash(b"upstream")),
        environment_digest: hash(b"environment"),
        compiler_version: "compiler-v1".into(),
        candidate_material: None,
    }
}

#[test]
fn happy() {
    let g0 = build(&golden(0)).bytes;
    // Independently serialized actual G0, with zlib.crc32 and ZIP header fields:
    // STORED; made_by=0x0314; needed=20; flags/time=0; date=33;
    // attributes=0o100644<<16; no extra/comment; manifest,prompt,skill order.
    assert_eq!(
        hash(&g0),
        "605ec25f97f429d71c419effed1964a1c7c5ebadc51481d29c1255fa49e0bc67",
        "P1 G0 actual byte SHA"
    );
    let result = read_restricted_zip(&g0).unwrap();
    assert_eq!(result.members.len(), 3, "P1 members include manifest");
    assert_eq!(result.files.len(), 2, "P1 entries");
    assert!(!result.files.contains_key("manifest.json"), "P1 reserved");
    assert_eq!(
        result.files["prompt.json"],
        zip_builder::PROMPT,
        "P1 prompt"
    );
    assert_eq!(result.files["skill.md"], zip_builder::SKILL, "P1 skill");
    for entry in &result.manifest.entries {
        assert_eq!(hash(&result.files[&entry.path]), entry.digest_sha256, "P1");
    }

    // P2 reproducible producer recipe: Python 3.12.14 / zlib 1.2.12.
    // Run the following Python in a directory containing the destination:
    // import hashlib, json, zipfile, zlib
    // files = [("prompt.json", b'{"instruction":"Be concise"}\n'),
    //          ("skill.md", b"# Skill\nFollow the prompt.\n")]
    // def raw(data):
    //     c = zlib.compressobj(6, zlib.DEFLATED, -15)
    //     return c.compress(data) + c.flush()
    // manifest = {
    //   "schema_version":"rsia.package.v1","kind":"skill","publisher":"org.rsia",
    //   "asset_id":"restricted_zip","name":"Restricted ZIP fixture","version":"1.0.0",
    //   "description":"Deterministic restricted ZIP package","license":"Apache-2.0",
    //   "entries":[{"path":n,"digest_sha256":hashlib.sha256(d).hexdigest(),
    //               "size_bytes":len(d),"compressed_bytes":len(raw(d))} for n,d in files],
    //   "dependencies":[],
    //   "redaction_report":{"scanned_at":0,"findings_count":0,"status":"clean",
    //     "disclaimer":"Privacy scan does not guarantee absolute absence of sensitive data; human audit is required.",
    //     "findings":[]},"foreign_metadata":None}
    // encoded = json.dumps(manifest, separators=(",",":"), ensure_ascii=False).encode()
    // with zipfile.ZipFile("ag173_interop_python_deflated.zip","w") as archive:
    //     for name,data in [("manifest.json",encoded),*files]:
    //         info = zipfile.ZipInfo(name,(1980,1,1,0,0,0))
    //         info.create_system = 3
    //         info.external_attr = 0o100644 << 16
    //         info.compress_type = zipfile.ZIP_DEFLATED
    //         archive.writestr(info,data,compress_type=zipfile.ZIP_DEFLATED,compresslevel=6)
    // Info-ZIP producers must use zip -X -D; default zip -r is outside this profile.
    let interop = include_bytes!("fixtures/ag173_interop_python_deflated.zip");
    assert_eq!(
        hash(interop),
        "6bf0eb8e52492f22322a291e67cd8e2918523d755b8d7762581e6685bc676b82",
        "P2 actual standard-tool fixture SHA"
    );
    let standard = read_restricted_zip(interop).unwrap();
    assert_eq!(standard.files, result.files, "P2 standard DEFLATE");
    assert!(
        standard.members.iter().all(|member| member.method == 8),
        "P2"
    );

    let hundred = (0..100)
        .map(|i| Entry::new(&format!("f{i:03}.md"), b"x", 0))
        .collect();
    assert_eq!(
        read_restricted_zip(&build(&package(hundred, 0)).bytes)
            .unwrap()
            .members
            .len(),
        101,
        "P3 manifest + 100"
    );
    let boundary = package(
        vec![Entry::new("large.md", &vec![b'a'; MAX_FILE as usize], 0)],
        0,
    );
    assert!(
        read_restricted_zip(&build(&boundary).bytes).is_ok(),
        "P3 exact member"
    );
    let mut files: Vec<_> = (0..10)
        .map(|i| Entry::new(&format!("p{i}.md"), &vec![b'a'; MAX_FILE as usize], 0))
        .collect();
    let total_boundary = loop {
        let entries = package(files.clone(), 0);
        let total: u64 = entries.iter().map(|entry| u64::from(entry.size)).sum();
        if total == MAX_TOTAL {
            break build(&entries).bytes;
        }
        let last = files.last_mut().unwrap();
        let wanted = (last.content.len() as i64 + MAX_TOTAL as i64 - total as i64) as usize;
        *last = Entry::new("p9.md", &vec![b'a'; wanted], 0);
    };
    let total = read_restricted_zip(&total_boundary).unwrap();
    assert_eq!(
        total.members.iter().map(|m| m.size).sum::<u64>(),
        MAX_TOTAL,
        "P3 exact total includes manifest"
    );

    let empty = package(vec![Entry::new("empty.md", b"", 0)], 0);
    assert!(
        read_restricted_zip(&build(&empty).bytes).unwrap().files["empty.md"].is_empty(),
        "P5"
    );
    for (name, content) in [
        ("技能.md".to_owned(), b"a\tb\nc\rd".as_slice()),
        (format!("{}.md", "a".repeat(252)), b"x".as_slice()),
    ] {
        let entries = package(vec![Entry::new(&name, content, 0)], 0);
        assert_eq!(
            read_restricted_zip(&build(&entries).bytes).unwrap().files[&name],
            content,
            "P6 {name}"
        );
    }
    assert_eq!(
        read_restricted_zip(&build(&golden(8)).bytes).unwrap().files,
        result.files,
        "P7"
    );
}

#[test]
fn structure() {
    let base = build(&golden(0));
    let mut large = base.bytes.clone();
    large.resize(MAX_ARCHIVE_BYTES as usize + 1, 0);
    assert_code("S1", &large, ARCHIVE_TOO_LARGE);
    assert_code("S2", &base.bytes[..21], MALFORMED);
    let mut trailing = base.bytes.clone();
    trailing.push(0);
    assert_code("S3", &trailing, MALFORMED);
    let mut comment = base.bytes.clone();
    set16(&mut comment, base.eocd + 20, 5);
    comment.extend_from_slice(b"hello");
    assert_code("S4", &comment, COMMENT);
    for (at, wide) in [(base.eocd + 10, false), (base.eocd + 16, true)] {
        let mut bytes = base.bytes.clone();
        if wide {
            set32(&mut bytes, at, u32::MAX);
        } else {
            set16(&mut bytes, at, u16::MAX);
        }
        assert_code("S5 sentinel", &bytes, ZIP64);
    }
    let mut locator = vec![0; 20];
    set32(&mut locator, 0, 0x0706_4b50);
    let mut bytes = base.bytes.clone();
    bytes.splice(base.eocd..base.eocd, locator);
    set32(
        &mut bytes,
        base.eocd + 20 + 12,
        (base.eocd - base.central[0] + 20) as u32,
    );
    assert_code("S5 locator", &bytes, ZIP64);
    for at in [base.eocd + 4, base.eocd + 8] {
        let mut bytes = base.bytes.clone();
        set16(&mut bytes, at, 1);
        assert_code("S6", &bytes, MULTI_DISK);
    }
    assert_code("S7", &build(&[]).bytes, MANIFEST_MISSING);
    let too_many = package(
        (0..101)
            .map(|i| Entry::new(&format!("f{i}.md"), b"x", 0))
            .collect(),
        0,
    );
    assert_code("S8", &build(&too_many).bytes, TOO_MANY_ENTRIES);
    let mut bytes = base.bytes.clone();
    set32(
        &mut bytes,
        base.eocd + 12,
        (base.eocd - base.central[0] + 1) as u32,
    );
    assert_code("S9 size", &bytes, MALFORMED);
    let mut bytes = base.bytes.clone();
    for at in [base.eocd + 8, base.eocd + 10] {
        set16(&mut bytes, at, 2);
    }
    assert_code("S9 count", &bytes, MALFORMED);
    for (at, padding) in [(0, 4), (base.locals[1], 1)] {
        let mut bytes = base.bytes.clone();
        bytes.splice(at..at, vec![b'!'; padding]);
        for (index, &offset) in base.locals.iter().enumerate() {
            let adjusted = offset + if offset >= at { padding } else { 0 };
            set32(
                &mut bytes,
                base.central[index] + padding + 42,
                adjusted as u32,
            );
        }
        set32(
            &mut bytes,
            base.eocd + padding + 16,
            (base.central[0] + padding) as u32,
        );
        assert_code("S10 prefix/gap", &bytes, MALFORMED);
    }
    let mut broken = base.bytes.clone();
    set32(&mut broken, base.central[2], 0);
    assert_code("S framing signature before E", &broken, MALFORMED);
}

#[test]
fn unsupported() {
    for flags in [1, 1 << 6, 1 << 13] {
        assert_code(
            "U1",
            &changed_member(|entry| entry.flags = flags),
            ENCRYPTION,
        );
    }
    assert_code(
        "U2",
        &changed_member(|entry| entry.flags = 8),
        DATA_DESCRIPTOR,
    );
    assert_code("U3", &changed_member(|entry| entry.flags = 1 << 12), FLAGS);
    for method in [12, 14] {
        assert_code(
            "U4",
            &changed_member(|entry| entry.method = method),
            COMPRESSION_METHOD,
        );
    }
    assert_code(
        "U5",
        &changed_member(|entry| entry.needed = 45),
        VERSION_NEEDED,
    );
    assert_code(
        "U6 central",
        &changed_member(|entry| entry.extra = vec![0x55, 0x54, 5, 0, 0, 0, 0, 0, 0]),
        EXTRA_FIELD,
    );
    assert_code(
        "U6 local",
        &changed_member(|entry| entry.local_extra = vec![0; 9]),
        EXTRA_FIELD,
    );
    assert_code(
        "U7",
        &changed_member(|entry| entry.comment = b"abc".to_vec()),
        COMMENT,
    );
    assert_code(
        "U8 name",
        &changed_member(|entry| *entry = Entry::new("material/", b"", 0)),
        DIRECTORY_ENTRY,
    );
    for attributes in [0x10, 0o040755 << 16] {
        assert_code(
            "U8 attributes",
            &changed_member(|entry| entry.attributes = attributes),
            DIRECTORY_ENTRY,
        );
    }
    assert_code(
        "U9",
        &changed_member(|entry| entry.compressed = u32::MAX),
        ZIP64,
    );
    assert_code(
        "U disk",
        &changed_member(|entry| entry.disk = 1),
        MULTI_DISK,
    );
}

#[test]
fn names() {
    for (label, name, code) in [
        ("N1", "../x.md", TRAVERSAL),
        ("N2", "a/../x.md", TRAVERSAL),
        ("N3", "dir\\..\\x.md", TRAVERSAL),
        ("N4", "/etc/x.md", ABSOLUTE),
        ("N5", "C:/x.md", ABSOLUTE),
        ("N5", "C:x.md", ABSOLUTE),
        ("N5", "a:b.md", ABSOLUTE),
        ("N6", "a\0b.md", BAD_NAME),
        ("N6", "a\u{1}b.md", BAD_NAME),
        ("N6", "a\u{7f}b.md", BAD_NAME),
        ("N7", "a\u{202e}b.md", BAD_NAME),
        ("N7", "a\u{200b}b.md", BAD_NAME),
        ("N7", "\u{feff}a.md", BAD_NAME),
        ("N9", "a//b.md", BAD_NAME),
        ("N9", "./a.md", BAD_NAME),
        ("N9", "a/./b.md", BAD_NAME),
        ("N9", "a./b.md", BAD_NAME),
        ("N11", "a b.md", BAD_NAME),
        ("N11", "a*b.md", BAD_NAME),
        ("N11", "a~b.md", BAD_NAME),
    ] {
        assert_code(
            label,
            &changed_member(|entry| {
                entry.name = name.as_bytes().to_vec();
                entry.flags = 0x800;
            }),
            code,
        );
    }
    for (name, flags) in [
        (b"caf\xc3\xa9.md".as_slice(), 0),
        (b"\xff.md".as_slice(), 0x800),
    ] {
        assert_code(
            "N8",
            &changed_member(|entry| {
                entry.name = name.to_vec();
                entry.flags = flags;
            }),
            BAD_NAME,
        );
    }
    assert_code(
        "N10",
        &changed_member(|entry| entry.name = vec![b'a'; 256]),
        BAD_NAME,
    );
    assert_code(
        "N1 empty length",
        &changed_member(|entry| entry.name.clear()),
        BAD_NAME,
    );
}

#[test]
fn duplicates() {
    for (label, first, second) in [
        ("D1", "skill.md", "skill.md"),
        ("D2", "Skill.md", "skill.md"),
        ("D4", "docs", "docs/a.md"),
        ("D4 reversed", "DOCS/a.md", "docs"),
    ] {
        let mut entries = golden(0);
        entries[1].name = first.as_bytes().to_vec();
        entries[2].name = second.as_bytes().to_vec();
        assert_code(label, &build(&entries).bytes, DUPLICATE);
    }
    assert_code(
        "D3",
        &changed_member(|entry| entry.name = b"MANIFEST.JSON".to_vec()),
        DUPLICATE,
    );
}

#[test]
fn types() {
    assert_code(
        "T1",
        &changed_member(|entry| {
            *entry = Entry::new("link.md", b"../../etc/passwd", 0);
            entry.attributes = 0o120777 << 16;
        }),
        SYMLINK,
    );
    assert_code(
        "T2",
        &changed_member(|entry| {
            entry.made_by = 20;
            entry.attributes = 0x400;
        }),
        SYMLINK,
    );
    for mode in [0o010644, 0o020644, 0o060644, 0o140644, 0o160644] {
        assert_code(
            "T3",
            &changed_member(|entry| entry.attributes = mode << 16),
            SPECIAL_FILE,
        );
    }
    for mode in [0o104644, 0o102644, 0o101644] {
        assert_code(
            "T4",
            &changed_member(|entry| entry.attributes = mode << 16),
            SPECIAL_FILE,
        );
    }
    for mode in [0o100755, 0o100744] {
        assert_code(
            "T5",
            &changed_member(|entry| entry.attributes = mode << 16),
            EXECUTABLE,
        );
    }
    for name in [
        "run.sh",
        "tool.js",
        "Makefile",
        "build.rs",
        "Dockerfile",
        "a.exe",
        "a.so",
        "a.dylib",
        "a.dll",
        "a.bin",
        "a.py",
        "a.bat",
        "a.cmd",
        "a.ps1",
        "a.vbs",
        "a.wasm",
        "a.com",
        "a.mjs",
        "a.ts",
        "a.rb",
        "a.pl",
        "a.php",
        "a.jar",
    ] {
        assert_code(
            "T6",
            &changed_member(|entry| entry.name = name.as_bytes().to_vec()),
            EXECUTABLE,
        );
    }
    let mut entries = golden(0);
    entries[2].attributes = 0o600 << 16;
    assert!(
        read_restricted_zip(&build(&entries).bytes).is_ok(),
        "T Python unspecified type"
    );
}

#[test]
fn limits_deflate() {
    for index in [0, 2] {
        let mut entries = golden(0);
        entries[index].method = 8;
        entries[index].size = MAX_FILE as u32 + 1;
        entries[index].compressed = 1;
        entries[index].payload = vec![0x07];
        assert_code(
            "Z1 member/manifest before decode",
            &build(&entries).bytes,
            MEMBER_TOO_LARGE,
        );
    }
    let mut entries = golden(0);
    entries.truncate(1);
    for i in 0..11 {
        let mut entry = Entry::new(&format!("large{i}.md"), b"x", 8);
        entry.size = MAX_FILE as u32;
        entry.compressed = 10_486;
        entry.payload = vec![0x07; 10_486];
        entries.push(entry);
    }
    assert_code(
        "Z2 before bad deflate",
        &build(&entries).bytes,
        TOTAL_TOO_LARGE,
    );
    assert_code(
        "Z3 fractional",
        &changed_member(|entry| {
            entry.method = 8;
            entry.size = 100_001;
            entry.compressed = 1_000;
            entry.payload = vec![0x07; 1_000];
        }),
        RATIO_EXCEEDED,
    );
    let bomb = Entry::new("bomb.md", &vec![0; MAX_FILE as usize], 8);
    let mut entries = golden(0);
    entries[2] = bomb.clone();
    assert_code("Z3 actual bomb", &build(&entries).bytes, RATIO_EXCEEDED);
    assert_code(
        "Z4 compressed zero",
        &changed_member(|entry| {
            entry.method = 8;
            entry.size = 10;
            entry.compressed = 0;
            entry.payload.clear();
        }),
        RATIO_EXCEEDED,
    );
    assert_code(
        "Z4 stored",
        &changed_member(|entry| entry.size += 1),
        SIZE_MISMATCH,
    );
    entries[2] = bomb;
    entries[2].size = 1_000;
    assert_code("Z5 bounded output", &build(&entries).bytes, SIZE_MISMATCH);
    let short: Vec<_> = b"abcdefghijklmnopqrstuvwxyz0123456789"
        .iter()
        .copied()
        .cycle()
        .take(1_000)
        .collect();
    entries[2] = Entry::new("short.md", &short, 8);
    entries[2].size = 2_000;
    assert_code("Z6", &build(&entries).bytes, SIZE_MISMATCH);
    entries[2] = Entry::new("trailer.md", b"some ordinary text", 8);
    entries[2].payload.extend_from_slice(b"extra");
    entries[2].compressed += 5;
    assert_code("Z7", &build(&entries).bytes, SIZE_MISMATCH);
    entries[2] = Entry::new("truncated.md", b"some ordinary text", 8);
    entries[2].payload.pop();
    entries[2].compressed -= 1;
    assert_code("Z8", &build(&entries).bytes, SIZE_MISMATCH);
    entries[2] = Entry::new("corrupt.md", b"x", 8);
    entries[2].payload = vec![0x07];
    entries[2].size = 1;
    entries[2].compressed = 1;
    assert_code("Z9", &build(&entries).bytes, MALFORMED);
}

#[test]
fn integrity() {
    assert_code(
        "I1",
        &changed_member(|entry| entry.crc32 ^= 1),
        CRC_MISMATCH,
    );
    let base = build(&golden(0));
    for (field, wide) in [(14, true), (8, false), (30, false)] {
        let mut bytes = base.bytes.clone();
        if wide {
            set32(&mut bytes, base.locals[2] + field, 1);
        } else if field == 8 {
            set16(&mut bytes, base.locals[2] + field, 12);
        } else {
            bytes[base.locals[2] + field] = b'x';
        }
        assert_code("I2 crc/method/name", &bytes, HEADER_MISMATCH);
    }
    for index in [1, 2] {
        let mut entries = golden(8);
        entries[index].compressed += 1;
        assert_code(
            "I3 next header/central overlap",
            &build(&entries).bytes,
            OVERLAP,
        );
    }
    let mut entries = golden(0);
    let inner = build(&[entries[2].clone()]);
    entries[1].method = 8;
    entries[1].payload = inner.bytes[..inner.central[0]].to_vec();
    entries[1].compressed = entries[1].payload.len() as u32;
    let mut nested = build(&entries);
    let inner_at = nested.locals[1] + 30 + entries[1].name.len();
    set32(&mut nested.bytes, nested.central[2] + 42, inner_at as u32);
    assert_code("I3 nested complete local header", &nested.bytes, OVERLAP);
}

#[test]
fn manifest() {
    assert_code("M1", &build(&golden(0)[1..]).bytes, MANIFEST_MISSING);
    let mut entries = golden(0);
    entries[0] = Entry::new("manifest.json", b"{", 0);
    assert_code("M2 invalid JSON", &build(&entries).bytes, MANIFEST_INVALID);
    entries = golden(0);
    let mut value: serde_json::Value = serde_json::from_slice(&entries[0].content).unwrap();
    value["unknown"] = serde_json::json!(1);
    entries[0] = Entry::new("manifest.json", &serde_json::to_vec(&value).unwrap(), 0);
    assert_code("M2 unknown field", &build(&entries).bytes, MANIFEST_INVALID);
    assert_code(
        "M2 reserved entry",
        &changed_manifest(|manifest| manifest.entries[0].path = "manifest.json".into()),
        MANIFEST_INVALID,
    );
    entries = golden(0);
    entries.push(Entry::new("extra.md", b"extra", 0));
    assert_code("M3", &build(&entries).bytes, MANIFEST_EXTRA_MEMBER);
    assert_code(
        "M4",
        &changed_manifest(|manifest| {
            manifest.entries.push(PackageEntry {
                path: "absent.md".into(),
                digest_sha256: hash(b"absent"),
                size_bytes: 6,
                compressed_bytes: 6,
            })
        }),
        MANIFEST_MEMBER_MISSING,
    );
    for field in 0..3 {
        assert_code(
            "M5 digest/size/compressed",
            &changed_manifest(|manifest| match field {
                0 => manifest.entries[1].digest_sha256 = hash(b"different"),
                1 => manifest.entries[1].size_bytes += 1,
                _ => manifest.entries[1].compressed_bytes += 1,
            }),
            MANIFEST_DIGEST_MISMATCH,
        );
    }
    let denied = changed_manifest(|manifest| manifest.license = "unknown".into());
    assert!(
        matches!(read_restricted_zip(&denied), Err(Error::Forbidden)),
        "M6 license original Forbidden"
    );
    let schema = changed_manifest(|manifest| manifest.schema_version = "rsia.package.v0".into());
    assert_code(
        "M6 original schema error",
        &schema,
        "unsupported package schema: expected rsia.package.v1, got rsia.package.v0",
    );
}

#[test]
fn text() {
    for (label, bytes) in [
        ("X1 UTF8", b"\xff".as_slice()),
        ("X1 NUL", b"a\0b".as_slice()),
        ("X1 C0", b"a\x01b".as_slice()),
        ("X1 DEL", b"a\x7fb".as_slice()),
        ("X1 nested ZIP", b"PK\x03\x04payload".as_slice()),
    ] {
        let entries = package(vec![Entry::new("content.md", bytes, 0)], 0);
        assert_code(label, &build(&entries).bytes, NOT_TEXT);
    }
}

#[tokio::test]
async fn stage() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("zip.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("zip-test", "admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    let body = serde_json::json!({"schema_version":"fixture.source.v1","id":"zip-source"});
    session
        .put(&admin, "artifact", "zip-source", admin.actor(), &body)
        .await
        .unwrap();
    session
        .bump_watermark(&admin, "zip-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let sources = vec![E16SourceRef {
        kind: "artifact".into(),
        id: "zip-source".into(),
        digest: fingerprint(&body).unwrap(),
    }];
    let g0 = build(&golden(0)).bytes;
    let read = read_restricted_zip(&g0).unwrap();
    let imported =
        import_external_package(&read.manifest, &read.files, |_, _| true, |_, _| false).unwrap();
    assert_eq!(imported.state, PackageState::Staged, "P8 import");
    assert!(
        !imported.is_active && !imported.is_approved,
        "P8 no authority"
    );
    assert!(
        imported.local_formal_evaluation_id.is_none() && imported.local_approval_id.is_none(),
        "P8 no local approval"
    );
    let zipped = stage_restricted_zip(&admin, &store, zip_request("zipped", g0.clone(), &sources))
        .await
        .unwrap();
    let direct = PersistentPackageStore::stage_package(
        &admin,
        &store,
        StagePackageRequest {
            request_key: "direct".into(),
            manifest: read.manifest,
            files: read.files,
            source_refs: sources.clone(),
            baseline_digest: hash(b"baseline"),
            local_digest: hash(b"local"),
            upstream_digest: Some(hash(b"upstream")),
            environment_digest: hash(b"environment"),
            compiler_version: "compiler-v1".into(),
            candidate_material: None,
        },
    )
    .await
    .unwrap();
    assert_ne!(zipped.id, direct.id, "P8 different requests actually stage");
    assert_eq!(
        zipped.payload.state,
        StagedAssetState::Staged,
        "P8 zipped state"
    );
    assert_eq!(
        direct.payload.state,
        StagedAssetState::Staged,
        "P8 direct state"
    );
    assert_eq!(
        zipped.payload.content_blob_digest, direct.payload.content_blob_digest,
        "P8 content digest"
    );
    assert_eq!(
        zipped.payload.manifest_digest, direct.payload.manifest_digest,
        "P8 manifest digest"
    );
    assert!(
        zipped.payload.candidate_material.is_none() && zipped.payload.candidate_ref.is_none(),
        "P8 no Current candidate"
    );

    let before = serde_json::to_value(
        PersistentPackageStore::read_staged(&admin, &store, &zipped.id)
            .await
            .unwrap(),
    )
    .unwrap();
    let mut duplicate = golden(0);
    duplicate[2].name = duplicate[1].name.clone();
    let mut bad_manifest = golden(0);
    bad_manifest[0] = Entry::new("manifest.json", b"{", 0);
    let invalid_text = package(vec![Entry::new("invalid.md", b"\xff", 0)], 0);
    let attacks = [
        ("s", vec![0; 21], MALFORMED),
        ("u", changed_member(|entry| entry.flags = 1), ENCRYPTION),
        (
            "n",
            changed_member(|entry| entry.name = b"../x.md".to_vec()),
            TRAVERSAL,
        ),
        ("d", build(&duplicate).bytes, DUPLICATE),
        (
            "t",
            changed_member(|entry| entry.attributes = 0o120777 << 16),
            SYMLINK,
        ),
        (
            "z",
            changed_member(|entry| {
                entry.method = 8;
                entry.size = MAX_FILE as u32 + 1;
                entry.payload = vec![0x07];
                entry.compressed = 1;
            }),
            MEMBER_TOO_LARGE,
        ),
        ("i", changed_member(|entry| entry.crc32 ^= 1), CRC_MISMATCH),
        ("m", build(&bad_manifest).bytes, MANIFEST_INVALID),
        ("x", build(&invalid_text).bytes, NOT_TEXT),
    ];
    for (family, attack, code) in attacks {
        let key = format!("failed-{family}");
        match stage_restricted_zip(&admin, &store, zip_request(&key, attack, &sources)).await {
            Err(Error::Invalid(actual)) => assert_eq!(actual, code, "W1 {family}"),
            actual => panic!("W1 {family}: expected {code}, got {actual:?}"),
        }
        let after = serde_json::to_value(
            PersistentPackageStore::read_staged(&admin, &store, &zipped.id)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            after, before,
            "W1 {family} complete existing envelope unchanged"
        );
        let first = stage_restricted_zip(&admin, &store, zip_request(&key, g0.clone(), &sources))
            .await
            .unwrap();
        assert_eq!(
            first.request_key, key,
            "W1 {family} failed key remains available"
        );
        assert_eq!(
            first.payload.state,
            StagedAssetState::Staged,
            "W1 {family} first valid creation"
        );
    }
    let worker = Context::new("zip-test", "worker", Role::Worker).unwrap();
    assert!(
        matches!(
            stage_restricted_zip(
                &worker,
                &store,
                zip_request("worker", vec![0; 21], &sources)
            )
            .await,
            Err(Error::Forbidden)
        ),
        "W2 Admin before ZIP parsing"
    );
    store.close().await;
}

#[test]
fn repository() {
    let reader = include_str!("../src/zip_reader.rs");
    for forbidden in ["std::fs", "tokio::fs", "tempfile", "File::", "OpenOptions"] {
        assert!(!reader.contains(forbidden), "W3 {forbidden}");
    }
    let lock = include_str!("../../../Cargo.lock");
    assert!(
        lock.contains("name = \"flate2\"\nversion = \"1.1.10\""),
        "W4 flate2 lock"
    );
    assert!(
        lock.contains("name = \"zlib-rs\"\nversion = \"0.6.8\""),
        "W4 zlib lock"
    );
    let engine = include_str!("../Cargo.toml");
    assert!(
        engine.contains(
            "flate2 = { version = \"=1.1.10\", default-features = false, features = [\"zlib-rs\"] }"
        ),
        "W4 engine dependency"
    );
    let register: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/dependency-register.json")).unwrap();
    let dependency = register["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "flate2")
        .unwrap();
    assert_eq!(dependency["version"], "1.1.10", "W4 register version");
    let consumer = dependency["consumers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["manifest"] == "crates/evo-engine/Cargo.toml")
        .unwrap();
    assert_eq!(
        consumer["features"],
        serde_json::json!(["zlib-rs"]),
        "W4 register backend"
    );
    assert_eq!(consumer["default_features"], false, "W4 register features");
}

#[test]
fn package_metadata() {
    for (size, compressed, allowed) in [
        (100_001, 1_000, false),
        (100_000, 1_000, true),
        (10, 0, false),
        (0, 0, true),
    ] {
        let member = PackageMember {
            path: "ratio.md".into(),
            size,
            compressed,
        };
        assert_eq!(
            reject_member(&member).is_ok(),
            allowed,
            "K1 {size}/{compressed}"
        );
    }
    for (path, allowed) in [
        ("dir\\..\\x.md", false),
        ("a\\b.md", false),
        ("a\u{1}b.md", false),
        ("a\u{202e}b.md", false),
        ("技能.md", true),
        ("skill.md", true),
    ] {
        let member = PackageMember {
            path: path.into(),
            size: 1,
            compressed: 1,
        };
        assert_eq!(reject_member(&member).is_ok(), allowed, "K2 {path:?}");
    }
    for (paths, allowed) in [
        (["a.md", "A.md"], false),
        (["docs", "docs/a.md"], false),
        (["a.md", "b/a.md"], true),
        (["Docs/a.md", "docs"], false),
    ] {
        let members: Vec<_> = paths
            .iter()
            .map(|path| PackageMember {
                path: (*path).into(),
                size: 1,
                compressed: 1,
            })
            .collect();
        assert_eq!(reject_package(&members).is_ok(), allowed, "K3 {paths:?}");
    }
    for point in [0x00ad, 0x200b, 0x200e, 0x202e, 0x2066, 0xfeff, 0xe0001] {
        let path = format!("a{}b.md", char::from_u32(point).unwrap());
        let member = PackageMember {
            path: path.clone(),
            size: 1,
            compressed: 1,
        };
        assert!(
            matches!(reject_member(&member), Err(Error::Invalid(_))),
            "K4 directory U+{point:X}"
        );
        assert_code(
            "K4 ZIP",
            &changed_member(|entry| {
                entry.name = path.as_bytes().to_vec();
                entry.flags = 0x800;
            }),
            BAD_NAME,
        );
    }
}

#[test]
fn fixture_validity() {
    let traversal = Entry::new("../x.md", b"x", 0);
    let joined = Path::new("/r").join(std::str::from_utf8(&traversal.name).unwrap());
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    assert!(
        !normalized.starts_with("/r"),
        "F1 lexical traversal escapes root"
    );
    let bomb = Entry::new("bomb.md", &vec![0; MAX_FILE as usize], 8);
    let decoder = flate2::read::DeflateDecoder::new(bomb.payload.as_slice());
    let produced = copy(&mut decoder.take(2 * MAX_FILE), &mut sink()).unwrap();
    assert_eq!(produced, MAX_FILE, "F2 actual decode count");
    assert!(
        u128::from(produced) > 100 * bomb.payload.len() as u128,
        "F2 actual ratio"
    );
    let mut link = Entry::new("link.md", b"../../etc/passwd", 0);
    link.attributes = 0o120777 << 16;
    let archive = build(&[link]);
    let attribute_at = archive.central[0] + 38;
    let attributes = u32::from_le_bytes(
        archive.bytes[attribute_at..attribute_at + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(
        (attributes >> 16) & 0o170000,
        0o120000,
        "F3 raw central type"
    );
    let name_length = u16::from_le_bytes(archive.bytes[26..28].try_into().unwrap()) as usize;
    assert_eq!(
        &archive.bytes[30 + name_length..archive.central[0]],
        b"../../etc/passwd",
        "F3 raw payload"
    );
}
