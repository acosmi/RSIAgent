//! Comprehensive validation test suite for E16.2: Asset Package Safety, Privacy Gates,
//! Foreign Metadata Isolation, and Revocation-Aware Export.
//!
//! Covers scenario families:
//! - V017: Revocation-Watermark-Monotonic
//! - V039: Zero-Regression-Holdout / Dependency Gating
//! - V066: Package-Safety-Limits (paths, executables, size, bombs)
//! - V067: Privacy-Scan-Gates (keys, tokens, answers, internal IPs, licenses)
//! - V068: Foreign-Metadata-Untrusted (no local approval/active status, quarantine)
//! - V069: Revocation-During-Export-Or-Restore
//! - V070: Diff-Preview-Review (exact diff, requires new approval, no escalation)
//! - V075: Revocation-Content-Closure & Shared Dependency Protection
//! - V091: All-Effective-Content-Gated
//! - V092: No-Auto-Elevation
//! - V098: Audit-Traceability & Golden Manifest Roundtrip

use evo_core::{Error, hash};
use evo_engine::packages::{
    AssetPackageManifest, DeliveryAuditRecord, DependencyTracker, ExportRequest, FindingSeverity,
    ForeignApprovalClaim, ForeignEvaluationClaim, ForeignMetadata, MAX_FILE, MAX_FILES, MAX_TOTAL,
    MAX_ZIP_RATIO, PACKAGE_SCHEMA_V1, PRIVACY_DISCLAIMER, PackageDependency, PackageEntry,
    PackageKind, PackageMember, PackageState, RedactionReport, RedactionStatus, export_package,
    foreign_approval_is_not_local, import_external_package, mark_delivered_package_revoked,
    preview_package_diff, privacy_block, reject_member, reject_package, scan_privacy,
    validate_manifest,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn valid_base_manifest() -> AssetPackageManifest {
    let content1 = b"# Test Skill\nInstructions here\n";
    let content2 = b"{\"param\": 123}\n";
    AssetPackageManifest {
        schema_version: PACKAGE_SCHEMA_V1.into(),
        kind: PackageKind::Skill,
        publisher: "org.rsia".into(),
        asset_id: "test_asset".into(),
        name: "Test Asset".into(),
        version: "1.0.0".into(),
        description: "A test package for verification".into(),
        license: "Apache-2.0".into(),
        entries: vec![
            PackageEntry {
                path: "skill.md".into(),
                digest_sha256: hash(content1),
                size_bytes: content1.len() as u64,
                compressed_bytes: content1.len() as u64,
            },
            PackageEntry {
                path: "config.json".into(),
                digest_sha256: hash(content2),
                size_bytes: content2.len() as u64,
                compressed_bytes: content2.len() as u64,
            },
        ],
        dependencies: vec![],
        redaction_report: RedactionReport {
            scanned_at: 1000,
            findings_count: 0,
            status: RedactionStatus::Clean,
            disclaimer: PRIVACY_DISCLAIMER.into(),
            findings: vec![],
        },
        foreign_metadata: None,
    }
}

// ---------------------------------------------------------------------------
// V066: Package-Safety-Limits (paths, scripts, bombs, sizes)
// ---------------------------------------------------------------------------
#[test]
fn test_v066_path_traversal_and_absolute_paths_rejected() {
    let bad_paths = [
        "../etc/passwd",
        "foo/../../bar",
        "/absolute/path.txt",
        "\\windows\\path.txt",
        "C:\\Users\\admin\\secret.txt",
        "foo//bar.txt",
        "./relative.txt",
        "path/with\0null.txt",
        "",
        "   ",
    ];

    for path in bad_paths {
        let m = PackageMember {
            path: path.into(),
            size: 100,
            compressed: 100,
        };
        assert!(
            reject_member(&m).is_err(),
            "Path '{}' should have been rejected",
            path
        );
    }
}

#[test]
fn test_v066_executables_and_scripts_rejected() {
    let script_paths = [
        "install.sh",
        "payload.exe",
        "libnative.so",
        "plugin.dylib",
        "driver.dll",
        "binary.bin",
        "exploit.py",
        "setup.bat",
        "run.cmd",
        "deploy.ps1",
        "script.vbs",
        "module.wasm",
        "program.com",
        "foo/symlink_target",
        "bar/hardlink_ref",
    ];

    for path in script_paths {
        let m = PackageMember {
            path: path.into(),
            size: 100,
            compressed: 100,
        };
        assert!(
            reject_member(&m).is_err(),
            "Script/executable member '{}' should have been rejected",
            path
        );
    }
}

#[test]
fn test_v066_size_limits_and_zip_bomb_ratios() {
    // Single file exceeds MAX_FILE (1 MiB)
    let oversized_file = PackageMember {
        path: "huge.json".into(),
        size: MAX_FILE + 1,
        compressed: 500,
    };
    assert!(reject_member(&oversized_file).is_err());

    // Zip bomb ratio exceeds MAX_ZIP_RATIO (100)
    assert_eq!(MAX_ZIP_RATIO, 100);
    let bomb_file = PackageMember {
        path: "bomb.txt".into(),
        size: 500_000,
        compressed: 1000, // ratio 500 > 100
    };
    assert!(reject_member(&bomb_file).is_err());

    // Package exceeding MAX_FILES (100)
    let too_many_members: Vec<PackageMember> = (0..=MAX_FILES)
        .map(|i| PackageMember {
            path: format!("file_{i}.txt"),
            size: 10,
            compressed: 10,
        })
        .collect();
    assert!(reject_package(&too_many_members).is_err());

    // Package exceeding MAX_TOTAL (10 MiB)
    let total_oversized = vec![
        PackageMember {
            path: "f1.txt".into(),
            size: MAX_FILE,
            compressed: MAX_FILE,
        },
        PackageMember {
            path: "f2.txt".into(),
            size: MAX_FILE,
            compressed: MAX_FILE,
        },
        PackageMember {
            path: "f3.txt".into(),
            size: MAX_FILE,
            compressed: MAX_FILE,
        },
        PackageMember {
            path: "f4.txt".into(),
            size: MAX_FILE,
            compressed: MAX_FILE,
        },
        PackageMember {
            path: "f5.txt".into(),
            size: MAX_FILE,
            compressed: MAX_FILE,
        },
        PackageMember {
            path: "f6.txt".into(),
            size: MAX_FILE,
            compressed: MAX_FILE,
        },
        PackageMember {
            path: "f7.txt".into(),
            size: MAX_FILE,
            compressed: MAX_FILE,
        },
        PackageMember {
            path: "f8.txt".into(),
            size: MAX_FILE,
            compressed: MAX_FILE,
        },
        PackageMember {
            path: "f9.txt".into(),
            size: MAX_FILE,
            compressed: MAX_FILE,
        },
        PackageMember {
            path: "f10.txt".into(),
            size: MAX_FILE,
            compressed: MAX_FILE,
        },
        PackageMember {
            path: "f11.txt".into(),
            size: 1,
            compressed: 1,
        },
    ];
    let total_size: u64 = total_oversized.iter().map(|m| m.size).sum();
    assert!(total_size > MAX_TOTAL);
    assert!(reject_package(&total_oversized).is_err());
}

// ---------------------------------------------------------------------------
// V067: Privacy-Scan-Gates (keys, tokens, answers, internal IPs, licenses)
// ---------------------------------------------------------------------------
#[test]
fn test_v067_privacy_blocks_sensitive_patterns() {
    let sensitive_inputs = [
        "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA...",
        "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk...",
        "openai_api_key = \"sk-proj-1234567890abcdef\"",
        "github_token = \"ghp_secretTokenVal1234567890\"",
        "slack_token = \"xoxb-1234-5678-abcdef\"",
        "google_key = \"AIzaSyB1234567890abcdef\"",
        "internal_path = \"/Users/alice/projects/rsi/data.json\"",
        "home_path = \"/home/bob/.ssh/id_rsa\"",
        "windows_user = \"C:\\Users\\admin\\Desktop\\data.csv\"",
        "ANSWER: The solution to the holdout question is 42",
        "GROUND_TRUTH: expected value is positive",
        "HIDDEN_EVAL: benchmark verification secret",
        "oracle_eval: score is computed as ...",
        "backend_url = \"http://192.168.1.100:8080/api\"",
        "db_host = \"mysql://10.0.1.5:3306/prod\"",
        "corp_host = \"http://service.corp/api\"",
        "cluster_host = \"http://worker.internal:9000\"",
        "local_host = \"http://device.local:80\"",
        "{\"role\": \"user\", \"content\": \"confidential chat prompt\"}",
        "chat_history = [{\"message\": \"hi\"}]",
        "database_conn = \"user=admin password=supersecret host=localhost\"",
        "aws_secret_access_key = \"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\"",
    ];

    for input in sensitive_inputs {
        let report = scan_privacy(input);
        assert_eq!(
            report.status,
            RedactionStatus::Blocked,
            "Input '{}' should have been blocked",
            input
        );
        assert!(
            privacy_block(input).is_err(),
            "privacy_block should return Err for '{}'",
            input
        );
    }
}

#[test]
fn test_v067_privacy_clean_content_and_warnings() {
    let clean_text = "# Skill Definition\nApply step by step reasoning.\n";
    let report = scan_privacy(clean_text);
    assert_eq!(report.status, RedactionStatus::Clean);
    assert_eq!(report.findings_count, 0);
    assert!(privacy_block(clean_text).is_ok());

    let warning_text = "# Draft Note\nTODO: secret token management should be reviewed\n";
    let warn_report = scan_privacy(warning_text);
    assert_eq!(warn_report.status, RedactionStatus::Warning);
    assert_eq!(warn_report.findings[0].severity, FindingSeverity::Warning);
    // Warnings do not block
    assert!(privacy_block(warning_text).is_ok());
}

#[test]
fn test_v067_unexportable_licenses_blocked() {
    let mut manifest = valid_base_manifest();

    let forbidden_licenses = [
        "",
        "unknown",
        "UNKNOWN",
        "unlicensed",
        "proprietary-internal",
        "all-rights-reserved;no-export",
    ];

    for lic in forbidden_licenses {
        manifest.license = lic.into();
        assert!(
            validate_manifest(&manifest).is_err(),
            "License '{}' should have been rejected",
            lic
        );
    }

    manifest.license = "MIT".into();
    assert!(validate_manifest(&manifest).is_ok());
}

// ---------------------------------------------------------------------------
// V068 & V092: Foreign-Metadata-Untrusted & No-Auto-Elevation
// ---------------------------------------------------------------------------
#[test]
fn test_v068_foreign_approval_and_evaluation_never_grants_local_permissions() {
    let mut manifest = valid_base_manifest();
    manifest.foreign_metadata = Some(ForeignMetadata {
        external_namespace: Some("untrusted_remote_corp".into()),
        external_parent_id: Some("remote_parent_candidate".into()),
        formal_evaluations: vec![ForeignEvaluationClaim {
            evaluator_identity: "untrusted_foreign_evaluator".into(),
            evaluated_at: 12345,
            score: "0.999".into(),
            notes: "Claiming 99% accuracy on remote private dataset".into(),
        }],
        approvals: vec![ForeignApprovalClaim {
            approver_identity: "foreign_admin_root".into(),
            approved_at: 12346,
            notes: "Approved by foreign admin".into(),
        }],
        disclaimer: "Foreign claims are untrusted.".into(),
    });

    let mut files = BTreeMap::new();
    files.insert(
        "skill.md".into(),
        b"# Test Skill\nInstructions here\n".to_vec(),
    );
    files.insert("config.json".into(), b"{\"param\": 123}\n".to_vec());

    // Import package with foreign approval metadata
    let staged = import_external_package(
        &manifest,
        &files,
        |_pub, _id| true,  // dependencies available
        |_pub, _id| false, // dependencies not revoked
    )
    .expect("Import should succeed into staged state");

    // V068 & V092 assertions:
    assert_eq!(staged.state, PackageState::Staged);
    assert!(!staged.is_active, "Imported package must NEVER be active!");
    assert!(
        !staged.is_approved,
        "Foreign approval must NEVER grant local approval!"
    );
    assert_eq!(staged.local_formal_evaluation_id, None);
    assert_eq!(staged.local_approval_id, None);
    assert!(foreign_approval_is_not_local("foreign_admin_root"));

    // Foreign claims are strictly quarantined in untrusted metadata
    let foreign = staged.untrusted_foreign_metadata.unwrap();
    assert_eq!(foreign.formal_evaluations.len(), 1);
    assert_eq!(foreign.approvals.len(), 1);
}

#[test]
fn test_v068_missing_dependencies_cause_quarantine() {
    let mut manifest = valid_base_manifest();
    manifest.dependencies.push(PackageDependency {
        publisher: "core.rsia".into(),
        asset_id: "missing_dep".into(),
        kind: "template".into(),
        version_req: ">=1.0.0".into(),
    });

    let mut files = BTreeMap::new();
    files.insert(
        "skill.md".into(),
        b"# Test Skill\nInstructions here\n".to_vec(),
    );
    files.insert("config.json".into(), b"{\"param\": 123}\n".to_vec());

    let staged = import_external_package(
        &manifest,
        &files,
        |_pub, id| id != "missing_dep", // dep not available
        |_pub, _id| false,
    )
    .expect("Import succeeds into quarantined state");

    assert_eq!(staged.state, PackageState::Quarantined);
    assert!(
        staged
            .quarantine_reason
            .as_ref()
            .unwrap()
            .contains("unresolved_dependency")
    );
}

// ---------------------------------------------------------------------------
// V069 & V017: Revocation-During-Export-Or-Restore & Watermark Monotonicity
// ---------------------------------------------------------------------------
#[test]
fn test_v069_source_revoked_during_export_aborts_package_creation() {
    let mut files = BTreeMap::new();
    files.insert("skill.md".into(), b"# Skill content\nStep 1.\n".to_vec());

    let req = ExportRequest {
        publisher: "org.rsia".into(),
        asset_id: "skill_a".into(),
        name: "Skill A".into(),
        version: "1.0.0".into(),
        description: "Export test".into(),
        kind: PackageKind::Skill,
        license: "MIT".into(),
        files,
        dependencies: vec![],
        referenced_source_ids: vec!["src_run_01".into(), "src_run_02".into()],
        export_started_watermark: 10,
    };

    // Case 1: Current watermark advanced during export (e.g. from 10 to 11)
    let res_watermark = export_package(&req, |_id| false, 11);
    assert!(
        matches!(res_watermark, Err(Error::Conflict(msg)) if msg.contains("watermark advanced"))
    );

    // Case 2: One of the referenced sources was revoked
    let res_source_revoked = export_package(
        &req,
        |id| id == "src_run_02", // src_run_02 revoked
        10,                      // watermark unchanged
    );
    assert!(matches!(res_source_revoked, Err(Error::Conflict(msg)) if msg.contains("is revoked")));

    // Case 3: No revocation -> Export succeeds
    let exported = export_package(&req, |_id| false, 10).expect("Export should succeed");
    assert_eq!(exported.manifest.entries.len(), 1);
    assert!(!exported.delivery_record.revoked);
}

#[test]
fn test_v069_delivered_package_revocation_disclaimer() {
    let mut record = DeliveryAuditRecord {
        package_id: "org.rsia:asset_1".into(),
        publisher: "org.rsia".into(),
        delivered_to: "external_partner".into(),
        delivered_at: 1000,
        watermark_at_delivery: 5,
        revoked: false,
        revocation_notice_sent: false,
        remote_erasure_disclaimer: "Initial disclaimer".into(),
    };

    mark_delivered_package_revoked(&mut record);
    assert!(record.revoked);
    assert!(record.revocation_notice_sent);
    assert!(!record.remote_erasure_disclaimer.is_empty());
}

// ---------------------------------------------------------------------------
// V070 & V091: Diff-Preview-Review & All-Effective-Content-Gated
// ---------------------------------------------------------------------------
#[test]
fn test_v070_diff_preview_identifies_exact_changes_and_mandates_approval() {
    let mut local = BTreeMap::new();
    local.insert("skill.md".into(), "Line 1\nLine 2\n".into());
    local.insert("delete_me.txt".into(), "Old file\n".into());

    let mut incoming = BTreeMap::new();
    incoming.insert(
        "skill.md".into(),
        "Line 1\nLine 2 modified\nLine 3\n".into(),
    );
    incoming.insert("new_file.txt".into(), "Brand new file\n".into());

    let preview = preview_package_diff("my_asset", "skill", &local, &incoming);

    assert_eq!(preview.asset_id, "my_asset");
    assert_eq!(preview.kind, "skill");
    assert!(!preview.permission_escalated, "Cannot escalate permissions");
    assert!(
        preview.requires_new_approval,
        "Content changes must require new approval!"
    );

    let changed = &preview.changed_files;
    let skill_diff = changed.iter().find(|d| d.path == "skill.md").unwrap();
    assert_eq!(skill_diff.change_type, "modified");

    let deleted_diff = changed.iter().find(|d| d.path == "delete_me.txt").unwrap();
    assert_eq!(deleted_diff.change_type, "deleted");

    let added_diff = changed.iter().find(|d| d.path == "new_file.txt").unwrap();
    assert_eq!(added_diff.change_type, "added");

    // Identical content case: requires_new_approval is false
    let identical_preview = preview_package_diff("my_asset", "skill", &local, &local);
    assert!(!identical_preview.requires_new_approval);
}

// ---------------------------------------------------------------------------
// V075: Revocation-Content-Closure & Shared Dependency Protection
// ---------------------------------------------------------------------------
#[test]
fn test_v075_revocation_dependency_causes_quarantine() {
    let mut manifest = valid_base_manifest();
    manifest.dependencies.push(PackageDependency {
        publisher: "core.rsia".into(),
        asset_id: "revoked_dep".into(),
        kind: "template".into(),
        version_req: ">=1.0.0".into(),
    });

    let mut files = BTreeMap::new();
    files.insert(
        "skill.md".into(),
        b"# Test Skill\nInstructions here\n".to_vec(),
    );
    files.insert("config.json".into(), b"{\"param\": 123}\n".to_vec());

    let staged = import_external_package(
        &manifest,
        &files,
        |_pub, _id| true,
        |_pub, id| id == "revoked_dep", // dep is revoked
    )
    .expect("Import completes with quarantined status");

    assert_eq!(staged.state, PackageState::Quarantined);
    assert!(
        staged
            .quarantine_reason
            .as_ref()
            .unwrap()
            .contains("revoked_dependency")
    );
}

#[test]
fn test_v075_shared_dependency_uninstall_protection() {
    let mut tracker = DependencyTracker::new();

    // Package A and Package B both depend on shared_lib
    tracker.register_dependency("package_A", "shared_lib");
    tracker.register_dependency("package_B", "shared_lib");

    // Attempting to uninstall shared_lib on behalf of package_A must be rejected
    // because package_B still depends on it!
    let uninstall_attempt = tracker.can_safely_uninstall("shared_lib", "package_A");
    assert!(
        matches!(uninstall_attempt, Err(Error::Conflict(msg)) if msg.contains("still required by other packages"))
    );

    // Package B unregisters / uninstalls
    tracker.unregister_package("package_B");

    // Now Package A can safely uninstall shared_lib
    let safe_uninstall = tracker.can_safely_uninstall("shared_lib", "package_A");
    assert!(safe_uninstall.is_ok());
}

// ---------------------------------------------------------------------------
// V098: Audit-Traceability & Golden Manifest Roundtrip
// ---------------------------------------------------------------------------
#[test]
fn test_v098_golden_manifest_fixture_roundtrip() {
    let fixture_path = Path::new("../../fixtures/packages/golden_manifest.json");
    let content = if fixture_path.exists() {
        fs::read_to_string(fixture_path).unwrap()
    } else {
        // Fallback for running from root
        fs::read_to_string("fixtures/packages/golden_manifest.json").unwrap()
    };

    let manifest: AssetPackageManifest =
        serde_json::from_str(&content).expect("Golden manifest must parse without error");

    assert_eq!(manifest.schema_version, PACKAGE_SCHEMA_V1);
    assert_eq!(manifest.kind, PackageKind::Skill);
    assert_eq!(manifest.publisher, "community.rsia");
    assert_eq!(manifest.asset_id, "math_reasoning_v1");
    assert_eq!(manifest.entries.len(), 2);
    assert_eq!(manifest.dependencies.len(), 1);
    assert_eq!(manifest.redaction_report.status, RedactionStatus::Clean);

    // Validate the golden manifest using engine validator
    validate_manifest(&manifest).expect("Golden manifest must satisfy validate_manifest");

    // Re-serialize and parse again to ensure byte roundtrip fidelity
    let serialized = serde_json::to_string_pretty(&manifest).unwrap();
    let reparsed: AssetPackageManifest = serde_json::from_str(&serialized).unwrap();
    assert_eq!(manifest, reparsed);
}

// =========================================================================
// F04 Adversarial Regression (from controller review)
// =========================================================================
#[test]
fn test_f04_review_export_metadata_must_pass_privacy_scan() {
    let req = ExportRequest {
        kind: PackageKind::Skill,
        publisher: "p".into(),
        asset_id: "a".into(),
        name: "safe".into(),
        version: "1".into(),
        description: "-----BEGIN PRIVATE KEY----- example-test-only".into(),
        license: "MIT".into(),
        files: BTreeMap::from([("skill.md".into(), b"safe content".to_vec())]),
        dependencies: vec![],
        referenced_source_ids: vec!["s".into()],
        export_started_watermark: 1,
    };
    assert!(
        export_package(&req, |_| false, 1).is_err(),
        "private-key marker in exported manifest description bypassed privacy scan"
    );
}

#[test]
fn test_f04_manifest_validation_blocks_sensitive_metadata() {
    let mut manifest = valid_base_manifest();
    manifest.description = "Contains secret ghp_abcdef123456789".into();
    assert!(
        validate_manifest(&manifest).is_err(),
        "secret token in manifest description bypassed validate_manifest"
    );

    let mut manifest2 = valid_base_manifest();
    manifest2.dependencies.push(PackageDependency {
        publisher: "org.rsia".into(),
        asset_id: "dep1".into(),
        kind: "skill".into(),
        version_req: "1.0-sk-1234567890abcdef".into(),
    });
    assert!(
        validate_manifest(&manifest2).is_err(),
        "secret token in dependency bypassed validate_manifest"
    );
}
