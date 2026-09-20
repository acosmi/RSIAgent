//! Asset package gates, staging, export, and privacy scanner.
//! Foreign approval is not local permission.
use crate::evidence::{load_stored_source, validate_stored_sources};
use crate::release_store::{RELEASE_CANDIDATE_SCHEMA, ReleaseCandidateRecord};
use crate::releases::validate_resolved_bundle_identity;
use evo_core::contract::SkillSnapshot;
use evo_core::{
    Context, Error, Result, Role, Strategy, Validate, fingerprint, hash, identifier, now,
};
use evo_storage::{
    LocalExportFile, LocalExportReceipt, Session, Store, local_export_receipt,
    local_export_tree_digest,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

pub const PACKAGE_SCHEMA_V1: &str = "rsia.package.v1";
pub const MAX_FILES: usize = 100;
pub const MAX_TOTAL: u64 = 10 * 1024 * 1024; // 10 MiB
pub const MAX_FILE: u64 = 1024 * 1024; // 1 MiB
pub const MAX_ZIP_RATIO: u64 = 100;
pub const MAX_PERSISTENT_PROJECTION: usize = 64 * 1024 * 1024; // encoded manifest + members

pub const PRIVACY_DISCLAIMER: &str =
    "Privacy scan does not guarantee absolute absence of sensitive data; human audit is required.";
pub const REMOTE_ERASURE_DISCLAIMER: &str = "Local revocation stops future distribution and invalidates local usage; remote erasure of third-party downloaded copies cannot be guaranteed.";
pub const FOREIGN_APPROVAL_DISCLAIMER: &str = "Foreign evaluations, approvals, and parent pointers are untrusted historical claims and do not grant local permissions or active status.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    Skill,
    Bundle,
    Improver,
    Template,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageEntry {
    pub path: String,
    pub digest_sha256: String,
    pub size_bytes: u64,
    pub compressed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDependency {
    pub publisher: String,
    pub asset_id: String,
    pub kind: String,
    pub version_req: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Warning,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacyFinding {
    pub rule: String,
    pub severity: FindingSeverity,
    pub detail: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionStatus {
    Clean,
    Warning,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedactionReport {
    pub scanned_at: i64,
    pub findings_count: usize,
    pub status: RedactionStatus,
    pub disclaimer: String,
    pub findings: Vec<PrivacyFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ForeignEvaluationClaim {
    pub evaluator_identity: String,
    pub evaluated_at: i64,
    pub score: String,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ForeignApprovalClaim {
    pub approver_identity: String,
    pub approved_at: i64,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForeignMetadata {
    #[serde(default)]
    pub external_namespace: Option<String>,
    #[serde(default)]
    pub external_parent_id: Option<String>,
    #[serde(default)]
    pub formal_evaluations: Vec<ForeignEvaluationClaim>,
    #[serde(default)]
    pub approvals: Vec<ForeignApprovalClaim>,
    pub disclaimer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetPackageManifest {
    pub schema_version: String,
    pub kind: PackageKind,
    pub publisher: String,
    pub asset_id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub license: String,
    pub entries: Vec<PackageEntry>,
    pub dependencies: Vec<PackageDependency>,
    pub redaction_report: RedactionReport,
    #[serde(default)]
    pub foreign_metadata: Option<ForeignMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageMember {
    pub path: String,
    pub size: u64,
    pub compressed: u64,
}

pub fn reject_member(m: &PackageMember) -> Result<()> {
    if m.path.trim().is_empty() {
        return Err(Error::Invalid("empty path".into()));
    }
    let p = Path::new(&m.path);
    if p.is_absolute() || m.path.starts_with('/') || m.path.starts_with('\\') {
        return Err(Error::Invalid("absolute path".into()));
    }
    // Check for drive letters like C: or device prefixes
    if m.path.contains(':') {
        return Err(Error::Invalid("device or drive prefix in path".into()));
    }

    let mut norm = String::new();
    for c in p.components() {
        match c {
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(Error::Invalid("path traversal".into()));
            }
            Component::Normal(s) => {
                let s = s.to_string_lossy();
                if s.contains('\0') {
                    return Err(Error::Invalid("null byte in path".into()));
                }
                if !norm.is_empty() {
                    norm.push('/');
                }
                norm.push_str(&s);
            }
            Component::CurDir => {}
        }
    }
    if m.path != norm || m.path.contains("//") || m.path.starts_with("./") {
        return Err(Error::Invalid("duplicate or non-normalized path".into()));
    }

    let lower = m.path.to_ascii_lowercase();
    let forbidden_extensions = [
        ".sh", ".exe", ".so", ".dylib", ".dll", ".bin", ".py", ".bat", ".cmd", ".ps1", ".vbs",
        ".wasm", ".com",
    ];
    for ext in forbidden_extensions {
        if lower.ends_with(ext) {
            return Err(Error::Invalid(format!(
                "executable or script member: {}",
                m.path
            )));
        }
    }
    if lower.contains("symlink") || lower.contains("hardlink") {
        return Err(Error::Invalid("link member indicator in path".into()));
    }

    if m.size > MAX_FILE {
        return Err(Error::Invalid(format!(
            "file too large: {} > max {}",
            m.size, MAX_FILE
        )));
    }
    if m.compressed > 0 && m.size / m.compressed.max(1) > MAX_ZIP_RATIO {
        return Err(Error::Invalid(format!(
            "zip bomb ratio: {} / {} > max {}",
            m.size, m.compressed, MAX_ZIP_RATIO
        )));
    }
    Ok(())
}

pub fn reject_package(members: &[PackageMember]) -> Result<()> {
    if members.is_empty() {
        return Err(Error::Invalid("package cannot be empty".into()));
    }
    if members.len() > MAX_FILES {
        return Err(Error::Invalid(format!(
            "too many files: {} > max {}",
            members.len(),
            MAX_FILES
        )));
    }
    let mut total = 0u64;
    let mut seen = BTreeSet::new();
    for m in members {
        reject_member(m)?;
        if !seen.insert(&m.path) {
            return Err(Error::Invalid(format!("duplicate path: {}", m.path)));
        }
        total = total.saturating_add(m.size);
    }
    if total > MAX_TOTAL {
        return Err(Error::Invalid(format!(
            "package too large: {} > max {}",
            total, MAX_TOTAL
        )));
    }
    Ok(())
}

pub fn scan_privacy(text: &str) -> RedactionReport {
    let mut findings = Vec::new();

    // 1. Private keys
    let private_key_markers = [
        "BEGIN PRIVATE KEY",
        "BEGIN RSA PRIVATE KEY",
        "BEGIN OPENSSH PRIVATE KEY",
        "BEGIN EC PRIVATE KEY",
        "BEGIN DSA PRIVATE KEY",
        "BEGIN PGP PRIVATE KEY BLOCK",
    ];
    for marker in private_key_markers {
        if text.contains(marker) {
            findings.push(PrivacyFinding {
                rule: "private_key".into(),
                severity: FindingSeverity::Block,
                detail: format!("contains private key marker '{marker}'"),
                snippet: marker.into(),
            });
        }
    }

    // 2. Secret tokens
    let token_prefixes = ["sk-", "ghp_", "gho_", "ghs_", "xoxb-", "xoxp-", "AIza"];
    for prefix in token_prefixes {
        if let Some(idx) = text.find(prefix) {
            let snippet_end = (idx + 12).min(text.len());
            findings.push(PrivacyFinding {
                rule: "secret_token".into(),
                severity: FindingSeverity::Block,
                detail: format!("contains secret token prefix '{prefix}'"),
                snippet: text[idx..snippet_end].into(),
            });
        }
    }

    // 3. User home directories / absolute paths
    let path_markers = ["/Users/", "/home/", "C:\\Users\\", "C:/Users/"];
    for marker in path_markers {
        if let Some(idx) = text.find(marker) {
            let snippet_end = (idx + 24).min(text.len());
            findings.push(PrivacyFinding {
                rule: "local_user_path".into(),
                severity: FindingSeverity::Block,
                detail: format!("contains local user path '{marker}'"),
                snippet: text[idx..snippet_end].into(),
            });
        }
    }

    // 4. Task answers / ground truth / hidden eval
    let answer_markers = [
        "ANSWER:",
        "GROUND_TRUTH:",
        "HIDDEN_EVAL:",
        "oracle_eval:",
        "oracle_solution:",
    ];
    for marker in answer_markers {
        if text.contains(marker) {
            findings.push(PrivacyFinding {
                rule: "hidden_answer".into(),
                severity: FindingSeverity::Block,
                detail: format!("contains hidden task answer marker '{marker}'"),
                snippet: marker.into(),
            });
        }
    }

    // 5. Internal network addresses & internal domains
    let internal_network_markers = [
        "192.168.",
        "10.0.",
        "10.1.",
        "172.16.",
        ".corp",
        ".internal",
        ".local",
    ];
    for marker in internal_network_markers {
        if let Some(idx) = text.find(marker) {
            let snippet_end = (idx + 16).min(text.len());
            findings.push(PrivacyFinding {
                rule: "internal_network".into(),
                severity: FindingSeverity::Block,
                detail: format!("contains internal network address/domain '{marker}'"),
                snippet: text[idx..snippet_end].into(),
            });
        }
    }

    // 6. Raw conversation sessions / chat history
    let raw_session_markers = [
        "\"role\": \"user\"",
        "\"role\": \"assistant\"",
        "chat_history",
    ];
    for marker in raw_session_markers {
        if text.contains(marker) {
            findings.push(PrivacyFinding {
                rule: "raw_session_data".into(),
                severity: FindingSeverity::Block,
                detail: format!("contains raw session conversation marker '{marker}'"),
                snippet: marker.into(),
            });
        }
    }

    // 7. Sensitive credential parameters
    let credential_markers = [
        "password=",
        "api_secret=",
        "client_secret=",
        "secret_key=",
        "aws_secret_access_key",
    ];
    for marker in credential_markers {
        if text.to_ascii_lowercase().contains(marker) {
            findings.push(PrivacyFinding {
                rule: "credential_parameter".into(),
                severity: FindingSeverity::Block,
                detail: format!("contains sensitive credential parameter '{marker}'"),
                snippet: marker.into(),
            });
        }
    }

    // 8. Warnings (e.g. TODO flags)
    if text.contains("TODO: secret") || text.contains("FIXME: token") {
        findings.push(PrivacyFinding {
            rule: "suspicious_comment".into(),
            severity: FindingSeverity::Warning,
            detail: "contains suspicious todo comment referencing secret/token".into(),
            snippet: "TODO/FIXME reference".into(),
        });
    }

    let status = if findings
        .iter()
        .any(|f| f.severity == FindingSeverity::Block)
    {
        RedactionStatus::Blocked
    } else if findings
        .iter()
        .any(|f| f.severity == FindingSeverity::Warning)
    {
        RedactionStatus::Warning
    } else {
        RedactionStatus::Clean
    };

    RedactionReport {
        scanned_at: now(),
        findings_count: findings.len(),
        status,
        disclaimer: PRIVACY_DISCLAIMER.into(),
        findings,
    }
}

pub fn privacy_block(text: &str) -> Result<()> {
    let report = scan_privacy(text);
    if report.status == RedactionStatus::Blocked {
        return Err(Error::Invalid(format!(
            "privacy_gate: {}",
            report
                .findings
                .first()
                .map(|f| f.detail.as_str())
                .unwrap_or("sensitive data blocked")
        )));
    }
    Ok(())
}

pub fn foreign_approval_is_not_local(_: &str) -> bool {
    true
}

pub fn validate_manifest(manifest: &AssetPackageManifest) -> Result<()> {
    if manifest.schema_version != PACKAGE_SCHEMA_V1 {
        return Err(Error::Invalid(format!(
            "unsupported package schema: expected {}, got {}",
            PACKAGE_SCHEMA_V1, manifest.schema_version
        )));
    }
    identifier(&manifest.publisher)?;
    identifier(&manifest.asset_id)?;
    let complete_manifest = serde_json::to_string(manifest).map_err(|_| Error::Internal)?;
    privacy_block(&complete_manifest)?;

    if manifest.name.trim().is_empty() {
        return Err(Error::Invalid("empty package name".into()));
    }
    if manifest.version.trim().is_empty() {
        return Err(Error::Invalid("empty package version".into()));
    }

    let lic = manifest.license.trim();
    if lic.is_empty()
        || lic.eq_ignore_ascii_case("unknown")
        || lic.eq_ignore_ascii_case("unlicensed")
        || lic.contains("proprietary-internal")
        || lic.contains("no-export")
    {
        return Err(Error::Forbidden);
    }

    // Privacy scan manifest metadata
    privacy_block(&manifest.description)?;
    privacy_block(&manifest.name)?;
    privacy_block(&manifest.publisher)?;
    privacy_block(&manifest.license)?;
    for dep in &manifest.dependencies {
        privacy_block(&dep.publisher)?;
        privacy_block(&dep.asset_id)?;
        privacy_block(&dep.kind)?;
        privacy_block(&dep.version_req)?;
    }
    if let Some(fm) = &manifest.foreign_metadata {
        if let Some(ns) = &fm.external_namespace {
            privacy_block(ns)?;
        }
        if let Some(pid) = &fm.external_parent_id {
            privacy_block(pid)?;
        }
        for eval in &fm.formal_evaluations {
            privacy_block(&eval.notes)?;
            privacy_block(&eval.evaluator_identity)?;
        }
        for app in &fm.approvals {
            privacy_block(&app.notes)?;
            privacy_block(&app.approver_identity)?;
        }
    }

    let members: Vec<PackageMember> = manifest
        .entries
        .iter()
        .map(|e| PackageMember {
            path: e.path.clone(),
            size: e.size_bytes,
            compressed: e.compressed_bytes,
        })
        .collect();
    reject_package(&members)?;

    for entry in &manifest.entries {
        if entry.digest_sha256.len() != 64
            || !entry.digest_sha256.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err(Error::Invalid(format!(
                "invalid sha256 digest for entry: {}",
                entry.path
            )));
        }
    }

    for dep in &manifest.dependencies {
        identifier(&dep.publisher)?;
        identifier(&dep.asset_id)?;
    }

    if manifest.redaction_report.status == RedactionStatus::Blocked {
        return Err(Error::Invalid(
            "package manifest has blocked redaction status".into(),
        ));
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageState {
    Staged,
    Quarantined,
    Rejected,
    Active,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedPackage {
    pub manifest: AssetPackageManifest,
    pub files: BTreeMap<String, Vec<u8>>,
    pub state: PackageState,
    pub quarantine_reason: Option<String>,
    pub is_active: bool,
    pub is_approved: bool,
    pub local_formal_evaluation_id: Option<String>,
    pub local_approval_id: Option<String>,
    pub untrusted_foreign_metadata: Option<ForeignMetadata>,
}

pub fn import_external_package(
    manifest: &AssetPackageManifest,
    files: &BTreeMap<String, Vec<u8>>,
    is_dep_available: impl Fn(&str, &str) -> bool,
    is_dep_revoked: impl Fn(&str, &str) -> bool,
) -> Result<StagedPackage> {
    validate_manifest(manifest)?;

    if files.len() != manifest.entries.len() {
        return Err(Error::Invalid(format!(
            "manifest entries count {} does not match files count {}",
            manifest.entries.len(),
            files.len()
        )));
    }

    for entry in &manifest.entries {
        let content = files
            .get(&entry.path)
            .ok_or_else(|| Error::Invalid(format!("missing file from entries: {}", entry.path)))?;
        if content.len() as u64 != entry.size_bytes {
            return Err(Error::Invalid(format!(
                "file size mismatch for {}: expected {}, got {}",
                entry.path,
                entry.size_bytes,
                content.len()
            )));
        }
        let actual_digest = hash(content);
        if actual_digest != entry.digest_sha256 {
            return Err(Error::Invalid(format!(
                "sha256 digest mismatch for {}: expected {}, got {}",
                entry.path, entry.digest_sha256, actual_digest
            )));
        }
        let text_content = std::str::from_utf8(content)
            .map_err(|_| Error::Invalid(format!("non-utf8 content in file: {}", entry.path)))?;
        privacy_block(text_content)?;
    }

    // Check dependencies
    let mut quarantine_reason = None;
    for dep in &manifest.dependencies {
        if is_dep_revoked(&dep.publisher, &dep.asset_id) {
            quarantine_reason = Some(format!(
                "revoked_dependency: {}:{}",
                dep.publisher, dep.asset_id
            ));
            break;
        }
        if !is_dep_available(&dep.publisher, &dep.asset_id) {
            quarantine_reason = Some(format!(
                "unresolved_dependency: {}:{}",
                dep.publisher, dep.asset_id
            ));
            break;
        }
    }

    let state = if quarantine_reason.is_some() {
        PackageState::Quarantined
    } else {
        PackageState::Staged
    };

    Ok(StagedPackage {
        manifest: manifest.clone(),
        files: files.clone(),
        state,
        quarantine_reason,
        is_active: false,
        is_approved: false,
        local_formal_evaluation_id: None,
        local_approval_id: None,
        untrusted_foreign_metadata: manifest.foreign_metadata.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileDiff {
    pub path: String,
    pub old_digest: Option<String>,
    pub new_digest: Option<String>,
    pub change_type: String, // "added", "modified", "deleted", "unchanged"
    pub additions: usize,
    pub deletions: usize,
    pub diff_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffPreview {
    pub asset_id: String,
    pub kind: String,
    pub changed_files: Vec<FileDiff>,
    pub total_additions: usize,
    pub total_deletions: usize,
    pub summary: String,
    pub requires_new_approval: bool,
    pub permission_escalated: bool,
}

pub fn preview_package_diff(
    asset_id: &str,
    kind: &str,
    local_files: &BTreeMap<String, String>,
    new_files: &BTreeMap<String, String>,
) -> DiffPreview {
    let mut all_paths = BTreeSet::new();
    for p in local_files.keys() {
        all_paths.insert(p.clone());
    }
    for p in new_files.keys() {
        all_paths.insert(p.clone());
    }

    let mut changed_files = Vec::new();
    let mut total_additions = 0;
    let mut total_deletions = 0;
    let mut any_content_changed = false;

    for p in all_paths {
        let old_content = local_files.get(&p);
        let new_content = new_files.get(&p);

        match (old_content, new_content) {
            (None, Some(new_c)) => {
                let lines = new_c.lines().count();
                total_additions += lines;
                any_content_changed = true;
                changed_files.push(FileDiff {
                    path: p,
                    old_digest: None,
                    new_digest: Some(hash(new_c.as_bytes())),
                    change_type: "added".into(),
                    additions: lines,
                    deletions: 0,
                    diff_summary: format!("+{} lines", lines),
                });
            }
            (Some(old_c), None) => {
                let lines = old_c.lines().count();
                total_deletions += lines;
                any_content_changed = true;
                changed_files.push(FileDiff {
                    path: p,
                    old_digest: Some(hash(old_c.as_bytes())),
                    new_digest: None,
                    change_type: "deleted".into(),
                    additions: 0,
                    deletions: lines,
                    diff_summary: format!("-{} lines", lines),
                });
            }
            (Some(old_c), Some(new_c)) => {
                let old_d = hash(old_c.as_bytes());
                let new_d = hash(new_c.as_bytes());
                if old_d == new_d {
                    changed_files.push(FileDiff {
                        path: p,
                        old_digest: Some(old_d),
                        new_digest: Some(new_d),
                        change_type: "unchanged".into(),
                        additions: 0,
                        deletions: 0,
                        diff_summary: "identical".into(),
                    });
                } else {
                    let old_lines = old_c.lines().count();
                    let new_lines = new_c.lines().count();
                    total_additions += new_lines;
                    total_deletions += old_lines;
                    any_content_changed = true;
                    changed_files.push(FileDiff {
                        path: p,
                        old_digest: Some(old_d),
                        new_digest: Some(new_d),
                        change_type: "modified".into(),
                        additions: new_lines,
                        deletions: old_lines,
                        diff_summary: format!("-{} +{} lines", old_lines, new_lines),
                    });
                }
            }
            (None, None) => unreachable!(),
        }
    }

    DiffPreview {
        asset_id: asset_id.into(),
        kind: kind.into(),
        changed_files,
        total_additions,
        total_deletions,
        summary: format!(
            "Total +{} -{} across files",
            total_additions, total_deletions
        ),
        requires_new_approval: any_content_changed,
        permission_escalated: false,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub publisher: String,
    pub asset_id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub kind: PackageKind,
    pub license: String,
    pub files: BTreeMap<String, Vec<u8>>,
    pub dependencies: Vec<PackageDependency>,
    pub referenced_source_ids: Vec<String>,
    pub export_started_watermark: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryAuditRecord {
    pub package_id: String,
    pub publisher: String,
    pub delivered_to: String,
    pub delivered_at: i64,
    pub watermark_at_delivery: u64,
    pub revoked: bool,
    pub revocation_notice_sent: bool,
    pub remote_erasure_disclaimer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedExportPackage {
    pub manifest: AssetPackageManifest,
    pub files: BTreeMap<String, Vec<u8>>,
}

pub fn export_package(
    req: &ExportRequest,
    is_source_revoked: impl Fn(&str) -> bool,
    current_watermark: u64,
) -> Result<PreparedExportPackage> {
    identifier(&req.publisher)?;
    identifier(&req.asset_id)?;

    let lic = req.license.trim();
    if lic.is_empty()
        || lic.eq_ignore_ascii_case("unknown")
        || lic.eq_ignore_ascii_case("unlicensed")
        || lic.contains("proprietary-internal")
        || lic.contains("no-export")
    {
        return Err(Error::Forbidden);
    }

    // Check revocation watermark: if watermark advanced since export started, fail
    if current_watermark > req.export_started_watermark {
        return Err(Error::Conflict(
            "source_revoked_during_export: revocation watermark advanced".into(),
        ));
    }

    // Check referenced sources: if any source was revoked, fail immediately
    for src in &req.referenced_source_ids {
        if is_source_revoked(src) {
            return Err(Error::Conflict(format!(
                "source_revoked_during_export: referenced source '{src}' is revoked"
            )));
        }
    }

    // Validate metadata privacy scan
    let mut all_findings = Vec::new();
    let metadata_fields = [
        ("description", req.description.as_str()),
        ("name", req.name.as_str()),
        ("publisher", req.publisher.as_str()),
        ("license", req.license.as_str()),
        ("version", req.version.as_str()),
        ("asset_id", req.asset_id.as_str()),
    ];
    for (field_name, field_val) in metadata_fields {
        let report = scan_privacy(field_val);
        if report.status == RedactionStatus::Blocked {
            return Err(Error::Invalid(format!(
                "privacy_gate: sensitive content detected in metadata field '{field_name}'"
            )));
        }
        all_findings.extend(report.findings);
    }
    for dep in &req.dependencies {
        let dep_fields = [
            ("dependency.publisher", dep.publisher.as_str()),
            ("dependency.asset_id", dep.asset_id.as_str()),
            ("dependency.kind", dep.kind.as_str()),
            ("dependency.version_req", dep.version_req.as_str()),
        ];
        for (field_name, field_val) in dep_fields {
            let report = scan_privacy(field_val);
            if report.status == RedactionStatus::Blocked {
                return Err(Error::Invalid(format!(
                    "privacy_gate: sensitive content detected in metadata field '{field_name}'"
                )));
            }
            all_findings.extend(report.findings);
        }
    }
    for src in &req.referenced_source_ids {
        let report = scan_privacy(src);
        if report.status == RedactionStatus::Blocked {
            return Err(Error::Invalid(
                "privacy_gate: sensitive content detected in referenced_source_id".into(),
            ));
        }
        all_findings.extend(report.findings);
    }

    // Validate files and privacy scan
    let mut entries = Vec::new();

    for (path, content) in &req.files {
        let member = PackageMember {
            path: path.clone(),
            size: content.len() as u64,
            compressed: content.len() as u64,
        };
        reject_member(&member)?;

        let text_content = std::str::from_utf8(content)
            .map_err(|_| Error::Invalid(format!("non-utf8 content in file: {path}")))?;
        let report = scan_privacy(text_content);
        if report.status == RedactionStatus::Blocked {
            return Err(Error::Invalid(format!(
                "privacy_gate: sensitive content detected in file '{path}'"
            )));
        }
        all_findings.extend(report.findings);

        entries.push(PackageEntry {
            path: path.clone(),
            digest_sha256: hash(content),
            size_bytes: content.len() as u64,
            compressed_bytes: content.len() as u64,
        });
    }

    let members: Vec<PackageMember> = entries
        .iter()
        .map(|e| PackageMember {
            path: e.path.clone(),
            size: e.size_bytes,
            compressed: e.compressed_bytes,
        })
        .collect();
    reject_package(&members)?;

    let redaction_status = if all_findings
        .iter()
        .any(|f| f.severity == FindingSeverity::Warning)
    {
        RedactionStatus::Warning
    } else {
        RedactionStatus::Clean
    };

    let redaction_report = RedactionReport {
        scanned_at: now(),
        findings_count: all_findings.len(),
        status: redaction_status,
        disclaimer: PRIVACY_DISCLAIMER.into(),
        findings: all_findings,
    };

    let manifest = AssetPackageManifest {
        schema_version: PACKAGE_SCHEMA_V1.into(),
        kind: req.kind,
        publisher: req.publisher.clone(),
        asset_id: req.asset_id.clone(),
        name: req.name.clone(),
        version: req.version.clone(),
        description: req.description.clone(),
        license: req.license.clone(),
        entries,
        dependencies: req.dependencies.clone(),
        redaction_report,
        foreign_metadata: None,
    };

    validate_manifest(&manifest)?;

    Ok(PreparedExportPackage {
        manifest,
        files: req.files.clone(),
    })
}

pub fn mark_delivered_package_revoked(record: &mut DeliveryAuditRecord) {
    record.revoked = true;
    record.revocation_notice_sent = true;
}

#[derive(Debug, Clone, Default)]
pub struct DependencyTracker {
    pub references: BTreeMap<String, BTreeSet<String>>,
}

impl DependencyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_dependency(&mut self, package_id: &str, dep_id: &str) {
        self.references
            .entry(dep_id.to_string())
            .or_default()
            .insert(package_id.to_string());
    }

    pub fn can_safely_uninstall(&self, dep_id: &str, requesting_package_id: &str) -> Result<bool> {
        if let Some(dependents) = self.references.get(dep_id) {
            let other_dependents: Vec<_> = dependents
                .iter()
                .filter(|id| id.as_str() != requesting_package_id)
                .collect();
            if !other_dependents.is_empty() {
                return Err(Error::Conflict(format!(
                    "shared dependency '{dep_id}' is still required by other packages: {:?}",
                    other_dependents
                )));
            }
        }
        Ok(true)
    }

    pub fn unregister_package(&mut self, package_id: &str) {
        for dependents in self.references.values_mut() {
            dependents.remove(package_id);
        }
    }
}

pub const E16_STAGED_ASSET_SCHEMA: &str = "rsia.e16.staged_asset.v1";
pub const E16_EXPORT_ATTEMPT_SCHEMA: &str = "rsia.e16.export_attempt.v2";
pub const E16_DELIVERY_AUDIT_SCHEMA: &str = "rsia.e16.delivery_audit.v2";
pub const LOCAL_EXPORT_DESTINATION: &str = "local_namespace";
pub const LOCAL_EXPORT_KIND: &str = "local_directory_v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct E16SourceRef {
    pub kind: String,
    pub id: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct E16Envelope<T> {
    pub schema_version: String,
    pub id: String,
    pub namespace: String,
    pub owner_actor: String,
    pub request_key: String,
    pub input_digest: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub source_refs: Vec<E16SourceRef>,
    pub revoke_watermark: u64,
    pub payload: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagedAssetState {
    Prepared,
    Staged,
    Quarantined,
    Aborted,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedAssetPayload {
    pub publisher: String,
    pub asset_id: String,
    pub package_kind: String,
    pub manifest_digest: String,
    pub content_blob_digest: String,
    pub content_bytes: u64,
    pub baseline_digest: String,
    pub local_digest: String,
    pub upstream_digest: Option<String>,
    pub environment_digest: String,
    pub package_schema_version: String,
    pub compiler_version: String,
    pub state: StagedAssetState,
    pub quarantine_reason: Option<String>,
    pub candidate_material: Option<CandidateMaterialBinding>,
    pub candidate_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateMaterialBinding {
    pub skill_snapshot_entry_path: String,
    pub strategy_entry_path: String,
    pub strategy_authority: CandidateStrategyAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStrategyAuthority {
    BuiltinDefaultV1,
}

#[derive(Debug, Clone)]
pub struct StagePackageRequest {
    pub request_key: String,
    pub manifest: AssetPackageManifest,
    pub files: BTreeMap<String, Vec<u8>>,
    pub source_refs: Vec<E16SourceRef>,
    pub baseline_digest: String,
    pub local_digest: String,
    pub upstream_digest: Option<String>,
    pub environment_digest: String,
    pub compiler_version: String,
    pub candidate_material: Option<CandidateMaterialBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportAttemptState {
    Prepared,
    Completed,
    Aborted,
    Revoked,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct E04BudgetRef {
    pub root_budget_id: String,
    pub billing_scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportAttemptPayload {
    pub publisher: String,
    pub asset_id: String,
    pub staged_asset_id: String,
    pub manifest_digest: String,
    pub projection_blob_digest: String,
    pub projection_blob_bytes: u64,
    pub package_tree_digest: String,
    pub delivered_bytes: u64,
    pub delivery_kind: String,
    pub local_export_ref: String,
    pub state: ExportAttemptState,
    pub destination_scope: String,
    pub delivery_audit_id: Option<String>,
    pub completion_receipt_digest: Option<String>,
    pub budget_ref: Option<E04BudgetRef>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryAuditPayload {
    pub export_attempt_id: String,
    pub package_id: String,
    pub projection_blob_digest: String,
    pub delivery_kind: String,
    pub local_export_ref: String,
    pub package_tree_digest: String,
    pub delivered_bytes: u64,
    pub completion_receipt_digest: String,
    pub delivered_to: String,
    pub delivered_at: i64,
    pub watermark_at_delivery: u64,
    pub revoked: bool,
    pub revocation_notice_sent: bool,
    pub remote_erasure_disclaimer: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageProjection {
    manifest: AssetPackageManifest,
    files: BTreeMap<String, String>,
}

fn build_package_projection(
    manifest: &AssetPackageManifest,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>> {
    validate_manifest(manifest)?;
    if files.len() != manifest.entries.len() {
        return Err(Error::Invalid("manifest/file count differs".into()));
    }
    let manifest_bytes = serde_json::to_vec(manifest).map_err(|_| Error::Internal)?;
    let mut raw_bytes = manifest_bytes.len() as u64;
    let mut projected = BTreeMap::new();
    for entry in &manifest.entries {
        let bytes = files
            .get(&entry.path)
            .ok_or_else(|| Error::Invalid(format!("missing file {}", entry.path)))?;
        if bytes.len() as u64 != entry.size_bytes || hash(bytes) != entry.digest_sha256 {
            return Err(Error::Conflict(format!(
                "file content differs from manifest: {}",
                entry.path
            )));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| Error::Invalid(format!("non-utf8 content: {}", entry.path)))?;
        raw_bytes = raw_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| Error::Invalid("package raw size overflow".into()))?;
        projected.insert(entry.path.clone(), text.into());
    }
    if raw_bytes > MAX_TOTAL {
        return Err(Error::Invalid(format!(
            "package manifest and members exceed {MAX_TOTAL} bytes"
        )));
    }
    let bytes = serde_json::to_vec(&PackageProjection {
        manifest: manifest.clone(),
        files: projected,
    })
    .map_err(|_| Error::Internal)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| Error::Internal)?;
    privacy_block(text)?;
    if bytes.len() > MAX_PERSISTENT_PROJECTION {
        return Err(Error::Invalid(
            "encoded package projection exceeds 64 MiB blob scope".into(),
        ));
    }
    Ok(bytes)
}

fn decode_package_projection(bytes: &[u8]) -> Result<PackageProjection> {
    if bytes.is_empty() || bytes.len() > MAX_PERSISTENT_PROJECTION {
        return Err(Error::Invalid(
            "invalid encoded package projection size".into(),
        ));
    }
    let projection: PackageProjection = serde_json::from_slice(bytes)
        .map_err(|_| Error::Invalid("invalid stored package projection".into()))?;
    let files = projection
        .files
        .iter()
        .map(|(path, content)| (path.clone(), content.as_bytes().to_vec()))
        .collect::<BTreeMap<_, _>>();
    if build_package_projection(&projection.manifest, &files)? != bytes {
        return Err(Error::Conflict(
            "stored package projection is not canonical".into(),
        ));
    }
    Ok(projection)
}

fn local_export_files(projection: &PackageProjection) -> Result<Vec<LocalExportFile>> {
    validate_manifest(&projection.manifest)?;
    if projection.files.contains_key("manifest.json") {
        return Err(Error::Invalid(
            "package member path manifest.json is reserved".into(),
        ));
    }
    let manifest = serde_json::to_vec(&projection.manifest).map_err(|_| Error::Internal)?;
    privacy_block(std::str::from_utf8(&manifest).map_err(|_| Error::Internal)?)?;
    let mut files = vec![LocalExportFile {
        path: "manifest.json".into(),
        bytes: manifest,
    }];
    for entry in &projection.manifest.entries {
        let content = projection
            .files
            .get(&entry.path)
            .ok_or_else(|| Error::Invalid("export projection member is missing".into()))?;
        let bytes = content.as_bytes().to_vec();
        if bytes.len() as u64 != entry.size_bytes || hash(&bytes) != entry.digest_sha256 {
            return Err(Error::Conflict(
                "export projection differs from manifest".into(),
            ));
        }
        privacy_block(content)?;
        files.push(LocalExportFile {
            path: entry.path.clone(),
            bytes,
        });
    }
    local_export_tree_digest(&files)?;
    Ok(files)
}

fn validate_candidate_material(
    manifest: &AssetPackageManifest,
    files: &BTreeMap<String, Vec<u8>>,
    binding: &CandidateMaterialBinding,
) -> Result<(SkillSnapshot, Strategy)> {
    if manifest.kind != PackageKind::Skill {
        return Err(Error::Invalid(
            "candidate material binding only supports skill packages".into(),
        ));
    }
    if binding.skill_snapshot_entry_path == binding.strategy_entry_path {
        return Err(Error::Invalid(
            "skill and strategy material paths must differ".into(),
        ));
    }
    for path in [
        binding.skill_snapshot_entry_path.as_str(),
        binding.strategy_entry_path.as_str(),
    ] {
        if !manifest.entries.iter().any(|entry| entry.path == path) {
            return Err(Error::Invalid(format!(
                "candidate material path is absent from manifest: {path}"
            )));
        }
    }
    let skill: SkillSnapshot = serde_json::from_slice(
        files
            .get(&binding.skill_snapshot_entry_path)
            .ok_or_else(|| Error::Invalid("missing skill material bytes".into()))?,
    )
    .map_err(|_| Error::Invalid("invalid SkillSnapshot material".into()))?;
    let strategy: Strategy = serde_json::from_slice(
        files
            .get(&binding.strategy_entry_path)
            .ok_or_else(|| Error::Invalid("missing strategy material bytes".into()))?,
    )
    .map_err(|_| Error::Invalid("invalid Strategy material".into()))?;
    skill.validate()?;
    strategy.validate()?;
    match binding.strategy_authority {
        CandidateStrategyAuthority::BuiltinDefaultV1 => {
            if fingerprint(&strategy)? != fingerprint(&Strategy::default())? {
                return Err(Error::Forbidden);
            }
        }
    }
    Ok((skill, strategy))
}

pub(crate) fn e16_storage_id(
    kind: &str,
    namespace: &str,
    owner: &str,
    key: &str,
) -> Result<String> {
    identifier(kind)?;
    identifier(namespace)?;
    identifier(owner)?;
    identifier(key)?;
    Ok(format!(
        "e16-{kind}-{}",
        fingerprint(&(namespace, owner, key))?
    ))
}

pub(crate) fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(format!("{name} must be lowercase sha256")));
    }
    Ok(())
}

pub(crate) fn canonical_sources(mut sources: Vec<E16SourceRef>) -> Result<Vec<E16SourceRef>> {
    sources.sort_by(|left, right| (&left.kind, &left.id).cmp(&(&right.kind, &right.id)));
    for source in &sources {
        identifier(&source.kind)?;
        identifier(&source.id)?;
        validate_digest(&source.digest, "source digest")?;
    }
    if sources
        .windows(2)
        .any(|pair| pair[0].kind == pair[1].kind && pair[0].id == pair[1].id)
    {
        return Err(Error::Conflict("duplicate E16 source ref".into()));
    }
    Ok(sources)
}

pub(crate) async fn current_watermark(session: &mut Session, ctx: &Context) -> Result<u64> {
    session
        .watermark(ctx)
        .await?
        .and_then(|(sequence, _)| u64::try_from(sequence).ok())
        .ok_or_else(|| Error::Conflict("missing current revocation watermark".into()))
}

pub(crate) async fn verify_sources(
    session: &mut Session,
    ctx: &Context,
    sources: &[E16SourceRef],
) -> Result<()> {
    for source in sources {
        if session
            .get::<serde_json::Value>(ctx, "tombstone", &source.id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
        let value = session.raw_object(ctx, &source.kind, &source.id).await?;
        if fingerprint(&value)? != source.digest {
            return Err(Error::Conflict(format!(
                "source changed: {}:{}",
                source.kind, source.id
            )));
        }
    }
    Ok(())
}

pub(crate) async fn put_envelope<T: Serialize>(
    session: &mut Session,
    ctx: &Context,
    envelope: &E16Envelope<T>,
) -> Result<()> {
    session
        .put(ctx, "artifact", &envelope.id, ctx.actor(), envelope)
        .await?;
    for source in &envelope.source_refs {
        session
            .put_edge(ctx, "artifact", &envelope.id, &source.kind, &source.id)
            .await?;
    }
    Ok(())
}

pub struct PersistentPackageStore;

impl PersistentPackageStore {
    pub async fn stage_package(
        ctx: &Context,
        store: &Store,
        request: StagePackageRequest,
    ) -> Result<E16Envelope<StagedAssetPayload>> {
        ctx.require(&[Role::Admin])?;
        identifier(&request.request_key)?;
        validate_digest(&request.baseline_digest, "baseline_digest")?;
        validate_digest(&request.local_digest, "local_digest")?;
        if let Some(upstream) = &request.upstream_digest {
            validate_digest(upstream, "upstream_digest")?;
        }
        validate_digest(&request.environment_digest, "environment_digest")?;
        identifier(&request.compiler_version)?;
        let projection = build_package_projection(&request.manifest, &request.files)?;
        let projection_digest = hash(&projection);
        if let Some(binding) = &request.candidate_material {
            validate_candidate_material(&request.manifest, &request.files, binding)?;
        }
        let sources = canonical_sources(request.source_refs)?;
        let id = e16_storage_id(
            "staged-asset",
            ctx.namespace(),
            ctx.actor(),
            &request.request_key,
        )?;
        let input_digest = fingerprint(&(
            &request.manifest,
            projection_digest.as_str(),
            &sources,
            request.baseline_digest.as_str(),
            request.local_digest.as_str(),
            request.upstream_digest.as_deref(),
            request.environment_digest.as_str(),
            request.compiler_version.as_str(),
            &request.candidate_material,
        ))?;
        let mut session = store.session().await?;
        let watermark = current_watermark(&mut session, ctx).await?;
        verify_sources(&mut session, ctx, &sources).await?;
        let mut quarantine = None;
        let mut dependency_refs = Vec::new();
        for dependency in &request.manifest.dependencies {
            let dependency_id = package_dependency_ref_id(dependency)?;
            match session
                .get::<serde_json::Value>(ctx, "artifact", &dependency_id)
                .await?
            {
                Some(value) => dependency_refs.push(E16SourceRef {
                    kind: "artifact".into(),
                    id: dependency_id,
                    digest: fingerprint(&value)?,
                }),
                None => {
                    quarantine = Some(format!(
                        "unresolved_dependency:{}:{}",
                        dependency.publisher, dependency.asset_id
                    ));
                }
            }
        }
        let mut all_sources = sources;
        all_sources.extend(dependency_refs);
        let all_sources = canonical_sources(all_sources)?;
        if let Some(existing) = session
            .get::<E16Envelope<StagedAssetPayload>>(ctx, "artifact", &id)
            .await?
        {
            if existing.schema_version != E16_STAGED_ASSET_SCHEMA
                || existing.input_digest != input_digest
                || existing.owner_actor != ctx.actor()
                || existing.namespace != ctx.namespace()
                || existing.revoke_watermark != watermark
                || existing.source_refs != all_sources
                || existing.payload.content_blob_digest != projection_digest
                || existing.payload.content_bytes != projection.len() as u64
                || existing.payload.candidate_material != request.candidate_material
                || existing.payload.quarantine_reason != quarantine
            {
                return Err(Error::Conflict(
                    "staged asset request key or frozen inputs changed".into(),
                ));
            }
            verify_sources(&mut session, ctx, &existing.source_refs).await?;
            if existing.payload.state != StagedAssetState::Prepared {
                session.commit().await?;
                return Self::read_staged(ctx, store, &existing.id).await;
            }
        } else {
            let timestamp = now();
            let envelope = E16Envelope {
                schema_version: E16_STAGED_ASSET_SCHEMA.into(),
                id: id.clone(),
                namespace: ctx.namespace().into(),
                owner_actor: ctx.actor().into(),
                request_key: request.request_key,
                input_digest: input_digest.clone(),
                created_at: timestamp,
                updated_at: timestamp,
                source_refs: all_sources.clone(),
                revoke_watermark: watermark,
                payload: StagedAssetPayload {
                    publisher: request.manifest.publisher.clone(),
                    asset_id: request.manifest.asset_id.clone(),
                    package_kind: format!("{:?}", request.manifest.kind).to_ascii_lowercase(),
                    manifest_digest: fingerprint(&request.manifest)?,
                    content_blob_digest: projection_digest.clone(),
                    content_bytes: projection.len() as u64,
                    baseline_digest: request.baseline_digest,
                    local_digest: request.local_digest,
                    upstream_digest: request.upstream_digest,
                    environment_digest: request.environment_digest,
                    package_schema_version: request.manifest.schema_version,
                    compiler_version: request.compiler_version,
                    state: StagedAssetState::Prepared,
                    quarantine_reason: quarantine.clone(),
                    candidate_material: request.candidate_material,
                    candidate_ref: None,
                },
            };
            put_envelope(&mut session, ctx, &envelope).await?;
            session
                .audit(ctx, "e16.staged_asset.prepare", &envelope.id)
                .await?;
        }
        session.commit().await?;
        let blob_digest = store
            .publish_registered_blob(
                ctx,
                &id,
                E16_STAGED_ASSET_SCHEMA,
                "content_blob_digest",
                &projection,
                MAX_PERSISTENT_PROJECTION,
            )
            .await?;
        if blob_digest != projection_digest {
            return Err(Error::Internal);
        }
        let mut session = store.session().await?;
        let mut current: E16Envelope<StagedAssetPayload> =
            session.need(ctx, "artifact", &id).await?;
        if current.schema_version != E16_STAGED_ASSET_SCHEMA
            || current.owner_actor != ctx.actor()
            || current.namespace != ctx.namespace()
            || current.input_digest != input_digest
            || current.payload.state != StagedAssetState::Prepared
            || current.payload.content_blob_digest != projection_digest
            || current.revoke_watermark != current_watermark(&mut session, ctx).await?
        {
            return Err(Error::Conflict(
                "staged asset changed before blob finalization".into(),
            ));
        }
        verify_sources(&mut session, ctx, &current.source_refs).await?;
        current.payload.state = if quarantine.is_some() {
            StagedAssetState::Quarantined
        } else {
            StagedAssetState::Staged
        };
        current.updated_at = now();
        put_envelope(&mut session, ctx, &current).await?;
        session
            .audit(ctx, "e16.staged_asset.finalize", &current.id)
            .await?;
        session.commit().await?;
        Self::read_staged(ctx, store, &id).await
    }

    pub async fn read_staged(
        ctx: &Context,
        store: &Store,
        id: &str,
    ) -> Result<E16Envelope<StagedAssetPayload>> {
        ctx.require(&[Role::Admin, Role::Worker])?;
        let mut session = store.session().await?;
        let envelope: E16Envelope<StagedAssetPayload> = session.need(ctx, "artifact", id).await?;
        if envelope.schema_version != E16_STAGED_ASSET_SCHEMA
            || envelope.namespace != ctx.namespace()
            || current_watermark(&mut session, ctx).await? != envelope.revoke_watermark
        {
            return Err(Error::Conflict("staged asset is stale or invalid".into()));
        }
        if ctx.role() == Role::Worker
            && (ctx.actor() != envelope.owner_actor
                || envelope.payload.state != StagedAssetState::Staged)
        {
            return Err(Error::Forbidden);
        }
        if envelope.payload.state == StagedAssetState::Prepared {
            return Err(Error::Conflict(
                "staged asset content is not finalized".into(),
            ));
        }
        verify_sources(&mut session, ctx, &envelope.source_refs).await?;
        session.commit().await?;
        let bytes = store
            .read_blob(
                ctx,
                &envelope.payload.content_blob_digest,
                MAX_PERSISTENT_PROJECTION,
            )
            .await?;
        if hash(&bytes) != envelope.payload.content_blob_digest
            || bytes.len() as u64 != envelope.payload.content_bytes
        {
            return Err(Error::Conflict("staged asset blob differs".into()));
        }
        if envelope.payload.package_schema_version == PACKAGE_SCHEMA_V1 {
            let projection = decode_package_projection(&bytes)?;
            if fingerprint(&projection.manifest)? != envelope.payload.manifest_digest {
                return Err(Error::Conflict(
                    "staged manifest differs from registered digest".into(),
                ));
            }
        } else if envelope.payload.package_schema_version != "rsia.seed.reset.v1" {
            return Err(Error::Invalid(
                "unsupported staged asset content schema".into(),
            ));
        }
        let mut session = store.session().await?;
        let current: E16Envelope<StagedAssetPayload> = session.need(ctx, "artifact", id).await?;
        if fingerprint(&current)? != fingerprint(&envelope)?
            || current_watermark(&mut session, ctx).await? != envelope.revoke_watermark
        {
            return Err(Error::Conflict(
                "staged asset changed during content read".into(),
            ));
        }
        verify_sources(&mut session, ctx, &current.source_refs).await?;
        session.commit().await?;
        Ok(envelope)
    }

    pub async fn abort_staged(
        ctx: &Context,
        store: &Store,
        id: &str,
    ) -> Result<E16Envelope<StagedAssetPayload>> {
        ctx.require(&[Role::Admin])?;
        let mut session = store.session().await?;
        let mut envelope: E16Envelope<StagedAssetPayload> =
            session.need(ctx, "artifact", id).await?;
        if envelope.owner_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        if envelope.payload.state == StagedAssetState::Aborted {
            session.commit().await?;
            return Ok(envelope);
        }
        if !matches!(
            envelope.payload.state,
            StagedAssetState::Prepared | StagedAssetState::Staged | StagedAssetState::Quarantined
        ) {
            return Err(Error::Conflict("staged asset is terminal".into()));
        }
        envelope.payload.state = StagedAssetState::Aborted;
        envelope.updated_at = now();
        put_envelope(&mut session, ctx, &envelope).await?;
        session.audit(ctx, "e16.staged_asset.abort", id).await?;
        session.commit().await?;
        Ok(envelope)
    }

    /// Persist the handoff to an existing E02 release candidate without granting
    /// approval or changing the Active pointer. The candidate remains subject to
    /// the existing E05/E06 evaluation and approval path.
    pub async fn record_candidate_handoff(
        ctx: &Context,
        store: &Store,
        staged_asset_id: &str,
        candidate_id: &str,
    ) -> Result<E16Envelope<StagedAssetPayload>> {
        ctx.require(&[Role::Admin])?;
        identifier(staged_asset_id)?;
        identifier(candidate_id)?;
        let staged = Self::read_staged(ctx, store, staged_asset_id).await?;
        if staged.owner_actor != ctx.actor() || staged.payload.state != StagedAssetState::Staged {
            return Err(Error::Forbidden);
        }
        if staged.payload.package_schema_version != PACKAGE_SCHEMA_V1 {
            return Err(Error::Invalid(
                "candidate handoff requires a validated package projection".into(),
            ));
        }
        let binding = staged.payload.candidate_material.as_ref().ok_or_else(|| {
            Error::Invalid("staged asset has no candidate material binding".into())
        })?;
        if staged.payload.package_kind != "skill" {
            return Err(Error::Invalid(
                "candidate handoff only supports skill packages".into(),
            ));
        }
        let bytes = store
            .read_blob(
                ctx,
                &staged.payload.content_blob_digest,
                MAX_PERSISTENT_PROJECTION,
            )
            .await?;
        let projection = decode_package_projection(&bytes)?;
        let material_files = projection
            .files
            .iter()
            .map(|(path, content)| (path.clone(), content.as_bytes().to_vec()))
            .collect::<BTreeMap<_, _>>();
        let (staged_skill, staged_strategy) =
            validate_candidate_material(&projection.manifest, &material_files, binding)?;

        let mut session = store.session().await?;
        let mut current_staged: E16Envelope<StagedAssetPayload> =
            session.need(ctx, "artifact", staged_asset_id).await?;
        if fingerprint(&current_staged)? != fingerprint(&staged)?
            || current_staged.schema_version != E16_STAGED_ASSET_SCHEMA
            || current_staged.namespace != ctx.namespace()
            || current_staged.owner_actor != ctx.actor()
            || current_staged.payload.state != StagedAssetState::Staged
        {
            return Err(Error::Forbidden);
        }
        let watermark = current_watermark(&mut session, ctx).await?;
        if current_staged.revoke_watermark != watermark {
            return Err(Error::Conflict(
                "staged asset watermark changed before candidate handoff".into(),
            ));
        }
        verify_sources(&mut session, ctx, &current_staged.source_refs).await?;

        let candidate: ReleaseCandidateRecord = session.need(ctx, "artifact", candidate_id).await?;
        if candidate.schema_version != RELEASE_CANDIDATE_SCHEMA
            || candidate.id != candidate_id
            || candidate.environment_digest != current_staged.payload.environment_digest
            || candidate.revoke_watermark != watermark
            || candidate.bundle_digest != candidate.bundle.digest
        {
            return Err(Error::Conflict(
                "release candidate does not match staged asset control plane".into(),
            ));
        }
        validate_resolved_bundle_identity(&candidate.bundle)?;
        if fingerprint(&candidate.bundle.skill)? != fingerprint(&staged_skill)?
            || fingerprint(&candidate.bundle.improver)? != fingerprint(&staged_strategy)?
        {
            return Err(Error::Conflict(
                "release candidate effective content differs from staged material".into(),
            ));
        }
        let dependency_source_ids = projection
            .manifest
            .dependencies
            .iter()
            .map(package_dependency_ref_id)
            .collect::<Result<BTreeSet<_>>>()?;
        let staged_material_sources = current_staged
            .source_refs
            .iter()
            .filter(|source| {
                !(source.kind == "artifact" && dependency_source_ids.contains(&source.id))
            })
            .collect::<Vec<_>>();
        if staged_material_sources
            .iter()
            .any(|source| source.kind != "run")
        {
            return Err(Error::Conflict(
                "staged material sources are not E06 release sources".into(),
            ));
        }
        let staged_source_ids = staged_material_sources
            .iter()
            .map(|source| (source.kind.as_str(), source.id.as_str()))
            .collect::<BTreeSet<_>>();
        let candidate_source_ids_set = candidate
            .sources
            .iter()
            .map(|source| (source.kind.as_str(), source.id.as_str()))
            .collect::<BTreeSet<_>>();
        if candidate.sources.is_empty() || candidate_source_ids_set != staged_source_ids {
            return Err(Error::Conflict(
                "candidate source closure differs from staged material sources".into(),
            ));
        }
        let candidate_source_ids = candidate
            .sources
            .iter()
            .map(|source| source.id.clone())
            .collect::<Vec<_>>();
        validate_stored_sources(&mut session, ctx, &candidate_source_ids, watermark).await?;
        for source in &candidate.sources {
            let authority = load_stored_source(&mut session, ctx, &source.id).await?;
            if authority.trace.source_digest != source.content_digest {
                return Err(Error::Conflict(
                    "release candidate source digest changed".into(),
                ));
            }
        }

        match current_staged.payload.candidate_ref.as_deref() {
            Some(existing) if existing == candidate_id => {
                session.commit().await?;
                return Ok(current_staged);
            }
            Some(_) => {
                return Err(Error::Conflict(
                    "staged asset already handed to a different candidate".into(),
                ));
            }
            None => {}
        }
        current_staged.payload.candidate_ref = Some(candidate_id.into());
        current_staged.updated_at = now();
        put_envelope(&mut session, ctx, &current_staged).await?;
        session
            .put_edge(
                ctx,
                "artifact",
                candidate_id,
                "artifact",
                &current_staged.id,
            )
            .await?;
        session
            .audit(
                ctx,
                "e16.staged_asset.candidate_handoff",
                &current_staged.id,
            )
            .await?;
        session.commit().await?;
        Ok(current_staged)
    }

    pub async fn prepare_export(
        ctx: &Context,
        store: &Store,
        request_key: &str,
        staged_asset_id: &str,
        destination_scope: &str,
        budget_ref: Option<E04BudgetRef>,
    ) -> Result<E16Envelope<ExportAttemptPayload>> {
        ctx.require(&[Role::Admin])?;
        identifier(request_key)?;
        if destination_scope != LOCAL_EXPORT_DESTINATION {
            return Err(Error::Invalid(
                "only local_namespace export delivery is supported".into(),
            ));
        }
        let staged = Self::read_staged(ctx, store, staged_asset_id).await?;
        if staged.owner_actor != ctx.actor() || staged.payload.state != StagedAssetState::Staged {
            return Err(Error::Forbidden);
        }
        if staged.payload.package_schema_version != PACKAGE_SCHEMA_V1 {
            return Err(Error::Invalid(
                "only validated package projections can be exported".into(),
            ));
        }
        if let Some(reference) = &budget_ref {
            identifier(&reference.root_budget_id)?;
            identifier(&reference.billing_scope)?;
            let root = store
                .root_budget(ctx, &reference.billing_scope)
                .await?
                .ok_or(Error::NotFound)?;
            if root.root_budget_id != reference.root_budget_id
                || root.authorizing_namespace != ctx.namespace()
            {
                return Err(Error::Conflict("E04 root budget reference differs".into()));
            }
        }
        let id = e16_storage_id("export-attempt", ctx.namespace(), ctx.actor(), request_key)?;
        let projection_bytes = store
            .read_blob(
                ctx,
                &staged.payload.content_blob_digest,
                MAX_PERSISTENT_PROJECTION,
            )
            .await?;
        let projection = decode_package_projection(&projection_bytes)?;
        let export_files = local_export_files(&projection)?;
        let (package_tree_digest, delivered_bytes) = local_export_tree_digest(&export_files)?;
        let local_export_ref = format!("exports/{}/{}", hash(ctx.namespace().as_bytes()), id);
        let input_digest = fingerprint(&(
            staged_asset_id,
            staged.input_digest.as_str(),
            destination_scope,
            package_tree_digest.as_str(),
            delivered_bytes,
            &budget_ref,
        ))?;
        let mut session = store.session().await?;
        if let Some(existing) = session
            .get::<serde_json::Value>(ctx, "artifact", &id)
            .await?
        {
            if existing
                .get("schema_version")
                .and_then(|value| value.as_str())
                != Some(E16_EXPORT_ATTEMPT_SCHEMA)
                || existing
                    .get("input_digest")
                    .and_then(|value| value.as_str())
                    != Some(input_digest.as_str())
            {
                return Err(Error::Conflict("export request key changed".into()));
            }
            let existing = serde_json::from_value(existing)
                .map_err(|_| Error::Invalid("invalid export attempt v2".into()))?;
            session.commit().await?;
            return Ok(existing);
        }
        let watermark = current_watermark(&mut session, ctx).await?;
        if watermark != staged.revoke_watermark {
            return Err(Error::Conflict("staged asset watermark is stale".into()));
        }
        verify_sources(&mut session, ctx, &staged.source_refs).await?;
        let timestamp = now();
        let envelope = E16Envelope {
            schema_version: E16_EXPORT_ATTEMPT_SCHEMA.into(),
            id,
            namespace: ctx.namespace().into(),
            owner_actor: ctx.actor().into(),
            request_key: request_key.into(),
            input_digest,
            created_at: timestamp,
            updated_at: timestamp,
            source_refs: staged.source_refs.clone(),
            revoke_watermark: watermark,
            payload: ExportAttemptPayload {
                publisher: staged.payload.publisher.clone(),
                asset_id: staged.payload.asset_id.clone(),
                staged_asset_id: staged.id.clone(),
                manifest_digest: staged.payload.manifest_digest.clone(),
                projection_blob_digest: staged.payload.content_blob_digest,
                projection_blob_bytes: staged.payload.content_bytes,
                package_tree_digest,
                delivered_bytes,
                delivery_kind: LOCAL_EXPORT_KIND.into(),
                local_export_ref,
                state: ExportAttemptState::Prepared,
                destination_scope: destination_scope.into(),
                delivery_audit_id: None,
                completion_receipt_digest: None,
                budget_ref,
                error: None,
            },
        };
        put_envelope(&mut session, ctx, &envelope).await?;
        session
            .put_edge(ctx, "artifact", &envelope.id, "artifact", &staged.id)
            .await?;
        session
            .audit(ctx, "e16.export.prepare", &envelope.id)
            .await?;
        session.commit().await?;
        Ok(envelope)
    }

    pub async fn complete_export(
        ctx: &Context,
        store: &Store,
        export_id: &str,
    ) -> Result<(
        E16Envelope<ExportAttemptPayload>,
        E16Envelope<DeliveryAuditPayload>,
        LocalExportReceipt,
    )> {
        ctx.require(&[Role::Admin])?;
        let mut session = store.session().await?;
        let raw: serde_json::Value = session.need(ctx, "artifact", export_id).await?;
        if raw.get("schema_version").and_then(|value| value.as_str())
            == Some("rsia.e16.export_attempt.v1")
        {
            return Err(Error::Invalid(
                "legacy export attempt has no verified local delivery".into(),
            ));
        }
        let attempt: E16Envelope<ExportAttemptPayload> = serde_json::from_value(raw)
            .map_err(|_| Error::Invalid("invalid export attempt v2".into()))?;
        if attempt.owner_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        session.commit().await?;
        let staged = Self::read_staged(ctx, store, &attempt.payload.staged_asset_id).await?;
        if staged.payload.state != StagedAssetState::Staged {
            return Err(Error::Conflict(
                "staged asset is no longer eligible for export".into(),
            ));
        }
        let projection_bytes = store
            .read_blob(
                ctx,
                &attempt.payload.projection_blob_digest,
                MAX_PERSISTENT_PROJECTION,
            )
            .await?;
        if hash(&projection_bytes) != attempt.payload.projection_blob_digest
            || projection_bytes.len() as u64 != attempt.payload.projection_blob_bytes
            || attempt.payload.projection_blob_digest != staged.payload.content_blob_digest
        {
            return Err(Error::Conflict("final export projection changed".into()));
        }
        let projection = decode_package_projection(&projection_bytes)?;
        let export_files = local_export_files(&projection)?;
        let (tree_digest, delivered_bytes) = local_export_tree_digest(&export_files)?;
        if tree_digest != attempt.payload.package_tree_digest
            || delivered_bytes != attempt.payload.delivered_bytes
        {
            return Err(Error::Conflict(
                "local export tree differs from Prepared".into(),
            ));
        }
        let receipt =
            local_export_receipt(ctx.namespace(), export_id, &tree_digest, delivered_bytes)?;
        if attempt.payload.state == ExportAttemptState::Completed {
            let audit_id = attempt
                .payload
                .delivery_audit_id
                .as_deref()
                .ok_or(Error::Internal)?;
            let mut session = store.session().await?;
            let audit: E16Envelope<DeliveryAuditPayload> =
                session.need(ctx, "artifact", audit_id).await?;
            session.commit().await?;
            let verified = store
                .verify_registered_local_export(ctx, export_id, &export_files)
                .await?;
            if verified != receipt
                || attempt.payload.completion_receipt_digest.as_deref()
                    != Some(receipt.completion_receipt_digest.as_str())
                || audit.payload.completion_receipt_digest != receipt.completion_receipt_digest
            {
                return Err(Error::Conflict(
                    "completed local export receipt differs".into(),
                ));
            }
            return Ok((attempt, audit, receipt));
        }
        if attempt.payload.state != ExportAttemptState::Prepared {
            return Err(Error::Conflict("export attempt is not prepared".into()));
        }
        let dependency_plan = store
            .snapshot_local_export_dependencies(ctx, export_id)
            .await?;
        let dependency_snapshot = store
            .prevalidate_local_export_dependency_blobs(ctx, dependency_plan)
            .await?;
        let mut session = store.session().await?;
        let mut current: E16Envelope<ExportAttemptPayload> =
            session.need(ctx, "artifact", export_id).await?;
        let current_watermark = current_watermark(&mut session, ctx).await?;
        if current.payload.state != ExportAttemptState::Prepared
            || current.input_digest != attempt.input_digest
            || current_watermark != current.revoke_watermark
        {
            return Err(Error::Conflict(
                "export state or watermark changed before completion".into(),
            ));
        }
        verify_sources(&mut session, ctx, &current.source_refs).await?;
        let audit_id = format!("e16-delivery-{}", fingerprint(&current.id)?);
        current.payload.state = ExportAttemptState::Completed;
        current.payload.delivery_audit_id = Some(audit_id.clone());
        current.payload.completion_receipt_digest = Some(receipt.completion_receipt_digest.clone());
        current.updated_at = now();
        let audit = E16Envelope {
            schema_version: E16_DELIVERY_AUDIT_SCHEMA.into(),
            id: audit_id.clone(),
            namespace: ctx.namespace().into(),
            owner_actor: ctx.actor().into(),
            request_key: current.request_key.clone(),
            input_digest: fingerprint(&(
                current.id.as_str(),
                current.payload.package_tree_digest.as_str(),
                current.payload.destination_scope.as_str(),
                receipt.completion_receipt_digest.as_str(),
            ))?,
            created_at: now(),
            updated_at: now(),
            source_refs: current.source_refs.clone(),
            revoke_watermark: current_watermark,
            payload: DeliveryAuditPayload {
                export_attempt_id: current.id.clone(),
                package_id: format!("{}:{}", current.payload.publisher, current.payload.asset_id),
                projection_blob_digest: current.payload.projection_blob_digest.clone(),
                delivery_kind: receipt.delivery_kind.clone(),
                local_export_ref: receipt.local_export_ref.clone(),
                package_tree_digest: receipt.package_tree_digest.clone(),
                delivered_bytes: receipt.delivered_bytes,
                completion_receipt_digest: receipt.completion_receipt_digest.clone(),
                delivered_to: current.payload.destination_scope.clone(),
                delivered_at: now(),
                watermark_at_delivery: current_watermark,
                revoked: false,
                revocation_notice_sent: false,
                remote_erasure_disclaimer: REMOTE_ERASURE_DISCLAIMER.into(),
            },
        };
        session.commit().await?;
        let current_value = serde_json::to_value(&current).map_err(|_| Error::Internal)?;
        let audit_value = serde_json::to_value(&audit).map_err(|_| Error::Internal)?;
        match store
            .publish_registered_local_export(
                ctx,
                export_id,
                &export_files,
                &current_value,
                &audit_value,
                &dependency_snapshot,
            )
            .await
        {
            Ok(actual) if actual == receipt => Ok((current, audit, actual)),
            Ok(_) => Err(Error::Conflict("local export receipt differs".into())),
            Err(error) => {
                let mut session = store.session().await?;
                let raced: E16Envelope<ExportAttemptPayload> =
                    session.need(ctx, "artifact", export_id).await?;
                if raced.payload.state != ExportAttemptState::Completed {
                    return Err(error);
                }
                let audit_id = raced
                    .payload
                    .delivery_audit_id
                    .as_deref()
                    .ok_or(Error::Internal)?;
                let raced_audit: E16Envelope<DeliveryAuditPayload> =
                    session.need(ctx, "artifact", audit_id).await?;
                session.commit().await?;
                let actual = store
                    .verify_registered_local_export(ctx, export_id, &export_files)
                    .await?;
                Ok((raced, raced_audit, actual))
            }
        }
    }

    pub async fn abort_export(
        ctx: &Context,
        store: &Store,
        export_id: &str,
    ) -> Result<E16Envelope<ExportAttemptPayload>> {
        ctx.require(&[Role::Admin])?;
        let mut session = store.session().await?;
        let raw: serde_json::Value = session.need(ctx, "artifact", export_id).await?;
        if raw.get("schema_version").and_then(|value| value.as_str())
            == Some("rsia.e16.export_attempt.v1")
        {
            return Err(Error::Invalid(
                "legacy export attempt has no abortable local delivery intent".into(),
            ));
        }
        let mut attempt: E16Envelope<ExportAttemptPayload> = serde_json::from_value(raw)
            .map_err(|_| Error::Invalid("invalid export attempt v2".into()))?;
        if attempt.owner_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        if attempt.payload.state == ExportAttemptState::Aborted {
            session.commit().await?;
            return Ok(attempt);
        }
        if attempt.payload.state != ExportAttemptState::Prepared {
            return Err(Error::Conflict("completed export is immutable".into()));
        }
        attempt.payload.state = ExportAttemptState::Aborted;
        attempt.updated_at = now();
        session.commit().await?;
        let value = serde_json::to_value(&attempt).map_err(|_| Error::Internal)?;
        store
            .abort_registered_local_export(ctx, export_id, &value)
            .await?;
        Ok(attempt)
    }

    pub async fn can_safely_uninstall_persistent(
        ctx: &Context,
        store: &Store,
        dependency_id: &str,
        requesting_package_id: &str,
    ) -> Result<bool> {
        ctx.require(&[Role::Admin])?;
        identifier(dependency_id)?;
        identifier(requesting_package_id)?;
        let mut session = store.session().await?;
        let dependents = session.dependents(ctx, "artifact", dependency_id).await?;
        let others: Vec<_> = dependents
            .into_iter()
            .filter(|(kind, id)| kind == "artifact" && id != requesting_package_id)
            .collect();
        session.commit().await?;
        if !others.is_empty() {
            return Err(Error::Conflict(format!(
                "shared dependency remains referenced by: {others:?}"
            )));
        }
        Ok(true)
    }
}

pub fn package_dependency_ref_id(dependency: &PackageDependency) -> Result<String> {
    identifier(&dependency.publisher)?;
    identifier(&dependency.asset_id)?;
    identifier(&dependency.kind)?;
    Ok(format!(
        "e16-package-dependency-{}",
        fingerprint(&(
            dependency.publisher.as_str(),
            dependency.asset_id.as_str(),
            dependency.kind.as_str(),
        ))?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_and_exec_rejected() {
        assert!(
            reject_member(&PackageMember {
                path: "../x".into(),
                size: 1,
                compressed: 1
            })
            .is_err()
        );
        assert!(
            reject_member(&PackageMember {
                path: "/etc/passwd".into(),
                size: 1,
                compressed: 1
            })
            .is_err()
        );
        assert!(
            reject_member(&PackageMember {
                path: "run.sh".into(),
                size: 1,
                compressed: 1
            })
            .is_err()
        );
        assert!(
            reject_member(&PackageMember {
                path: "skill.md".into(),
                size: 1,
                compressed: 1
            })
            .is_ok()
        );
    }

    #[test]
    fn privacy_and_foreign_approval() {
        assert!(privacy_block("hello").is_ok());
        assert!(privacy_block("BEGIN PRIVATE KEY").is_err());
        assert!(foreign_approval_is_not_local("formal_eval_from_elsewhere"));
    }

    #[test]
    fn zip_bomb_ratio_rejected() {
        assert!(
            reject_member(&PackageMember {
                path: "data.txt".into(),
                size: 2000,
                compressed: 10, // ratio 200 > 100
            })
            .is_err()
        );
    }

    #[test]
    fn non_normalized_paths_rejected() {
        assert!(
            reject_member(&PackageMember {
                path: "a//b.txt".into(),
                size: 10,
                compressed: 10,
            })
            .is_err()
        );
        assert!(
            reject_member(&PackageMember {
                path: "./a/b.txt".into(),
                size: 10,
                compressed: 10,
            })
            .is_err()
        );
    }
}
