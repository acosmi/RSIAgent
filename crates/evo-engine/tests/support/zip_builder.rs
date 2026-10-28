use evo_core::hash;
use evo_engine::packages::{
    AssetPackageManifest, PACKAGE_SCHEMA_V1, PRIVACY_DISCLAIMER, PackageEntry, PackageKind,
    RedactionReport, RedactionStatus,
};
use flate2::{Compression, Crc, write::DeflateEncoder};
use std::io::Write;

pub const PROMPT: &[u8] = b"{\"instruction\":\"Be concise\"}\n";
pub const SKILL: &[u8] = b"# Skill\nFollow the prompt.\n";

#[derive(Clone)]
pub struct Entry {
    pub name: Vec<u8>,
    pub content: Vec<u8>,
    pub payload: Vec<u8>,
    pub method: u16,
    pub flags: u16,
    pub needed: u16,
    pub made_by: u16,
    pub attributes: u32,
    pub crc32: u32,
    pub size: u32,
    pub compressed: u32,
    pub extra: Vec<u8>,
    pub local_extra: Vec<u8>,
    pub comment: Vec<u8>,
    pub disk: u16,
}

impl Entry {
    pub fn new(name: &str, content: &[u8], method: u16) -> Self {
        let payload = if method == 8 {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(content).unwrap();
            encoder.finish().unwrap()
        } else {
            content.to_vec()
        };
        let mut crc = Crc::new();
        crc.update(content);
        Self {
            name: name.as_bytes().to_vec(),
            content: content.to_vec(),
            size: content.len().try_into().unwrap(),
            compressed: payload.len().try_into().unwrap(),
            payload,
            method,
            flags: if name.is_ascii() { 0 } else { 0x800 },
            needed: 20,
            made_by: 0x0314,
            attributes: 0o100644 << 16,
            crc32: crc.sum(),
            extra: Vec::new(),
            local_extra: Vec::new(),
            comment: Vec::new(),
            disk: 0,
        }
    }
}

pub fn manifest_for(entries: &[Entry]) -> AssetPackageManifest {
    AssetPackageManifest {
        schema_version: PACKAGE_SCHEMA_V1.into(),
        kind: PackageKind::Skill,
        publisher: "org.rsia".into(),
        asset_id: "restricted_zip".into(),
        name: "Restricted ZIP fixture".into(),
        version: "1.0.0".into(),
        description: "Deterministic restricted ZIP package".into(),
        license: "Apache-2.0".into(),
        entries: entries
            .iter()
            .map(|entry| PackageEntry {
                path: String::from_utf8(entry.name.clone()).unwrap(),
                digest_sha256: hash(&entry.content),
                size_bytes: u64::from(entry.size),
                compressed_bytes: u64::from(entry.compressed),
            })
            .collect(),
        dependencies: Vec::new(),
        redaction_report: RedactionReport {
            scanned_at: 0,
            findings_count: 0,
            status: RedactionStatus::Clean,
            disclaimer: PRIVACY_DISCLAIMER.into(),
            findings: Vec::new(),
        },
        foreign_metadata: None,
    }
}

pub fn package(mut entries: Vec<Entry>, method: u16) -> Vec<Entry> {
    let manifest = serde_json::to_vec(&manifest_for(&entries)).unwrap();
    entries.insert(0, Entry::new("manifest.json", &manifest, method));
    entries
}

pub fn golden(method: u16) -> Vec<Entry> {
    package(
        vec![
            Entry::new("prompt.json", PROMPT, method),
            Entry::new("skill.md", SKILL, method),
        ],
        method,
    )
}

pub fn set_manifest(entries: &mut [Entry], manifest: &AssetPackageManifest) {
    entries[0] = Entry::new(
        "manifest.json",
        &serde_json::to_vec(manifest).unwrap(),
        entries[0].method,
    );
}

pub struct Archive {
    pub bytes: Vec<u8>,
    pub locals: Vec<usize>,
    pub central: Vec<usize>,
    pub eocd: usize,
}

// Serialize the ZIP layout independently; no reader functions are used.
pub fn build(entries: &[Entry]) -> Archive {
    let mut bytes = Vec::new();
    let mut locals = Vec::new();
    for entry in entries {
        locals.push(bytes.len());
        bytes.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        for field in [entry.needed, entry.flags, entry.method, 0, 0x0021] {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
        for field in [entry.crc32, entry.compressed, entry.size] {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
        for length in [entry.name.len(), entry.local_extra.len()] {
            bytes.extend_from_slice(&u16::try_from(length).unwrap().to_le_bytes());
        }
        bytes.extend_from_slice(&entry.name);
        bytes.extend_from_slice(&entry.local_extra);
        bytes.extend_from_slice(&entry.payload);
    }
    let cd_at = bytes.len();
    let mut central = Vec::new();
    for (entry, &offset) in entries.iter().zip(&locals) {
        central.push(bytes.len());
        bytes.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        for field in [
            entry.made_by,
            entry.needed,
            entry.flags,
            entry.method,
            0,
            0x0021,
        ] {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
        for field in [entry.crc32, entry.compressed, entry.size] {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
        for length in [entry.name.len(), entry.extra.len(), entry.comment.len()] {
            bytes.extend_from_slice(&u16::try_from(length).unwrap().to_le_bytes());
        }
        bytes.extend_from_slice(&entry.disk.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&entry.attributes.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(offset).unwrap().to_le_bytes());
        bytes.extend_from_slice(&entry.name);
        bytes.extend_from_slice(&entry.extra);
        bytes.extend_from_slice(&entry.comment);
    }
    let eocd = bytes.len();
    let cd_size = eocd - cd_at;
    bytes.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    for field in [0, 0, entries.len() as u16, entries.len() as u16] {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    for field in [cd_size, cd_at] {
        bytes.extend_from_slice(&u32::try_from(field).unwrap().to_le_bytes());
    }
    bytes.extend_from_slice(&0u16.to_le_bytes());
    Archive {
        bytes,
        locals,
        central,
        eocd,
    }
}

pub fn set16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

pub fn set32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
