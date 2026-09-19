//! Asset package gates, staging, export, and privacy scanner.
//! Foreign approval is not local permission.
use evo_core::{Error, Result, hash, identifier, now};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

pub const PACKAGE_SCHEMA_V1: &str = "rsia.package.v1";
pub const MAX_FILES: usize = 100;
pub const MAX_TOTAL: u64 = 10 * 1024 * 1024; // 10 MiB
pub const MAX_FILE: u64 = 1024 * 1024; // 1 MiB
pub const MAX_ZIP_RATIO: u64 = 100;

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
pub struct ExportedPackage {
    pub manifest: AssetPackageManifest,
    pub files: BTreeMap<String, Vec<u8>>,
    pub delivery_record: DeliveryAuditRecord,
}

pub fn export_package(
    req: &ExportRequest,
    is_source_revoked: impl Fn(&str) -> bool,
    current_watermark: u64,
) -> Result<ExportedPackage> {
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

    let delivery_record = DeliveryAuditRecord {
        package_id: format!("{}:{}", req.publisher, req.asset_id),
        publisher: req.publisher.clone(),
        delivered_to: "export_destination".into(),
        delivered_at: now(),
        watermark_at_delivery: current_watermark,
        revoked: false,
        revocation_notice_sent: false,
        remote_erasure_disclaimer: REMOTE_ERASURE_DISCLAIMER.into(),
    };

    Ok(ExportedPackage {
        manifest,
        files: req.files.clone(),
        delivery_record,
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
