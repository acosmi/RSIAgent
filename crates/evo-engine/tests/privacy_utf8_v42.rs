//! V067: existing privacy findings retain their policy on valid UTF-8 input.
//! This checks snippet boundaries, not completeness of sensitive-data detection.

use evo_core::{Context, Error, Role, hash, now};
use evo_engine::packages::{
    AssetPackageManifest, ExportRequest, FindingSeverity, PACKAGE_SCHEMA_V1, PRIVACY_DISCLAIMER,
    PackageEntry, PackageKind, PackageState, PersistentPackageStore, PrivacyFinding,
    RedactionReport, RedactionStatus, StagePackageRequest, export_package, import_external_package,
    privacy_block, scan_privacy,
};
use evo_storage::Store;
use std::collections::BTreeMap;
use std::panic::catch_unwind;

struct Family {
    rule: &'static str,
    detail: &'static str,
    cap: usize,
    markers: &'static [&'static str],
}

const TOKENS: Family = Family {
    rule: "secret_token",
    detail: "contains secret token prefix",
    cap: 12,
    markers: &["sk-", "ghp_", "gho_", "ghs_", "xoxb-", "xoxp-", "AIza"],
};
const PATHS: Family = Family {
    rule: "local_user_path",
    detail: "contains local user path",
    cap: 24,
    markers: &["/Users/", "/home/", "C:\\Users\\", "C:/Users/"],
};
const NETWORKS: Family = Family {
    rule: "internal_network",
    detail: "contains internal network address/domain",
    cap: 16,
    markers: &[
        "192.168.",
        "10.0.",
        "10.1.",
        "172.16.",
        ".corp",
        ".internal",
        ".local",
    ],
};
const DISCLAIMER: &str =
    "Privacy scan does not guarantee absolute absence of sensitive data; human audit is required.";

fn blocked_finding(rule: &str, detail: &str, snippet: &str) -> PrivacyFinding {
    PrivacyFinding {
        rule: rule.into(),
        severity: FindingSeverity::Block,
        detail: detail.into(),
        snippet: snippet.into(),
    }
}

fn warning_finding() -> PrivacyFinding {
    PrivacyFinding {
        rule: "suspicious_comment".into(),
        severity: FindingSeverity::Warning,
        detail: "contains suspicious todo comment referencing secret/token".into(),
        snippet: "TODO/FIXME reference".into(),
    }
}

fn assert_report(report: &RedactionReport, status: RedactionStatus, findings: &[PrivacyFinding]) {
    assert_eq!(report.status, status);
    assert_eq!(report.findings_count, findings.len());
    assert_eq!(report.findings, findings);
    assert_eq!(report.disclaimer, DISCLAIMER);
}

fn check_family(family: &Family) {
    let mut panicked = Vec::new();
    let mut checked = 0;
    for marker in family.markers {
        let mut cases = vec![(marker.to_string(), marker.to_string())];
        cases.push((format!("{marker}é"), format!("{marker}é")));
        for scalar in ["é", "中", "🙂"] {
            // The old byte window ends one byte into this scalar.
            let crossing = format!("{marker}{}", "a".repeat(family.cap - marker.len() - 1));
            cases.push((format!("{crossing}{scalar}!"), crossing));
            // A scalar ending at the cap must be retained in full.
            let exact = format!(
                "{marker}{}{scalar}",
                "a".repeat(family.cap - marker.len() - scalar.len())
            );
            cases.push((format!("{exact}!"), exact));
            // A scalar before the cap must not displace the final ASCII byte.
            let before = format!(
                "{marker}{}{scalar}b",
                "a".repeat(family.cap - marker.len() - scalar.len() - 1)
            );
            cases.push((format!("{before}!"), before));
            cases.push((
                format!("{marker}{}", scalar.repeat(family.cap)),
                format!(
                    "{marker}{}",
                    scalar.repeat((family.cap - marker.len()) / scalar.len())
                ),
            ));
        }
        for prefix in ["ASCII: ", "说明🙂："] {
            for (body, expected) in &cases {
                let text = format!("{prefix}{body}");
                checked += 1;
                // Collect every baseline panic; any collected panic fails the test.
                let report = match catch_unwind(|| scan_privacy(&text)) {
                    Ok(report) => report,
                    Err(_) => {
                        panicked.push(text);
                        continue;
                    }
                };
                let finding = blocked_finding(
                    family.rule,
                    &format!("{} '{marker}'", family.detail),
                    expected,
                );
                if *marker == "C:/Users/" {
                    // Existing policy also matches /Users/ inside C:/Users/ first.
                    assert_eq!(report.status, RedactionStatus::Blocked);
                    assert_eq!(report.findings_count, 2);
                    assert_eq!(report.findings.len(), 2);
                    assert_eq!(report.disclaimer, DISCLAIMER);
                    assert_eq!(report.findings[1], finding);
                    let overlap = &report.findings[0];
                    assert_eq!(overlap.rule, "local_user_path");
                    assert_eq!(overlap.severity, FindingSeverity::Block);
                    assert_eq!(overlap.detail, "contains local user path '/Users/'");
                    assert!(overlap.snippet.starts_with("/Users/"));
                    assert!(overlap.snippet.len() <= 24);
                } else {
                    assert_report(&report, RedactionStatus::Blocked, &[finding]);
                }
                let primary = report.findings.last().unwrap();
                assert!(primary.snippet.starts_with(*marker));
                assert!(primary.snippet.len() <= family.cap);
            }
        }
    }
    assert_eq!(checked, family.markers.len() * 28);
    assert!(
        panicked.is_empty(),
        "{} panicked on {} of {checked} valid UTF-8 cases:\n{}",
        family.rule,
        panicked.len(),
        panicked.join("\n")
    );
}

#[test]
fn token_markers_keep_blocking_at_utf8_boundaries() {
    check_family(&TOKENS);
}

#[test]
fn path_markers_keep_blocking_at_utf8_boundaries() {
    check_family(&PATHS);
}

#[test]
fn network_markers_keep_blocking_at_utf8_boundaries() {
    check_family(&NETWORKS);
}

#[test]
fn fixed_literal_snippets_cover_each_partial_scalar_offset() {
    // These expected strings are literal examples, independent of a boundary helper.
    let cases = [
        ("sk-abcdefghéX", "sk-abcdefgh", 12),
        ("ghp_abcdef中X", "ghp_abcdef", 12),
        ("AIzaabcde🙂X", "AIzaabcde", 12),
        ("ghp_abcde中X", "ghp_abcde中", 12),
        ("/home/abcdefghijklmnopqéX", "/home/abcdefghijklmnopq", 24),
        ("/Users/abcdefghijklmno中X", "/Users/abcdefghijklmno", 24),
        ("C:/Users/abcdefghijkl🙂X", "C:/Users/abcdefghijkl", 24),
        ("/Users/abcdefghijklmn中X", "/Users/abcdefghijklmn中", 24),
        (".corp1234567890éX", ".corp1234567890", 16),
        ("10.0.abcdefghi中X", "10.0.abcdefghi", 16),
        (".internalabcd🙂X", ".internalabcd", 16),
        ("172.16.abcdef中X", "172.16.abcdef中", 16),
        ("sk-a🙂béX", "sk-a🙂béX", 12),
    ];
    for (text, expected, cap) in cases {
        let report = scan_privacy(text);
        assert_eq!(report.status, RedactionStatus::Blocked);
        if text == "C:/Users/abcdefghijkl🙂X" {
            assert_eq!(report.findings_count, 2);
            assert_eq!(report.findings[0].snippet, "/Users/abcdefghijkl🙂X");
            assert_eq!(report.findings[1].snippet, expected);
        } else {
            assert_eq!(report.findings_count, 1);
            assert_eq!(report.findings[0].snippet, expected, "input: {text}");
        }
        for finding in &report.findings {
            assert!(finding.snippet.len() <= cap);
        }
    }
}

#[test]
fn full_ascii_report_matches_baseline_snapshot_except_clock() {
    let baseline = [
        (
            "private_key",
            "contains private key marker 'BEGIN PRIVATE KEY'",
            "BEGIN PRIVATE KEY",
        ),
        (
            "private_key",
            "contains private key marker 'BEGIN RSA PRIVATE KEY'",
            "BEGIN RSA PRIVATE KEY",
        ),
        (
            "private_key",
            "contains private key marker 'BEGIN OPENSSH PRIVATE KEY'",
            "BEGIN OPENSSH PRIVATE KEY",
        ),
        (
            "private_key",
            "contains private key marker 'BEGIN EC PRIVATE KEY'",
            "BEGIN EC PRIVATE KEY",
        ),
        (
            "private_key",
            "contains private key marker 'BEGIN DSA PRIVATE KEY'",
            "BEGIN DSA PRIVATE KEY",
        ),
        (
            "private_key",
            "contains private key marker 'BEGIN PGP PRIVATE KEY BLOCK'",
            "BEGIN PGP PRIVATE KEY BLOCK",
        ),
        (
            "secret_token",
            "contains secret token prefix 'sk-'",
            "sk-123456789",
        ),
        (
            "secret_token",
            "contains secret token prefix 'ghp_'",
            "ghp_12345678",
        ),
        (
            "secret_token",
            "contains secret token prefix 'gho_'",
            "gho_12345678",
        ),
        (
            "secret_token",
            "contains secret token prefix 'ghs_'",
            "ghs_12345678",
        ),
        (
            "secret_token",
            "contains secret token prefix 'xoxb-'",
            "xoxb-1234567",
        ),
        (
            "secret_token",
            "contains secret token prefix 'xoxp-'",
            "xoxp-1234567",
        ),
        (
            "secret_token",
            "contains secret token prefix 'AIza'",
            "AIza12345678",
        ),
        (
            "local_user_path",
            "contains local user path '/Users/'",
            "/Users/abcdefghijklmnopq",
        ),
        (
            "local_user_path",
            "contains local user path '/home/'",
            "/home/abcdefghijklmnopqr",
        ),
        (
            "local_user_path",
            "contains local user path 'C:\\Users\\'",
            "C:\\Users\\abcdefghijklmno",
        ),
        (
            "local_user_path",
            "contains local user path 'C:/Users/'",
            "C:/Users/abcdefghijklmno",
        ),
        (
            "hidden_answer",
            "contains hidden task answer marker 'ANSWER:'",
            "ANSWER:",
        ),
        (
            "hidden_answer",
            "contains hidden task answer marker 'GROUND_TRUTH:'",
            "GROUND_TRUTH:",
        ),
        (
            "hidden_answer",
            "contains hidden task answer marker 'HIDDEN_EVAL:'",
            "HIDDEN_EVAL:",
        ),
        (
            "hidden_answer",
            "contains hidden task answer marker 'oracle_eval:'",
            "oracle_eval:",
        ),
        (
            "hidden_answer",
            "contains hidden task answer marker 'oracle_solution:'",
            "oracle_solution:",
        ),
        (
            "internal_network",
            "contains internal network address/domain '192.168.'",
            "192.168.abcdefgh",
        ),
        (
            "internal_network",
            "contains internal network address/domain '10.0.'",
            "10.0.abcdefghijk",
        ),
        (
            "internal_network",
            "contains internal network address/domain '10.1.'",
            "10.1.abcdefghijk",
        ),
        (
            "internal_network",
            "contains internal network address/domain '172.16.'",
            "172.16.abcdefghi",
        ),
        (
            "internal_network",
            "contains internal network address/domain '.corp'",
            ".corpabcdefghijk",
        ),
        (
            "internal_network",
            "contains internal network address/domain '.internal'",
            ".internalabcdefg",
        ),
        (
            "internal_network",
            "contains internal network address/domain '.local'",
            ".localabcdefghij",
        ),
        (
            "raw_session_data",
            "contains raw session conversation marker '\"role\": \"user\"'",
            "\"role\": \"user\"",
        ),
        (
            "raw_session_data",
            "contains raw session conversation marker '\"role\": \"assistant\"'",
            "\"role\": \"assistant\"",
        ),
        (
            "raw_session_data",
            "contains raw session conversation marker 'chat_history'",
            "chat_history",
        ),
        (
            "credential_parameter",
            "contains sensitive credential parameter 'password='",
            "password=",
        ),
        (
            "credential_parameter",
            "contains sensitive credential parameter 'api_secret='",
            "api_secret=",
        ),
        (
            "credential_parameter",
            "contains sensitive credential parameter 'client_secret='",
            "client_secret=",
        ),
        (
            "credential_parameter",
            "contains sensitive credential parameter 'secret_key='",
            "secret_key=",
        ),
        (
            "credential_parameter",
            "contains sensitive credential parameter 'aws_secret_access_key'",
            "aws_secret_access_key",
        ),
    ];
    let mut text = baseline
        .iter()
        .map(|(_, _, snippet)| *snippet)
        .collect::<Vec<_>>()
        .join(" | ");
    text.push_str(" | TODO: secret | FIXME: token | sk-second-match-must-not-replace-first");
    assert!(text.is_ascii());
    let before = now();
    let mut report = scan_privacy(&text);
    let after = now();
    assert!((before..=after).contains(&report.scanned_at));
    let mut findings = baseline
        .iter()
        .map(|(rule, detail, snippet)| blocked_finding(rule, detail, snippet))
        .collect::<Vec<_>>();
    findings.push(warning_finding());
    assert_eq!(PRIVACY_DISCLAIMER, DISCLAIMER);
    assert_report(&report, RedactionStatus::Blocked, &findings);
    report.scanned_at = 0;
    let snapshot = RedactionReport {
        scanned_at: 0,
        findings_count: 38,
        status: RedactionStatus::Blocked,
        disclaimer: DISCLAIMER.into(),
        findings,
    };
    assert_eq!(report, snapshot);
    println!(
        "ASCII_BASELINE_SNAPSHOT={}",
        serde_json::to_string(&report).unwrap()
    );
}

#[test]
fn mixed_unicode_findings_keep_order_details_and_first_match() {
    let text = concat!(
        "password=示例 | .corp1234567890éZ | ghp_abcde中Z | ",
        "sk-abcdefgh🙂Z | /home/abcdefghijklmnopqéZ | ",
        "ANSWER:测试 | \"role\": \"user\" | BEGIN PRIVATE KEY | ",
        "TODO: secret | sk-second-match"
    );
    let mut expected = vec![
        blocked_finding(
            "private_key",
            "contains private key marker 'BEGIN PRIVATE KEY'",
            "BEGIN PRIVATE KEY",
        ),
        blocked_finding(
            "secret_token",
            "contains secret token prefix 'sk-'",
            "sk-abcdefgh",
        ),
        blocked_finding(
            "secret_token",
            "contains secret token prefix 'ghp_'",
            "ghp_abcde中",
        ),
        blocked_finding(
            "local_user_path",
            "contains local user path '/home/'",
            "/home/abcdefghijklmnopq",
        ),
        blocked_finding(
            "hidden_answer",
            "contains hidden task answer marker 'ANSWER:'",
            "ANSWER:",
        ),
        blocked_finding(
            "internal_network",
            "contains internal network address/domain '.corp'",
            ".corp1234567890",
        ),
        blocked_finding(
            "raw_session_data",
            "contains raw session conversation marker '\"role\": \"user\"'",
            "\"role\": \"user\"",
        ),
        blocked_finding(
            "credential_parameter",
            "contains sensitive credential parameter 'password='",
            "password=",
        ),
    ];
    expected.push(warning_finding());
    assert_report(&scan_privacy(text), RedactionStatus::Blocked, &expected);
}

#[test]
fn unicode_clean_warning_and_case_sensitive_policy_are_unchanged() {
    for text in [
        "普通中文和🙂说明",
        "é中🙂",
        "SK-example GHP_example",
        "中文.todo",
    ] {
        assert_report(&scan_privacy(text), RedactionStatus::Clean, &[]);
        assert!(privacy_block(text).is_ok());
    }
    for text in [
        "说明🙂 TODO: secret",
        "FIXME: token 中文",
        "TODO: secret FIXME: token 🙂",
    ] {
        assert_report(
            &scan_privacy(text),
            RedactionStatus::Warning,
            &[warning_finding()],
        );
        assert!(privacy_block(text).is_ok());
    }
    let report = scan_privacy("中文🙂 PASSWORD=样例");
    assert_report(
        &report,
        RedactionStatus::Blocked,
        &[blocked_finding(
            "credential_parameter",
            "contains sensitive credential parameter 'password='",
            "password=",
        )],
    );
}

#[test]
fn privacy_block_returns_original_error_for_unicode_sensitive_text() {
    for (text, expected) in [
        (
            "说明🙂 sk-abcdefgh中",
            "privacy_gate: contains secret token prefix 'sk-'",
        ),
        (
            "/Users/abcdefghijklmnopé",
            "privacy_gate: contains local user path '/Users/'",
        ),
        (
            ".corp1234567890🙂",
            "privacy_gate: contains internal network address/domain '.corp'",
        ),
    ] {
        let result = catch_unwind(|| privacy_block(text))
            .expect("privacy_block must return an error, not panic");
        assert!(matches!(result, Err(Error::Invalid(ref detail)) if detail == expected));
    }
}

fn export_request(body: &str) -> ExportRequest {
    ExportRequest {
        publisher: "org.rsia".into(),
        asset_id: "utf8_fixture".into(),
        name: "Unicode fixture".into(),
        version: "1".into(),
        description: "普通中文🙂".into(),
        kind: PackageKind::Skill,
        license: "MIT".into(),
        files: BTreeMap::from([("skill.md".into(), body.as_bytes().to_vec())]),
        dependencies: vec![],
        referenced_source_ids: vec![],
        export_started_watermark: 0,
    }
}

fn manifest_for_body(body: &str) -> AssetPackageManifest {
    AssetPackageManifest {
        schema_version: PACKAGE_SCHEMA_V1.into(),
        kind: PackageKind::Skill,
        publisher: "org.rsia".into(),
        asset_id: "utf8_fixture".into(),
        name: "Unicode fixture".into(),
        version: "1".into(),
        description: "普通中文🙂".into(),
        license: "MIT".into(),
        entries: vec![PackageEntry {
            path: "skill.md".into(),
            digest_sha256: hash(body.as_bytes()),
            size_bytes: body.len() as u64,
            compressed_bytes: body.len() as u64,
        }],
        dependencies: vec![],
        redaction_report: RedactionReport {
            scanned_at: 1,
            findings_count: 0,
            status: RedactionStatus::Clean,
            disclaimer: DISCLAIMER.into(),
            findings: vec![],
        },
        foreign_metadata: None,
    }
}

#[test]
fn existing_export_and_import_reject_unicode_sensitive_members() {
    let clean = export_package(&export_request("普通中文🙂正文"), |_| false, 0).unwrap();
    let staged =
        import_external_package(&clean.manifest, &clean.files, |_, _| true, |_, _| false).unwrap();
    assert_eq!(staged.state, PackageState::Staged);
    assert!(!staged.is_active);
    assert!(!staged.is_approved);
    for body in [
        "说明🙂 sk-abcdefgh中",
        "/home/abcdefghijklmnopqé",
        ".corp1234567890🙂",
    ] {
        let result = export_package(&export_request(body), |_| false, 0);
        assert!(
            matches!(result, Err(Error::Invalid(ref detail)) if detail == "privacy_gate: sensitive content detected in file 'skill.md'")
        );
        let result = import_external_package(
            &manifest_for_body(body),
            &BTreeMap::from([("skill.md".into(), body.as_bytes().to_vec())]),
            |_, _| true,
            |_, _| false,
        );
        assert!(
            matches!(result, Err(Error::Invalid(ref detail)) if detail.starts_with("privacy_gate: "))
        );
    }
}

#[tokio::test]
async fn persistent_stage_rejects_before_artifacts_blobs_or_active_pointer_changes() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("privacy.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("privacy", "admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    let pointers = session
        .list::<serde_json::Value>(&admin, "pointer")
        .await
        .unwrap();
    let artifacts = session
        .list::<serde_json::Value>(&admin, "artifact")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let body = "说明🙂 sk-abcdefgh中";
    let result = PersistentPackageStore::stage_package(
        &admin,
        &store,
        StagePackageRequest {
            request_key: "utf8-rejection".into(),
            manifest: manifest_for_body(body),
            files: BTreeMap::from([("skill.md".into(), body.as_bytes().to_vec())]),
            source_refs: vec![],
            baseline_digest: hash(b"baseline"),
            local_digest: hash(b"local"),
            upstream_digest: None,
            environment_digest: hash(b"environment"),
            compiler_version: "compiler-v1".into(),
            candidate_material: None,
        },
    )
    .await;
    assert!(
        matches!(result, Err(Error::Invalid(ref detail)) if detail == "privacy_gate: contains secret token prefix 'sk-'")
    );
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .list::<serde_json::Value>(&admin, "pointer")
            .await
            .unwrap(),
        pointers
    );
    assert_eq!(
        session
            .list::<serde_json::Value>(&admin, "artifact")
            .await
            .unwrap(),
        artifacts
    );
    session.commit().await.unwrap();
    assert!(
        !tokio::fs::try_exists(directory.path().join("blobs"))
            .await
            .unwrap()
    );
    assert!(
        !tokio::fs::try_exists(directory.path().join("exports"))
            .await
            .unwrap()
    );
    store.close().await;
}
