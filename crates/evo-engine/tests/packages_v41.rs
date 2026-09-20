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

use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch, SkillSnapshot,
    compile_bundle,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::packages::{
    AssetPackageManifest, CandidateMaterialBinding, CandidateStrategyAuthority,
    DeliveryAuditRecord, DependencyTracker, E16Envelope, E16SourceRef, ExportAttemptState,
    ExportRequest, FindingSeverity, ForeignApprovalClaim, ForeignEvaluationClaim, ForeignMetadata,
    MAX_FILE, MAX_FILES, MAX_TOTAL, MAX_ZIP_RATIO, PACKAGE_SCHEMA_V1, PRIVACY_DISCLAIMER,
    PackageDependency, PackageEntry, PackageKind, PackageMember, PackageState,
    PersistentPackageStore, RedactionReport, RedactionStatus, StagePackageRequest,
    StagedAssetPayload, StagedAssetState, export_package, foreign_approval_is_not_local,
    import_external_package, mark_delivered_package_revoked, package_dependency_ref_id,
    preview_package_diff, privacy_block, reject_member, reject_package, scan_privacy,
    validate_manifest,
};
use evo_engine::release_store::{ReleaseStore, StageBundleRequest, TypedSourceRef};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use std::collections::{BTreeMap, BTreeSet};
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
    assert_eq!(exported.files, req.files);
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

fn persistent_stage_request(
    request_key: &str,
    manifest: AssetPackageManifest,
    files: BTreeMap<String, Vec<u8>>,
    sources: Vec<E16SourceRef>,
) -> StagePackageRequest {
    StagePackageRequest {
        request_key: request_key.into(),
        manifest,
        files,
        source_refs: sources,
        baseline_digest: hash(b"baseline"),
        local_digest: hash(b"local"),
        upstream_digest: Some(hash(b"upstream")),
        environment_digest: hash(b"environment"),
        compiler_version: "compiler-v1".into(),
        candidate_material: None,
    }
}

fn valid_files() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            "skill.md".into(),
            b"# Test Skill\nInstructions here\n".to_vec(),
        ),
        ("config.json".into(), b"{\"param\": 123}\n".to_vec()),
    ])
}

fn manifest_for_files(asset_id: &str, files: &BTreeMap<String, Vec<u8>>) -> AssetPackageManifest {
    AssetPackageManifest {
        schema_version: PACKAGE_SCHEMA_V1.into(),
        kind: PackageKind::Skill,
        publisher: "org.rsia".into(),
        asset_id: asset_id.into(),
        name: format!("Package {asset_id}"),
        version: "1.0.0".into(),
        description: "Persistent package size boundary fixture".into(),
        license: "Apache-2.0".into(),
        entries: files
            .iter()
            .map(|(path, bytes)| PackageEntry {
                path: path.clone(),
                digest_sha256: hash(bytes),
                size_bytes: bytes.len() as u64,
                compressed_bytes: bytes.len() as u64,
            })
            .collect(),
        dependencies: vec![],
        redaction_report: RedactionReport {
            scanned_at: 1,
            findings_count: 0,
            status: RedactionStatus::Clean,
            disclaimer: PRIVACY_DISCLAIMER.into(),
            findings: vec![],
        },
        foreign_metadata: None,
    }
}

async fn persistent_store() -> (tempfile::TempDir, Store, Context, Vec<E16SourceRef>) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("packages.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    let mut sources = Vec::new();
    for id in ["package-source-1", "package-source-2"] {
        let body = serde_json::json!({"schema_version":"fixture.source.v1","id":id});
        session
            .put(&admin, "artifact", id, admin.actor(), &body)
            .await
            .unwrap();
        sources.push(E16SourceRef {
            kind: "artifact".into(),
            id: id.into(),
            digest: fingerprint(&body).unwrap(),
        });
    }
    session
        .bump_watermark(&admin, "package-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    (directory, store, admin, sources)
}

fn trusted_authority(id: &str, body: &str) -> StoredTraceAuthority {
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.as_bytes().to_vec(),
            parent_family: format!("family-{id}"),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: format!("family-{id}"),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    }
}

fn candidate_bundle(content: &str) -> evo_core::contract::ResolvedBundle {
    let profile = Profile {
        id: "package-profile".into(),
        evolution_enabled: true,
        parent_digest: hash(b"candidate-parent"),
        baseline_digest: hash(b"candidate-baseline"),
    };
    let parent = SkillSnapshot {
        content: content.into(),
        applicability: "package applicability".into(),
        counterexample: "package counterexample".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    compile_bundle(CompileParts {
        profile: &profile,
        parent: &parent,
        baseline: &SkillSnapshot::empty(),
        parent_strategy: &Strategy::default(),
        baseline_strategy: &Strategy::default(),
        skill_patch: &SkillPatch::default(),
        improver_patch: &ImproverPatch::default(),
        caps: &HostCapabilities {
            available: BTreeSet::new(),
            granted: BTreeSet::new(),
        },
        revoked: &BTreeSet::new(),
    })
    .unwrap()
}

fn candidate_material_package(
    bundle: &evo_core::contract::ResolvedBundle,
) -> (
    AssetPackageManifest,
    BTreeMap<String, Vec<u8>>,
    CandidateMaterialBinding,
) {
    let skill_path = "material/skill_snapshot.json";
    let strategy_path = "material/strategy.json";
    let skill = serde_json::to_vec(&bundle.skill).unwrap();
    let strategy = serde_json::to_vec(&bundle.improver).unwrap();
    let files = BTreeMap::from([
        (skill_path.into(), skill.clone()),
        (strategy_path.into(), strategy.clone()),
    ]);
    let manifest = AssetPackageManifest {
        schema_version: PACKAGE_SCHEMA_V1.into(),
        kind: PackageKind::Skill,
        publisher: "org.rsia".into(),
        asset_id: "candidate_material".into(),
        name: "Candidate material".into(),
        version: "1.0.0".into(),
        description: "Frozen local candidate material".into(),
        license: "Apache-2.0".into(),
        entries: vec![
            PackageEntry {
                path: skill_path.into(),
                digest_sha256: hash(&skill),
                size_bytes: skill.len() as u64,
                compressed_bytes: skill.len() as u64,
            },
            PackageEntry {
                path: strategy_path.into(),
                digest_sha256: hash(&strategy),
                size_bytes: strategy.len() as u64,
                compressed_bytes: strategy.len() as u64,
            },
        ],
        dependencies: vec![],
        redaction_report: RedactionReport {
            scanned_at: 1,
            findings_count: 0,
            status: RedactionStatus::Clean,
            disclaimer: PRIVACY_DISCLAIMER.into(),
            findings: vec![],
        },
        foreign_metadata: None,
    };
    (
        manifest,
        files,
        CandidateMaterialBinding {
            skill_snapshot_entry_path: skill_path.into(),
            strategy_entry_path: strategy_path.into(),
            strategy_authority: CandidateStrategyAuthority::BuiltinDefaultV1,
        },
    )
}

#[tokio::test]
async fn persistent_stage_restart_idempotency_permissions_and_source2_revoke() {
    let (directory, store, admin, sources) = persistent_store().await;
    let manifest = valid_base_manifest();
    let files = valid_files();
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request(
            "stage-key",
            manifest.clone(),
            files.clone(),
            sources.clone(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(staged.payload.state, StagedAssetState::Staged);
    let mut unknown_envelope = serde_json::to_value(&staged).unwrap();
    unknown_envelope
        .as_object_mut()
        .unwrap()
        .insert("unexpected".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<E16Envelope<StagedAssetPayload>>(unknown_envelope).is_err());
    let mut unknown_payload = serde_json::to_value(&staged).unwrap();
    unknown_payload["payload"]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<E16Envelope<StagedAssetPayload>>(unknown_payload).is_err());
    assert_eq!(
        PersistentPackageStore::stage_package(
            &admin,
            &store,
            persistent_stage_request(
                "stage-key",
                manifest.clone(),
                files.clone(),
                sources.clone()
            ),
        )
        .await
        .unwrap()
        .id,
        staged.id
    );
    let mut changed_manifest = manifest.clone();
    changed_manifest.description = "changed but safe".into();
    assert!(
        PersistentPackageStore::stage_package(
            &admin,
            &store,
            persistent_stage_request(
                "stage-key",
                changed_manifest,
                files.clone(),
                sources.clone(),
            ),
        )
        .await
        .is_err()
    );
    store.close().await;
    let reopened = Store::open(&directory.path().join("packages.sqlite3"))
        .await
        .unwrap();
    let worker = Context::new("n", "admin", Role::Worker).unwrap();
    PersistentPackageStore::read_staged(&worker, &reopened, &staged.id)
        .await
        .unwrap();
    let unrelated_worker = Context::new("n", "worker", Role::Worker).unwrap();
    assert!(
        PersistentPackageStore::read_staged(&unrelated_worker, &reopened, &staged.id)
            .await
            .is_err()
    );
    let other = Context::new("other", "admin", Role::Admin).unwrap();
    assert!(
        PersistentPackageStore::read_staged(&other, &reopened, &staged.id)
            .await
            .is_err()
    );
    let mut session = reopened.session().await.unwrap();
    session
        .put(
            &admin,
            "tombstone",
            "package-source-2",
            admin.actor(),
            &serde_json::json!({"id":"package-source-2"}),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&admin, "source-2-revoked")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        PersistentPackageStore::read_staged(&admin, &reopened, &staged.id)
            .await
            .is_err()
    );
    assert!(
        PersistentPackageStore::stage_package(
            &admin,
            &reopened,
            persistent_stage_request("stage-key", manifest, files, sources),
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn candidate_handoff_consumes_existing_e02_candidate_without_granting_release() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("candidate-handoff.sqlite3");
    let store = Store::open(&database_path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    let proposer = Context::new("n", "proposer", Role::Worker).unwrap();
    let first = trusted_authority("package-run-1", "first trusted package trace");
    let second = trusted_authority("package-run-2", "second trusted package trace");
    store_trace_authority(&store, &host, &first).await.unwrap();
    store_trace_authority(&store, &host, &second).await.unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, "candidate-handoff-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();

    let staged_sources = [&first, &second]
        .into_iter()
        .map(|authority| E16SourceRef {
            kind: "run".into(),
            id: authority.record.id.clone(),
            digest: fingerprint(&serde_json::to_value(authority).unwrap()).unwrap(),
        })
        .collect::<Vec<_>>();
    let matching_bundle = candidate_bundle("candidate compiled through the existing E02 path");
    let (material_manifest, material_files, material_binding) =
        candidate_material_package(&matching_bundle);
    let mut stage_request = persistent_stage_request(
        "candidate-handoff-stage",
        material_manifest,
        material_files,
        staged_sources.clone(),
    );
    stage_request.candidate_material = Some(material_binding);
    let staged = PersistentPackageStore::stage_package(&admin, &store, stage_request)
        .await
        .unwrap();
    let candidate_sources = [&first, &second]
        .into_iter()
        .map(|authority| TypedSourceRef {
            kind: "run".into(),
            id: authority.record.id.clone(),
            content_digest: authority.trace.source_digest.clone(),
        })
        .collect::<Vec<_>>();
    ReleaseStore::stage_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "package-candidate-wrong-content".into(),
            bundle: candidate_bundle("different content with the same source closure"),
            environment_digest: hash(b"environment"),
            proposer_actor: proposer.actor().into(),
            sources: candidate_sources.clone(),
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    let mut unauthorized_strategy_bundle = matching_bundle.clone();
    unauthorized_strategy_bundle.improver.instruction =
        "package supplied improver change is not locally authorized".into();
    let (unauthorized_manifest, unauthorized_files, unauthorized_binding) =
        candidate_material_package(&unauthorized_strategy_bundle);
    let mut unauthorized_request = persistent_stage_request(
        "unauthorized-strategy-stage",
        unauthorized_manifest,
        unauthorized_files,
        staged_sources,
    );
    unauthorized_request.candidate_material = Some(unauthorized_binding);
    assert!(
        PersistentPackageStore::stage_package(&admin, &store, unauthorized_request)
            .await
            .is_err()
    );
    assert!(
        PersistentPackageStore::record_candidate_handoff(
            &admin,
            &store,
            &staged.id,
            "package-candidate-wrong-content",
        )
        .await
        .is_err()
    );
    ReleaseStore::stage_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "package-candidate-missing-source".into(),
            bundle: matching_bundle.clone(),
            environment_digest: hash(b"environment"),
            proposer_actor: proposer.actor().into(),
            sources: vec![candidate_sources[0].clone()],
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    assert!(
        PersistentPackageStore::record_candidate_handoff(
            &admin,
            &store,
            &staged.id,
            "package-candidate-missing-source",
        )
        .await
        .is_err()
    );
    ReleaseStore::stage_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "package-candidate-1".into(),
            bundle: matching_bundle,
            environment_digest: hash(b"environment"),
            proposer_actor: proposer.actor().into(),
            sources: candidate_sources,
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();

    let handed_off = PersistentPackageStore::record_candidate_handoff(
        &admin,
        &store,
        &staged.id,
        "package-candidate-1",
    )
    .await
    .unwrap();
    assert_eq!(
        handed_off.payload.candidate_ref.as_deref(),
        Some("package-candidate-1")
    );
    assert_eq!(handed_off.payload.state, StagedAssetState::Staged);
    assert_eq!(
        PersistentPackageStore::record_candidate_handoff(
            &admin,
            &store,
            &staged.id,
            "package-candidate-1",
        )
        .await
        .unwrap()
        .payload
        .candidate_ref
        .as_deref(),
        Some("package-candidate-1")
    );
    drop(store);
    let reopened = Store::open(&database_path).await.unwrap();
    assert_eq!(
        PersistentPackageStore::read_staged(&admin, &reopened, &staged.id)
            .await
            .unwrap()
            .payload
            .candidate_ref
            .as_deref(),
        Some("package-candidate-1")
    );
}

#[tokio::test]
async fn persistent_export_rechecks_midflight_revoke_and_keeps_delivery_audit() {
    let (_directory, store, admin, sources) = persistent_store().await;
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request(
            "stage-export",
            valid_base_manifest(),
            valid_files(),
            sources,
        ),
    )
    .await
    .unwrap();
    let attempt = PersistentPackageStore::prepare_export(
        &admin,
        &store,
        "export-key",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    assert_eq!(attempt.payload.state, ExportAttemptState::Prepared);
    let (completed, audit, receipt) =
        PersistentPackageStore::complete_export(&admin, &store, &attempt.id)
            .await
            .unwrap();
    assert_eq!(completed.payload.state, ExportAttemptState::Completed);
    assert!(!audit.payload.revoked);
    assert_eq!(receipt.delivery_kind, "local_directory_v1");
    let output = store.local_export_directory(&admin, &attempt.id).unwrap();
    assert_eq!(
        tokio::fs::read(output.join("skill.md")).await.unwrap(),
        valid_files()["skill.md"]
    );
    assert_eq!(
        tokio::fs::read(output.join("config.json")).await.unwrap(),
        valid_files()["config.json"]
    );
    let manifest: AssetPackageManifest =
        serde_json::from_slice(&tokio::fs::read(output.join("manifest.json")).await.unwrap())
            .unwrap();
    assert_eq!(manifest.asset_id, staged.payload.asset_id);
    assert_eq!(
        PersistentPackageStore::complete_export(&admin, &store, &attempt.id)
            .await
            .unwrap()
            .1
            .input_digest,
        audit.input_digest
    );
    assert!(
        PersistentPackageStore::prepare_export(
            &admin,
            &store,
            "remote-export-rejected",
            &staged.id,
            "external_partner",
            None,
        )
        .await
        .is_err()
    );
    let aborted = PersistentPackageStore::prepare_export(
        &admin,
        &store,
        "aborted-local-export",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    let aborted = PersistentPackageStore::abort_export(&admin, &store, &aborted.id)
        .await
        .unwrap();
    assert_eq!(aborted.payload.state, ExportAttemptState::Aborted);
    assert!(
        !tokio::fs::try_exists(store.local_export_directory(&admin, &aborted.id).unwrap())
            .await
            .unwrap()
    );

    let attempt2 = PersistentPackageStore::prepare_export(
        &admin,
        &store,
        "export-key-2",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "tombstone",
            "package-source-2",
            admin.actor(),
            &serde_json::json!({"id":"package-source-2"}),
        )
        .await
        .unwrap();
    session
        .bump_watermark(&admin, "revoke-during-export")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(
        PersistentPackageStore::complete_export(&admin, &store, &attempt2.id)
            .await
            .is_err()
    );
}

async fn write_exact_export_directory(
    store: &Store,
    admin: &Context,
    export_id: &str,
    manifest: &AssetPackageManifest,
    files: &BTreeMap<String, Vec<u8>>,
) {
    let root = store.local_export_directory(admin, export_id).unwrap();
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .await
    .unwrap();
    let mut manifest_permissions = tokio::fs::metadata(root.join("manifest.json"))
        .await
        .unwrap()
        .permissions();
    manifest_permissions.set_readonly(true);
    tokio::fs::set_permissions(root.join("manifest.json"), manifest_permissions)
        .await
        .unwrap();
    for (path, bytes) in files {
        let target = root.join(path);
        tokio::fs::create_dir_all(target.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&target, bytes).await.unwrap();
        let mut permissions = tokio::fs::metadata(&target).await.unwrap().permissions();
        permissions.set_readonly(true);
        tokio::fs::set_permissions(target, permissions)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn local_export_recovers_exact_published_tree_and_rejects_existing_other_tree() {
    let (_directory, store, admin, sources) = persistent_store().await;
    let files = valid_files();
    let manifest = manifest_for_files("recovery-package", &files);
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request(
            "recovery-stage",
            manifest.clone(),
            files.clone(),
            sources.clone(),
        ),
    )
    .await
    .unwrap();
    let recoverable = PersistentPackageStore::prepare_export(
        &admin,
        &store,
        "recoverable-export",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    write_exact_export_directory(&store, &admin, &recoverable.id, &manifest, &files).await;
    let (completed, _, _) =
        PersistentPackageStore::complete_export(&admin, &store, &recoverable.id)
            .await
            .unwrap();
    assert_eq!(completed.payload.state, ExportAttemptState::Completed);

    let staged2 = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request("different-stage", manifest, files, sources),
    )
    .await
    .unwrap();
    let different = PersistentPackageStore::prepare_export(
        &admin,
        &store,
        "different-export",
        &staged2.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    let target = store.local_export_directory(&admin, &different.id).unwrap();
    tokio::fs::create_dir_all(&target).await.unwrap();
    tokio::fs::write(target.join("manifest.json"), b"different")
        .await
        .unwrap();
    assert!(
        PersistentPackageStore::complete_export(&admin, &store, &different.id)
            .await
            .is_err()
    );
    let mut session = store.session().await.unwrap();
    let unchanged: E16Envelope<evo_engine::packages::ExportAttemptPayload> = session
        .need(&admin, "artifact", &different.id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(unchanged.payload.state, ExportAttemptState::Prepared);
    assert!(unchanged.payload.delivery_audit_id.is_none());
}

#[tokio::test]
async fn completed_local_export_missing_or_changed_is_unavailable_and_cleanup_preserves_audit() {
    let (_directory, store, admin, _sources) = persistent_store().await;
    let host = Context::new("n", "host", Role::Host).unwrap();
    let authority = trusted_authority("cleanup-export-source", "cleanup export evidence");
    store_trace_authority(&store, &host, &authority)
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let stored_source: serde_json::Value = session
        .need(&admin, "run", "cleanup-export-source")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let sources = vec![E16SourceRef {
        kind: "run".into(),
        id: "cleanup-export-source".into(),
        digest: fingerprint(&stored_source).unwrap(),
    }];
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request(
            "tamper-stage",
            valid_base_manifest(),
            valid_files(),
            sources,
        ),
    )
    .await
    .unwrap();
    let attempt = PersistentPackageStore::prepare_export(
        &admin,
        &store,
        "tamper-export",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    let (completed, audit, _) =
        PersistentPackageStore::complete_export(&admin, &store, &attempt.id)
            .await
            .unwrap();
    let output = store.local_export_directory(&admin, &completed.id).unwrap();
    tokio::fs::remove_file(output.join("skill.md"))
        .await
        .unwrap();
    assert!(
        PersistentPackageStore::complete_export(&admin, &store, &attempt.id)
            .await
            .is_err()
    );
    tokio::fs::write(output.join("skill.md"), valid_files()["skill.md"].clone())
        .await
        .unwrap();

    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "cleanup-export-source".into(),
        },
        "remove controlled local export",
        50,
    )
    .await
    .unwrap();
    for now in 51..120 {
        if status.state == CleanupState::Complete {
            break;
        }
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 32, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete);
    assert!(!tokio::fs::try_exists(output).await.unwrap());
    let mut session = store.session().await.unwrap();
    let preserved: serde_json::Value = session.need(&admin, "artifact", &audit.id).await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(preserved["schema_version"], "rsia.e16.delivery_audit.v2");
    assert_eq!(preserved["payload"]["revoked"], true);
    assert_eq!(preserved["payload"]["revocation_notice_sent"], false);
}

#[tokio::test]
async fn local_export_write_failure_never_marks_completed_or_creates_audit() {
    let (_directory, store, admin, sources) = persistent_store().await;
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request(
            "write-failure-stage",
            valid_base_manifest(),
            valid_files(),
            sources,
        ),
    )
    .await
    .unwrap();
    let attempt = PersistentPackageStore::prepare_export(
        &admin,
        &store,
        "write-failure-export",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    let output = store.local_export_directory(&admin, &attempt.id).unwrap();
    let namespace_directory = output.parent().unwrap();
    tokio::fs::create_dir_all(namespace_directory.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(namespace_directory, b"not a directory")
        .await
        .unwrap();
    assert!(
        PersistentPackageStore::complete_export(&admin, &store, &attempt.id)
            .await
            .is_err()
    );
    let mut session = store.session().await.unwrap();
    let current: E16Envelope<evo_engine::packages::ExportAttemptPayload> =
        session.need(&admin, "artifact", &attempt.id).await.unwrap();
    let artifacts: Vec<serde_json::Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(current.payload.state, ExportAttemptState::Prepared);
    assert!(current.payload.delivery_audit_id.is_none());
    assert!(
        artifacts.iter().all(|value| {
            value["schema_version"].as_str() != Some("rsia.e16.delivery_audit.v2")
        })
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cleanup_rejects_linked_export_ancestor_and_preserves_external_canary() {
    use std::os::unix::fs::symlink;

    let (directory, store, admin, _sources) = persistent_store().await;
    let host = Context::new("n", "host", Role::Host).unwrap();
    let authority = trusted_authority("linked-export-source", "linked export evidence");
    store_trace_authority(&store, &host, &authority)
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let source: serde_json::Value = session
        .need(&admin, "run", "linked-export-source")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request(
            "linked-cleanup-stage",
            valid_base_manifest(),
            valid_files(),
            vec![E16SourceRef {
                kind: "run".into(),
                id: "linked-export-source".into(),
                digest: fingerprint(&source).unwrap(),
            }],
        ),
    )
    .await
    .unwrap();
    let attempt = PersistentPackageStore::prepare_export(
        &admin,
        &store,
        "linked-cleanup-export",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    PersistentPackageStore::complete_export(&admin, &store, &attempt.id)
        .await
        .unwrap();
    let output = store.local_export_directory(&admin, &attempt.id).unwrap();
    let export_root = output.parent().unwrap().parent().unwrap().to_path_buf();
    let saved = directory.path().join("saved-exports");
    tokio::fs::rename(&export_root, &saved).await.unwrap();
    let external = directory.path().join("external-canary");
    tokio::fs::create_dir_all(&external).await.unwrap();
    let canary = external.join("DO_NOT_DELETE");
    tokio::fs::write(&canary, b"canary").await.unwrap();
    symlink(&external, &export_root).unwrap();

    let mut status = LifecycleStore::begin_revoke(
        &admin,
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: "linked-export-source".into(),
        },
        "linked export cleanup",
        300,
    )
    .await
    .unwrap();
    for now in 301..380 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 32, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Failed);
    assert_eq!(tokio::fs::read(&canary).await.unwrap(), b"canary");
}

#[tokio::test]
async fn local_export_abort_and_publish_race_has_one_terminal_fact() {
    let (_directory, store, admin, sources) = persistent_store().await;
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request("race-stage", valid_base_manifest(), valid_files(), sources),
    )
    .await
    .unwrap();
    let attempt = PersistentPackageStore::prepare_export(
        &admin,
        &store,
        "race-export",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    let (complete, abort) = tokio::join!(
        PersistentPackageStore::complete_export(&admin, &store, &attempt.id),
        PersistentPackageStore::abort_export(&admin, &store, &attempt.id)
    );
    assert_ne!(complete.is_ok(), abort.is_ok());
    let mut session = store.session().await.unwrap();
    let final_attempt: E16Envelope<evo_engine::packages::ExportAttemptPayload> =
        session.need(&admin, "artifact", &attempt.id).await.unwrap();
    let audits: Vec<serde_json::Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    let output_exists =
        tokio::fs::try_exists(store.local_export_directory(&admin, &attempt.id).unwrap())
            .await
            .unwrap();
    match final_attempt.payload.state {
        ExportAttemptState::Completed => {
            assert!(output_exists);
            assert_eq!(
                audits
                    .iter()
                    .filter(|value| value["schema_version"] == "rsia.e16.delivery_audit.v2")
                    .count(),
                1
            );
        }
        ExportAttemptState::Aborted => {
            assert!(!output_exists);
            assert!(audits.iter().all(|value| {
                value["schema_version"].as_str() != Some("rsia.e16.delivery_audit.v2")
            }));
        }
        state => panic!("unexpected terminal export state: {state:?}"),
    }
}

#[tokio::test]
async fn legacy_metadata_only_completed_export_is_not_local_delivery() {
    let (_directory, store, admin, _sources) = persistent_store().await;
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            "legacy-export",
            admin.actor(),
            &serde_json::json!({
                "schema_version":"rsia.e16.export_attempt.v1",
                "id":"legacy-export",
                "namespace":"n",
                "owner_actor":"admin",
                "request_key":"legacy",
                "input_digest":hash(b"legacy"),
                "created_at":1,
                "updated_at":1,
                "source_refs":[],
                "revoke_watermark":1,
                "payload":{"state":"completed"}
            }),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let error = PersistentPackageStore::complete_export(&admin, &store, "legacy-export")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("legacy export attempt"));
}

#[tokio::test]
async fn local_export_and_source_revoke_race_preserves_only_actual_delivery_fact() {
    let (_directory, store, admin, _sources) = persistent_store().await;
    let host = Context::new("n", "host", Role::Host).unwrap();
    let authority = trusted_authority("race-export-source", "race export evidence");
    store_trace_authority(&store, &host, &authority)
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let source: serde_json::Value = session
        .need(&admin, "run", "race-export-source")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request(
            "revoke-race-stage",
            valid_base_manifest(),
            valid_files(),
            vec![E16SourceRef {
                kind: "run".into(),
                id: "race-export-source".into(),
                digest: fingerprint(&source).unwrap(),
            }],
        ),
    )
    .await
    .unwrap();
    let attempt = PersistentPackageStore::prepare_export(
        &admin,
        &store,
        "revoke-race-export",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    let (delivery, revoke) = tokio::join!(
        PersistentPackageStore::complete_export(&admin, &store, &attempt.id),
        LifecycleStore::begin_revoke(
            &admin,
            &store,
            TypedObjectRef {
                kind: "run".into(),
                id: "race-export-source".into(),
            },
            "race source revoke",
            200,
        )
    );
    let mut status = revoke.unwrap();
    for now in 201..280 {
        if status.state == CleanupState::Complete {
            break;
        }
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 32, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete);
    assert!(
        !tokio::fs::try_exists(store.local_export_directory(&admin, &attempt.id).unwrap())
            .await
            .unwrap()
    );
    let mut session = store.session().await.unwrap();
    let audits: Vec<serde_json::Value> = session.list(&admin, "artifact").await.unwrap();
    session.commit().await.unwrap();
    let audit_count = audits
        .iter()
        .filter(|value| value["schema_version"] == "rsia.e16.delivery_audit.v2")
        .count();
    assert_eq!(audit_count, usize::from(delivery.is_ok()));
}

#[tokio::test]
async fn persistent_multi_member_package_restart_and_export_preserves_ten_mib_scope() {
    let (directory, store, admin, sources) = persistent_store().await;
    let files = BTreeMap::from([
        ("large/first.txt".into(), vec![b'a'; 600 * 1024]),
        ("large/second.txt".into(), vec![b'b'; 600 * 1024]),
    ]);
    let manifest = manifest_for_files("large_package", &files);
    let request = persistent_stage_request("large-stage", manifest, files, sources);
    let staged = PersistentPackageStore::stage_package(&admin, &store, request.clone())
        .await
        .unwrap();
    assert!(staged.payload.content_bytes > 1024 * 1024);
    let mut interrupted = staged.clone();
    interrupted.payload.state = StagedAssetState::Prepared;
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            &interrupted.id,
            admin.actor(),
            &interrupted,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;

    let reopened = Store::open(&directory.path().join("packages.sqlite3"))
        .await
        .unwrap();
    let live = PersistentPackageStore::stage_package(&admin, &reopened, request)
        .await
        .unwrap();
    assert_eq!(live.payload.state, StagedAssetState::Staged);
    assert_eq!(
        live.payload.content_blob_digest,
        staged.payload.content_blob_digest
    );
    let attempt = PersistentPackageStore::prepare_export(
        &admin,
        &reopened,
        "large-export",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap();
    let (completed, audit, receipt) =
        PersistentPackageStore::complete_export(&admin, &reopened, &attempt.id)
            .await
            .unwrap();
    assert_eq!(completed.payload.state, ExportAttemptState::Completed);
    assert_eq!(
        audit.payload.projection_blob_digest,
        staged.payload.content_blob_digest
    );
    assert_eq!(
        receipt.package_tree_digest,
        audit.payload.package_tree_digest
    );
    reopened.close().await;
    let restarted = Store::open(&directory.path().join("packages.sqlite3"))
        .await
        .unwrap();
    let (_, restarted_audit, restarted_receipt) =
        PersistentPackageStore::complete_export(&admin, &restarted, &attempt.id)
            .await
            .unwrap();
    assert_eq!(restarted_audit.id, audit.id);
    assert_eq!(restarted_receipt, receipt);
    let export_parent = restarted
        .local_export_directory(&admin, &attempt.id)
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut entries = tokio::fs::read_dir(export_parent).await.unwrap();
    let mut count = 0;
    while entries.next_entry().await.unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 1);
}

#[tokio::test]
async fn persistent_package_rejects_raw_total_overflow_and_bounds_json_escape_expansion() {
    let (_directory, store, admin, sources) = persistent_store().await;
    let oversized = (0..11)
        .map(|index| (format!("oversized/{index}.txt"), vec![b'a'; 1024 * 1024]))
        .collect::<BTreeMap<_, _>>();
    assert!(
        PersistentPackageStore::stage_package(
            &admin,
            &store,
            persistent_stage_request(
                "oversized-stage",
                manifest_for_files("oversized_package", &oversized),
                oversized,
                sources.clone(),
            ),
        )
        .await
        .is_err()
    );

    let mut escaped = BTreeMap::new();
    for index in 0..9 {
        escaped.insert(format!("escaped/{index}.txt"), vec![0x01; 1024 * 1024]);
    }
    escaped.insert("escaped/last.txt".into(), vec![0x01; 700 * 1024]);
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request(
            "escaped-stage",
            manifest_for_files("escaped_package", &escaped),
            escaped,
            sources,
        ),
    )
    .await
    .unwrap();
    assert!(staged.payload.content_bytes > 50 * 1024 * 1024);
    assert!(staged.payload.content_bytes <= 64 * 1024 * 1024);
}

#[test]
fn final_projection_scans_sensitive_entry_paths_and_manifest_fields() {
    let mut manifest = valid_base_manifest();
    manifest.entries[0].path = "ghp_secretTokenVal123.txt".into();
    let mut files = valid_files();
    let content = files.remove("skill.md").unwrap();
    files.insert("ghp_secretTokenVal123.txt".into(), content);
    let req = ExportRequest {
        publisher: manifest.publisher.clone(),
        asset_id: manifest.asset_id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
        kind: manifest.kind,
        license: manifest.license.clone(),
        files,
        dependencies: vec![],
        referenced_source_ids: vec!["source".into()],
        export_started_watermark: 1,
    };
    assert!(export_package(&req, |_| false, 1).is_err());

    let mut metadata = valid_base_manifest();
    metadata.foreign_metadata = Some(ForeignMetadata {
        external_namespace: None,
        external_parent_id: None,
        formal_evaluations: vec![ForeignEvaluationClaim {
            evaluator_identity: "foreign".into(),
            evaluated_at: 1,
            score: "sk-secret-in-score".into(),
            notes: "safe".into(),
        }],
        approvals: vec![],
        disclaimer: "safe".into(),
    });
    assert!(validate_manifest(&metadata).is_err());
}

#[tokio::test]
async fn persistent_shared_dependency_edges_block_uninstall() {
    let (_directory, store, admin, sources) = persistent_store().await;
    let dependency = PackageDependency {
        publisher: "shared.publisher".into(),
        asset_id: "shared-asset".into(),
        kind: "skill".into(),
        version_req: "1".into(),
    };
    let dependency_id = package_dependency_ref_id(&dependency).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "artifact",
            &dependency_id,
            admin.actor(),
            &serde_json::json!({"id":dependency_id}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut first_manifest = valid_base_manifest();
    first_manifest.dependencies.push(dependency.clone());
    let first = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request(
            "shared-first",
            first_manifest,
            valid_files(),
            sources.clone(),
        ),
    )
    .await
    .unwrap();
    let mut second_manifest = valid_base_manifest();
    second_manifest.asset_id = "test_asset_2".into();
    second_manifest.dependencies.push(dependency);
    let second = PersistentPackageStore::stage_package(
        &admin,
        &store,
        persistent_stage_request("shared-second", second_manifest, valid_files(), sources),
    )
    .await
    .unwrap();
    assert_ne!(first.id, second.id);
    assert!(
        PersistentPackageStore::can_safely_uninstall_persistent(
            &admin,
            &store,
            &dependency_id,
            &first.id,
        )
        .await
        .is_err()
    );
}
