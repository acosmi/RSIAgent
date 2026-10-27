//! Restricted, in-memory ZIP packages. Management ingress remains separate.
use crate::packages::{
    AssetPackageManifest, CandidateMaterialBinding, E16Envelope, E16SourceRef, MAX_FILE, MAX_FILES,
    MAX_TOTAL, MAX_ZIP_RATIO, PersistentPackageStore, StagePackageRequest, StagedAssetPayload,
    validate_manifest,
};
use evo_core::{Context, Error, Result, Role, hash};
use evo_storage::Store;
use flate2::{Crc, Decompress, FlushDecompress, Status};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_ENTRIES: usize = MAX_FILES + 1;
pub const MAX_NAME_BYTES: usize = 255;
pub const MAX_ARCHIVE_BYTES: u64 = MAX_TOTAL + MAX_FILE;

pub const ZIP64: &str = "unsupported_format:zip:zip64";
pub const ENCRYPTION: &str = "unsupported_format:zip:encryption";
pub const COMPRESSION_METHOD: &str = "unsupported_format:zip:compression_method";
pub const DATA_DESCRIPTOR: &str = "unsupported_format:zip:data_descriptor";
pub const EXTRA_FIELD: &str = "unsupported_format:zip:extra_field";
pub const DIRECTORY_ENTRY: &str = "unsupported_format:zip:directory_entry";
pub const MULTI_DISK: &str = "unsupported_format:zip:multi_disk";
pub const COMMENT: &str = "unsupported_format:zip:comment";
pub const FLAGS: &str = "unsupported_format:zip:flags";
pub const VERSION_NEEDED: &str = "unsupported_format:zip:version_needed";
pub const TRAVERSAL: &str = "zip_invalid:traversal";
pub const ABSOLUTE: &str = "zip_invalid:absolute";
pub const DUPLICATE: &str = "zip_invalid:duplicate";
pub const SYMLINK: &str = "zip_invalid:symlink";
pub const SPECIAL_FILE: &str = "zip_invalid:special_file";
pub const EXECUTABLE: &str = "zip_invalid:executable";
pub const TOO_MANY_ENTRIES: &str = "zip_invalid:too_many_entries";
pub const MEMBER_TOO_LARGE: &str = "zip_invalid:member_too_large";
pub const TOTAL_TOO_LARGE: &str = "zip_invalid:total_too_large";
pub const RATIO_EXCEEDED: &str = "zip_invalid:ratio_exceeded";
pub const SIZE_MISMATCH: &str = "zip_invalid:size_mismatch";
pub const CRC_MISMATCH: &str = "zip_invalid:crc_mismatch";
pub const OVERLAP: &str = "zip_invalid:overlap";
pub const HEADER_MISMATCH: &str = "zip_invalid:header_mismatch";
pub const MANIFEST_MISSING: &str = "zip_invalid:manifest_missing";
pub const MANIFEST_EXTRA_MEMBER: &str = "zip_invalid:manifest_extra_member";
pub const MANIFEST_DIGEST_MISMATCH: &str = "zip_invalid:manifest_digest_mismatch";
pub const NOT_TEXT: &str = "zip_invalid:not_text";
pub const ARCHIVE_TOO_LARGE: &str = "zip_invalid:archive_too_large";
pub const MALFORMED: &str = "zip_invalid:malformed";
pub const BAD_NAME: &str = "zip_invalid:bad_name";
pub const MANIFEST_INVALID: &str = "zip_invalid:manifest_invalid";
pub const MANIFEST_MEMBER_MISSING: &str = "zip_invalid:manifest_member_missing";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestrictedZipMember {
    pub path: String,
    pub method: u16,
    pub size: u64,
    pub compressed: u64,
    pub crc32: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestrictedZipPackage {
    pub manifest: AssetPackageManifest,
    pub files: BTreeMap<String, Vec<u8>>,
    pub members: Vec<RestrictedZipMember>,
}

#[derive(Debug, Clone)]
pub struct StageRestrictedZipRequest {
    pub archive: Vec<u8>,
    pub request_key: String,
    pub source_refs: Vec<E16SourceRef>,
    pub baseline_digest: String,
    pub local_digest: String,
    pub upstream_digest: Option<String>,
    pub environment_digest: String,
    pub compiler_version: String,
    pub candidate_material: Option<CandidateMaterialBinding>,
}

// Unicode 15.0 Cf: 21 ranges, 170 code points.
const FORMAT_RANGES: [(u32, u32); 21] = [
    (0x00ad, 0x00ad),
    (0x0600, 0x0605),
    (0x061c, 0x061c),
    (0x06dd, 0x06dd),
    (0x070f, 0x070f),
    (0x0890, 0x0891),
    (0x08e2, 0x08e2),
    (0x180e, 0x180e),
    (0x200b, 0x200f),
    (0x202a, 0x202e),
    (0x2060, 0x2064),
    (0x2066, 0x206f),
    (0xfeff, 0xfeff),
    (0xfff9, 0xfffb),
    (0x110bd, 0x110bd),
    (0x110cd, 0x110cd),
    (0x13430, 0x1343f),
    (0x1bca0, 0x1bca3),
    (0x1d173, 0x1d17a),
    (0xe0001, 0xe0001),
    (0xe0020, 0xe007f),
];

// All callers have already framed the fixed-width header.
fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn ratio_exceeded(size: u64, compressed: u64) -> bool {
    u128::from(size) > u128::from(MAX_ZIP_RATIO) * u128::from(compressed)
}

fn checked_name(bytes: &[u8], flags: u16) -> Result<String> {
    if bytes.is_empty() || bytes.len() > MAX_NAME_BYTES {
        return Err(Error::Invalid(BAD_NAME.into()));
    }
    if flags & 0x0800 == 0 && !bytes.is_ascii() {
        return Err(Error::Invalid(BAD_NAME.into()));
    }
    let name = std::str::from_utf8(bytes).map_err(|_| Error::Invalid(BAD_NAME.into()))?;
    if name.chars().any(|c| {
        c.is_control()
            || FORMAT_RANGES
                .iter()
                .any(|&(first, last)| (first..=last).contains(&(c as u32)))
    }) {
        return Err(Error::Invalid(BAD_NAME.into()));
    }
    if name.starts_with('/') || name.contains(':') {
        return Err(Error::Invalid(ABSOLUTE.into()));
    }
    if name.contains('\\') || name.split('/').any(|part| part == "..") {
        return Err(Error::Invalid(TRAVERSAL.into()));
    }
    if name.ends_with('/') {
        return Err(Error::Invalid(DIRECTORY_ENTRY.into()));
    }
    if name
        .split('/')
        .any(|part| part.is_empty() || part == "." || part.ends_with('.'))
        || name
            .bytes()
            .any(|b| b.is_ascii() && !b.is_ascii_alphanumeric() && !b"._-/".contains(&b))
    {
        return Err(Error::Invalid(BAD_NAME.into()));
    }
    Ok(name.into())
}

struct CentralEntry<'a> {
    header: &'a [u8],
    name: &'a [u8],
    member: RestrictedZipMember,
    offset: usize,
}

struct LocalRange {
    index: usize,
    offset: usize,
    data_start: usize,
    end: usize,
}

pub fn read_restricted_zip(archive: &[u8]) -> Result<RestrictedZipPackage> {
    // A: frame the entire central directory before interpreting member metadata.
    if archive.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(Error::Invalid(ARCHIVE_TOO_LARGE.into()));
    }
    if archive.len() < 22 {
        return Err(Error::Invalid(MALFORMED.into()));
    }
    let end_at = archive.len() - 22;
    let end = &archive[end_at..];
    if le32(end, 0) != 0x0605_4b50 || le16(end, 20) != 0 {
        for at in (archive.len().saturating_sub(65_557)..=end_at).rev() {
            if le32(archive, at) == 0x0605_4b50
                && usize::from(le16(archive, at + 20)) == archive.len() - at - 22
            {
                return Err(Error::Invalid(COMMENT.into()));
            }
        }
        return Err(Error::Invalid(MALFORMED.into()));
    }
    if [4, 6, 8, 10].iter().any(|&at| le16(end, at) == u16::MAX)
        || le32(end, 12) == u32::MAX
        || le32(end, 16) == u32::MAX
        || (end_at >= 20 && le32(archive, end_at - 20) == 0x0706_4b50)
    {
        return Err(Error::Invalid(ZIP64.into()));
    }
    if le16(end, 4) != 0 || le16(end, 6) != 0 || le16(end, 8) != le16(end, 10) {
        return Err(Error::Invalid(MULTI_DISK.into()));
    }
    let count = usize::from(le16(end, 10));
    if count == 0 {
        return Err(Error::Invalid(MANIFEST_MISSING.into()));
    }
    if count > MAX_ENTRIES {
        return Err(Error::Invalid(TOO_MANY_ENTRIES.into()));
    }
    let cd_at = usize::try_from(le32(end, 16)).map_err(|_| Error::Invalid(MALFORMED.into()))?;
    let cd_size = usize::try_from(le32(end, 12)).map_err(|_| Error::Invalid(MALFORMED.into()))?;
    if cd_at.checked_add(cd_size) != Some(end_at) {
        return Err(Error::Invalid(MALFORMED.into()));
    }
    let mut frames = Vec::new();
    let mut cursor = cd_at;
    for _ in 0..count {
        let fixed_end = cursor
            .checked_add(46)
            .ok_or_else(|| Error::Invalid(MALFORMED.into()))?;
        if fixed_end > end_at {
            return Err(Error::Invalid(MALFORMED.into()));
        }
        let header = &archive[cursor..fixed_end];
        if le32(header, 0) != 0x0201_4b50 {
            return Err(Error::Invalid(MALFORMED.into()));
        }
        let name_end = fixed_end
            .checked_add(usize::from(le16(header, 28)))
            .ok_or_else(|| Error::Invalid(MALFORMED.into()))?;
        let record_end = name_end
            .checked_add(usize::from(le16(header, 30)))
            .and_then(|at| at.checked_add(usize::from(le16(header, 32))))
            .ok_or_else(|| Error::Invalid(MALFORMED.into()))?;
        if record_end > end_at {
            return Err(Error::Invalid(MALFORMED.into()));
        }
        frames.push((header, &archive[fixed_end..name_end]));
        cursor = record_end;
    }
    if cursor != end_at {
        return Err(Error::Invalid(MALFORMED.into()));
    }

    // E: convert each framed member into checked metadata, in directory order.
    let mut entries = Vec::new();
    for (header, name) in frames {
        let flags = le16(header, 8);
        if flags & 0x2041 != 0 {
            return Err(Error::Invalid(ENCRYPTION.into()));
        }
        if flags & 0x0008 != 0 {
            return Err(Error::Invalid(DATA_DESCRIPTOR.into()));
        }
        if flags & !0x0806 != 0 {
            return Err(Error::Invalid(FLAGS.into()));
        }
        let method = le16(header, 10);
        if !matches!(method, 0 | 8) {
            return Err(Error::Invalid(COMPRESSION_METHOD.into()));
        }
        if [20, 24, 42].iter().any(|&at| le32(header, at) == u32::MAX) {
            return Err(Error::Invalid(ZIP64.into()));
        }
        if le16(header, 6) > 20 {
            return Err(Error::Invalid(VERSION_NEEDED.into()));
        }
        if le16(header, 30) != 0 {
            return Err(Error::Invalid(EXTRA_FIELD.into()));
        }
        if le16(header, 32) != 0 {
            return Err(Error::Invalid(COMMENT.into()));
        }
        if le16(header, 34) != 0 {
            return Err(Error::Invalid(MULTI_DISK.into()));
        }
        let path = checked_name(name, flags)?;
        let attributes = le32(header, 38);
        let mode = attributes >> 16;
        let kind = mode & 0o170000;
        if attributes & 0x10 != 0 || kind == 0o040000 {
            return Err(Error::Invalid(DIRECTORY_ENTRY.into()));
        }
        if kind == 0o120000 || attributes & 0x400 != 0 {
            return Err(Error::Invalid(SYMLINK.into()));
        }
        if !matches!(kind, 0 | 0o100000) || mode & 0o7000 != 0 {
            return Err(Error::Invalid(SPECIAL_FILE.into()));
        }
        if mode & 0o111 != 0 {
            return Err(Error::Invalid(EXECUTABLE.into()));
        }
        let lower = path.to_lowercase();
        if [
            ".sh", ".exe", ".so", ".dylib", ".dll", ".bin", ".py", ".bat", ".cmd", ".ps1", ".vbs",
            ".wasm", ".com", ".js", ".mjs", ".ts", ".rb", ".pl", ".php", ".jar",
        ]
        .iter()
        .any(|extension| lower.ends_with(*extension))
            || matches!(
                lower.rsplit('/').next(),
                Some("makefile" | "dockerfile" | "build.rs")
            )
        {
            return Err(Error::Invalid(EXECUTABLE.into()));
        }
        let size = u64::from(le32(header, 24));
        let compressed = u64::from(le32(header, 20));
        if size > MAX_FILE {
            return Err(Error::Invalid(MEMBER_TOO_LARGE.into()));
        }
        if method == 0 && size != compressed {
            return Err(Error::Invalid(SIZE_MISMATCH.into()));
        }
        if ratio_exceeded(size, compressed) {
            return Err(Error::Invalid(RATIO_EXCEEDED.into()));
        }
        entries.push(CentralEntry {
            header,
            name,
            member: RestrictedZipMember {
                path,
                method,
                size,
                compressed,
                crc32: le32(header, 16),
            },
            offset: usize::try_from(le32(header, 42))
                .map_err(|_| Error::Invalid(MALFORMED.into()))?,
        });
    }

    // X: finish every cross-member and local-header check before touching data.
    let mut names = BTreeSet::<String>::new();
    for entry in &entries {
        let folded = entry.member.path.to_lowercase();
        if !names.insert(folded.clone())
            || names.iter().any(|other| {
                folded
                    .strip_prefix(other.as_str())
                    .is_some_and(|tail| tail.starts_with('/'))
                    || other
                        .strip_prefix(folded.as_str())
                        .is_some_and(|tail| tail.starts_with('/'))
            })
        {
            return Err(Error::Invalid(DUPLICATE.into()));
        }
    }
    let total: u64 = entries.iter().map(|entry| entry.member.size).sum();
    let compressed: u64 = entries.iter().map(|entry| entry.member.compressed).sum();
    if total > MAX_TOTAL {
        return Err(Error::Invalid(TOTAL_TOO_LARGE.into()));
    }
    if ratio_exceeded(total, compressed) {
        return Err(Error::Invalid(RATIO_EXCEEDED.into()));
    }
    if !entries
        .iter()
        .any(|entry| entry.member.path == "manifest.json")
    {
        return Err(Error::Invalid(MANIFEST_MISSING.into()));
    }
    let mut ranges = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let fixed_end = entry
            .offset
            .checked_add(30)
            .ok_or_else(|| Error::Invalid(MALFORMED.into()))?;
        let local = archive
            .get(entry.offset..fixed_end)
            .ok_or_else(|| Error::Invalid(MALFORMED.into()))?;
        if le32(local, 0) != 0x0403_4b50 {
            return Err(Error::Invalid(MALFORMED.into()));
        }
        if le16(local, 28) != 0 {
            return Err(Error::Invalid(EXTRA_FIELD.into()));
        }
        let data_start = fixed_end
            .checked_add(usize::from(le16(local, 26)))
            .ok_or_else(|| Error::Invalid(MALFORMED.into()))?;
        let local_name = archive
            .get(fixed_end..data_start)
            .ok_or_else(|| Error::Invalid(MALFORMED.into()))?;
        if local[4..28] != entry.header[6..30] || local_name != entry.name {
            return Err(Error::Invalid(HEADER_MISMATCH.into()));
        }
        let compressed = usize::try_from(entry.member.compressed)
            .map_err(|_| Error::Invalid(MALFORMED.into()))?;
        let range_end = data_start
            .checked_add(compressed)
            .ok_or_else(|| Error::Invalid(MALFORMED.into()))?;
        ranges.push(LocalRange {
            index,
            offset: entry.offset,
            data_start,
            end: range_end,
        });
    }
    ranges.sort_unstable_by_key(|range| range.offset);
    let mut previous_end = 0;
    for range in &ranges {
        if range.offset < previous_end || range.end > cd_at {
            return Err(Error::Invalid(OVERLAP.into()));
        }
        if range.offset != previous_end {
            return Err(Error::Invalid(MALFORMED.into()));
        }
        previous_end = range.end;
    }
    if previous_end != cd_at {
        return Err(Error::Invalid(MALFORMED.into()));
    }

    // D: raw DEFLATE is bounded by the already checked declaration plus one.
    let mut decoded = BTreeMap::new();
    let mut produced = 0u64;
    for range in ranges {
        let member = &entries[range.index].member;
        let data = &archive[range.data_start..range.end];
        let bytes = if member.method == 0 {
            data.to_vec()
        } else {
            let size =
                usize::try_from(member.size).map_err(|_| Error::Invalid(SIZE_MISMATCH.into()))?;
            let mut bytes = vec![0; size + 1];
            let mut decoder = Decompress::new(false);
            loop {
                let before = (decoder.total_in(), decoder.total_out());
                let status = decoder
                    .decompress(
                        &data[decoder.total_in() as usize..],
                        &mut bytes[decoder.total_out() as usize..],
                        FlushDecompress::None,
                    )
                    .map_err(|_| Error::Invalid(MALFORMED.into()))?;
                if status == Status::StreamEnd {
                    break;
                }
                if decoder.total_out() > member.size
                    || before == (decoder.total_in(), decoder.total_out())
                {
                    return Err(Error::Invalid(SIZE_MISMATCH.into()));
                }
            }
            if decoder.total_in() != member.compressed || decoder.total_out() != member.size {
                return Err(Error::Invalid(SIZE_MISMATCH.into()));
            }
            bytes.truncate(size);
            bytes
        };
        produced += bytes.len() as u64;
        if produced > MAX_TOTAL {
            return Err(Error::Invalid(TOTAL_TOO_LARGE.into()));
        }
        let mut crc = Crc::new();
        crc.update(&bytes);
        if crc.sum() != member.crc32 {
            return Err(Error::Invalid(CRC_MISMATCH.into()));
        }
        decoded.insert(member.path.clone(), (bytes, member.compressed));
    }

    // M: preserve the existing manifest gate and its original errors.
    let (manifest_bytes, _) = decoded
        .remove("manifest.json")
        .ok_or_else(|| Error::Invalid(MANIFEST_MISSING.into()))?;
    let manifest: AssetPackageManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| Error::Invalid(MANIFEST_INVALID.into()))?;
    if manifest
        .entries
        .iter()
        .any(|entry| entry.path == "manifest.json")
    {
        return Err(Error::Invalid(MANIFEST_INVALID.into()));
    }
    validate_manifest(&manifest)?;
    if decoded
        .keys()
        .any(|path| !manifest.entries.iter().any(|entry| &entry.path == path))
    {
        return Err(Error::Invalid(MANIFEST_EXTRA_MEMBER.into()));
    }
    for entry in &manifest.entries {
        let (bytes, compressed) = decoded
            .get(&entry.path)
            .ok_or_else(|| Error::Invalid(MANIFEST_MEMBER_MISSING.into()))?;
        if bytes.len() as u64 != entry.size_bytes
            || hash(bytes) != entry.digest_sha256
            || *compressed != entry.compressed_bytes
        {
            return Err(Error::Invalid(MANIFEST_DIGEST_MISMATCH.into()));
        }
    }
    for (bytes, _) in decoded.values() {
        std::str::from_utf8(bytes).map_err(|_| Error::Invalid(NOT_TEXT.into()))?;
        if bytes
            .iter()
            .any(|&b| (b < 0x20 && !b"\t\n\r".contains(&b)) || b == 0x7f)
        {
            return Err(Error::Invalid(NOT_TEXT.into()));
        }
    }
    Ok(RestrictedZipPackage {
        manifest,
        files: decoded
            .into_iter()
            .map(|(path, (bytes, _))| (path, bytes))
            .collect(),
        members: entries.into_iter().map(|entry| entry.member).collect(),
    })
}

pub async fn stage_restricted_zip(
    ctx: &Context,
    store: &Store,
    request: StageRestrictedZipRequest,
) -> Result<E16Envelope<StagedAssetPayload>> {
    ctx.require(&[Role::Admin])?;
    let package = read_restricted_zip(&request.archive)?;
    PersistentPackageStore::stage_package(
        ctx,
        store,
        StagePackageRequest {
            request_key: request.request_key,
            manifest: package.manifest,
            files: package.files,
            source_refs: request.source_refs,
            baseline_digest: request.baseline_digest,
            local_digest: request.local_digest,
            upstream_digest: request.upstream_digest,
            environment_digest: request.environment_digest,
            compiler_version: request.compiler_version,
            candidate_material: request.candidate_material,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_boundaries() {
        assert!(!ratio_exceeded(100_000, 1_000), "P4 exact");
        assert!(ratio_exceeded(100_001, 1_000), "P4 fraction");
        assert!(ratio_exceeded(1, 0), "P4 zero compressed");
        assert!(!ratio_exceeded(0, 0), "P4 empty");
        let compressed = u64::from(u32::MAX);
        assert!(!ratio_exceeded(100 * compressed, compressed), "P4 wide");
        assert!(
            ratio_exceeded(100 * compressed + 1, compressed),
            "P4 wide +1"
        );
        assert!(!ratio_exceeded(u64::MAX, u64::MAX), "P4 u128 product");
    }

    #[test]
    fn independent_crc_golden() {
        let mut crc = Crc::new();
        crc.update(b"123456789");
        assert_eq!(crc.sum(), 0xcbf4_3926, "CRC independent published vector");
    }

    #[test]
    fn eocd_and_comment_classification() {
        let mut archive = vec![0; 22];
        archive[..4].copy_from_slice(&0x0605_4b50u32.to_le_bytes());
        assert!(
            matches!(read_restricted_zip(&archive), Err(Error::Invalid(code)) if code == MANIFEST_MISSING),
            "S7"
        );
        archive[20..22].copy_from_slice(&5u16.to_le_bytes());
        archive.extend_from_slice(b"hello");
        assert!(
            matches!(read_restricted_zip(&archive), Err(Error::Invalid(code)) if code == COMMENT),
            "S4"
        );
        archive.push(0);
        assert!(
            matches!(read_restricted_zip(&archive), Err(Error::Invalid(code)) if code == MALFORMED),
            "S3 comment length"
        );
    }

    #[test]
    fn name_classification() {
        for (name, code) in [
            ("../x.md", TRAVERSAL),
            ("a/../x.md", TRAVERSAL),
            ("dir\\..\\x.md", TRAVERSAL),
            ("/etc/x.md", ABSOLUTE),
            ("C:x.md", ABSOLUTE),
            ("a\0b.md", BAD_NAME),
            ("a\u{85}b.md", BAD_NAME),
            ("a\u{202e}b.md", BAD_NAME),
            ("material/", DIRECTORY_ENTRY),
            ("a//b.md", BAD_NAME),
            ("a./b.md", BAD_NAME),
            ("a b.md", BAD_NAME),
        ] {
            assert!(
                matches!(checked_name(name.as_bytes(), 0x800), Err(Error::Invalid(actual)) if actual == code),
                "N {name:?}"
            );
        }
        assert_eq!(
            checked_name("技能.md".as_bytes(), 0x800).unwrap(),
            "技能.md",
            "P6"
        );
    }
}
