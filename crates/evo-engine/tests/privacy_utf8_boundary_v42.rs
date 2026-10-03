//! Limited E16.2/V067 UTF-8 snippet and existing privacy-rejection boundary.
//! Uses only the parent public scanner, privacy gate and pure export APIs.
//! Golden files are emitted only by separately authorized exact test invocations.

use evo_core::{Error, hash};
use evo_engine::packages::{
    ExportRequest, FindingSeverity, PRIVACY_DISCLAIMER, PackageDependency, PackageKind,
    PrivacyFinding, RedactionReport, RedactionStatus, export_package, privacy_block, scan_privacy,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::panic::{UnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

struct DynamicRule {
    name: &'static str,
    cap: usize,
    detail_prefix: &'static str,
    markers: &'static [&'static str],
}

const RULES: [DynamicRule; 3] = [
    DynamicRule {
        name: "secret_token",
        cap: 12,
        detail_prefix: "contains secret token prefix",
        markers: &["sk-", "ghp_", "gho_", "ghs_", "xoxb-", "xoxp-", "AIza"],
    },
    DynamicRule {
        name: "local_user_path",
        cap: 24,
        detail_prefix: "contains local user path",
        markers: &["/Users/", "/home/", "C:\\Users\\", "C:/Users/"],
    },
    DynamicRule {
        name: "internal_network",
        cap: 16,
        detail_prefix: "contains internal network address/domain",
        markers: &[
            "192.168.",
            "10.0.",
            "10.1.",
            "172.16.",
            ".corp",
            ".internal",
            ".local",
        ],
    },
];

const UNICODE: [&str; 3] = ["é", "中", "🧪"];
const PREFIXES: [&str; 2] = ["", "é中🧪 "];
const ASCII_MULTI: &str = "TODO: secret | 10.0.abcdefghijklmnop | /Users/abcdefghijklmnopqrstuv | ghp_abcdefghijklmnop | sk-abcdefghijklmnop | ANSWER: sample | BEGIN PRIVATE KEY";

fn observe<T>(label: &str, operation: impl FnOnce() -> T + UnwindSafe) -> T {
    let observed = catch_unwind(operation);
    println!(
        "AG085_NORMAL_RETURN label={label} normal={}",
        observed.is_ok()
    );
    assert!(
        observed.is_ok(),
        "{label}: valid UTF-8 must return normally"
    );
    observed.unwrap_or_else(|_| unreachable!("normal return asserted above"))
}

fn assert_dynamic_finding(
    text: &str,
    rule: &DynamicRule,
    marker: &str,
    expected_snippet: &str,
    label: &str,
) {
    let report = observe(label, || scan_privacy(text));
    assert_eq!(report.status, RedactionStatus::Blocked, "{label}");
    assert_eq!(report.findings_count, report.findings.len());
    assert_eq!(report.disclaimer, PRIVACY_DISCLAIMER);
    let detail = format!("{} '{marker}'", rule.detail_prefix);
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.rule == rule.name && finding.detail == detail)
        .expect("original rule and detail must be retained");
    assert_eq!(finding.severity, FindingSeverity::Block);
    assert_eq!(finding.snippet, expected_snippet, "{label}");
    assert!(finding.snippet.len() <= rule.cap, "{label}");
    let start = text.find(marker).expect("constructed marker is present");
    assert!(text[start..].starts_with(&finding.snippet));
    assert!(text.is_char_boundary(start + finding.snippet.len()));
    let gate = observe(label, || privacy_block(text));
    assert!(matches!(gate, Err(Error::Invalid(message)) if message.starts_with("privacy_gate:")));
}

fn inside_codepoint_cases(rule: &DynamicRule) {
    for &marker in rule.markers {
        for unicode in UNICODE {
            for split in 1..unicode.len() {
                for prefix in PREFIXES {
                    let padding = "a".repeat(rule.cap - marker.len() - split);
                    let expected = format!("{marker}{padding}");
                    let text = format!("{prefix}{expected}{unicode}tail");
                    let start = text.find(marker).unwrap();
                    assert_eq!(start, prefix.len());
                    assert!(!text.is_char_boundary(start + rule.cap));
                    let label = format!(
                        "{}/{marker}/width={}/split={split}/offset={start}",
                        rule.name,
                        unicode.len()
                    );
                    assert_dynamic_finding(&text, rule, marker, &expected, &label);
                }
            }
        }
    }
}

#[test]
fn secret_token_12_bytes_inside_each_unicode_width_returns_blocked() {
    inside_codepoint_cases(&RULES[0]);
}

#[test]
fn local_user_path_24_bytes_inside_each_unicode_width_returns_blocked() {
    inside_codepoint_cases(&RULES[1]);
}

#[test]
fn internal_network_16_bytes_inside_each_unicode_width_returns_blocked() {
    inside_codepoint_cases(&RULES[2]);
}

#[test]
fn exact_unicode_end_boundaries_retain_the_full_byte_cap() {
    for rule in &RULES {
        for &marker in rule.markers {
            for unicode in UNICODE {
                for prefix in PREFIXES {
                    let padding = "a".repeat(rule.cap - marker.len() - unicode.len());
                    let expected = format!("{marker}{padding}{unicode}");
                    let text = format!("{prefix}{expected}tail");
                    assert_eq!(expected.len(), rule.cap);
                    assert!(text.is_char_boundary(prefix.len() + rule.cap));
                    assert_dynamic_finding(&text, rule, marker, &expected, "exact boundary");
                }
            }
        }
    }
}

#[test]
fn short_unicode_text_and_marker_at_eof_keep_the_original_excerpt() {
    for rule in &RULES {
        for &marker in rule.markers {
            for prefix in PREFIXES {
                let at_eof = format!("{prefix}{marker}");
                assert_dynamic_finding(&at_eof, rule, marker, marker, "marker at EOF");
                for unicode in UNICODE {
                    let expected = format!("{marker}{unicode}");
                    assert!(expected.len() < rule.cap);
                    let text = format!("{prefix}{expected}");
                    assert_dynamic_finding(&text, rule, marker, &expected, "short Unicode at EOF");
                }
            }
        }
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

#[test]
fn clean_and_warning_unicode_keep_the_original_classification() {
    let clean = "é 中 🧪 café cafe\u{301}\r\n普通开发说明";
    let report = observe("clean Unicode", || scan_privacy(clean));
    assert_eq!(report.status, RedactionStatus::Clean);
    assert_eq!(report.findings_count, 0);
    assert!(report.findings.is_empty());
    assert_eq!(report.disclaimer, PRIVACY_DISCLAIMER);
    assert!(observe("clean Unicode gate", || privacy_block(clean)).is_ok());

    let warning = "é中🧪 前置 TODO: secret 后置";
    let report = observe("warning Unicode", || scan_privacy(warning));
    assert_eq!(report.status, RedactionStatus::Warning);
    assert_eq!(report.findings_count, 1);
    assert_eq!(report.findings, vec![warning_finding()]);
    assert_eq!(report.disclaimer, PRIVACY_DISCLAIMER);
    assert!(observe("warning Unicode gate", || privacy_block(warning)).is_ok());
}

#[test]
fn ascii_multiple_findings_keep_rule_order_details_and_exact_snippets() {
    let report = observe("ASCII multiple findings", || scan_privacy(ASCII_MULTI));
    let mut expected = vec![PrivacyFinding {
        rule: "private_key".into(),
        severity: FindingSeverity::Block,
        detail: "contains private key marker 'BEGIN PRIVATE KEY'".into(),
        snippet: "BEGIN PRIVATE KEY".into(),
    }];
    for (rule, detail, snippet) in [
        (
            "secret_token",
            "contains secret token prefix 'sk-'",
            "sk-abcdefghi",
        ),
        (
            "secret_token",
            "contains secret token prefix 'ghp_'",
            "ghp_abcdefgh",
        ),
        (
            "local_user_path",
            "contains local user path '/Users/'",
            "/Users/abcdefghijklmnopq",
        ),
        (
            "hidden_answer",
            "contains hidden task answer marker 'ANSWER:'",
            "ANSWER:",
        ),
        (
            "internal_network",
            "contains internal network address/domain '10.0.'",
            "10.0.abcdefghijk",
        ),
    ] {
        expected.push(PrivacyFinding {
            rule: rule.into(),
            severity: FindingSeverity::Block,
            detail: detail.into(),
            snippet: snippet.into(),
        });
    }
    expected.push(warning_finding());
    assert_eq!(report.status, RedactionStatus::Blocked);
    assert_eq!(report.findings_count, expected.len());
    assert_eq!(report.findings, expected);
    assert_eq!(report.disclaimer, PRIVACY_DISCLAIMER);
    assert!(matches!(
        observe("ASCII multiple gate", || privacy_block(ASCII_MULTI)),
        Err(Error::Invalid(_))
    ));
}

fn export_request(content: &str) -> ExportRequest {
    ExportRequest {
        publisher: "org.rsia".into(),
        asset_id: "utf8_privacy_boundary".into(),
        name: "Safe text package".into(),
        version: "1.0.0".into(),
        description: "A public development text fixture".into(),
        kind: PackageKind::Skill,
        license: "Apache-2.0".into(),
        files: BTreeMap::from([("skill.md".into(), content.as_bytes().to_vec())]),
        dependencies: vec![],
        referenced_source_ids: vec!["public-source".into()],
        export_started_watermark: 7,
    }
}

fn split_input(rule: &DynamicRule, unicode: &str) -> String {
    let marker = rule.markers[0];
    let padding = "a".repeat(rule.cap - marker.len() - 1);
    format!("é中🧪 {marker}{padding}{unicode}tail")
}

#[test]
fn pure_export_unicode_sensitive_members_return_privacy_invalid_without_panic() {
    for rule in &RULES {
        for unicode in UNICODE {
            let text = split_input(rule, unicode);
            let request = export_request(&text);
            let result = observe("pure export sensitive member", || {
                export_package(&request, |_| false, 7)
            });
            assert!(
                matches!(result, Err(Error::Invalid(message)) if message == "privacy_gate: sensitive content detected in file 'skill.md'")
            );
        }
    }
}

#[test]
fn pure_export_unicode_metadata_dependencies_and_sources_use_the_same_gate() {
    for rule in &RULES {
        for unicode in UNICODE {
            let text = split_input(rule, unicode);
            for field in ["description", "dependency.asset_id", "referenced_source_id"] {
                let mut request = export_request("safe package text");
                match field {
                    "description" => request.description = text.clone(),
                    "dependency.asset_id" => request.dependencies.push(PackageDependency {
                        publisher: "public".into(),
                        asset_id: text.clone(),
                        kind: "template".into(),
                        version_req: "1.0.0".into(),
                    }),
                    "referenced_source_id" => request.referenced_source_ids = vec![text.clone()],
                    _ => unreachable!(),
                }
                let result = observe(field, || export_package(&request, |_| false, 7));
                let expected = if field == "referenced_source_id" {
                    "privacy_gate: sensitive content detected in referenced_source_id".to_string()
                } else {
                    format!("privacy_gate: sensitive content detected in metadata field '{field}'")
                };
                assert!(matches!(result, Err(Error::Invalid(message)) if message == expected));
            }
        }
    }
}

#[test]
fn pure_export_clean_unicode_preserves_original_utf8_content_and_digest() {
    for text in [
        "é 中 🧪 café\n",
        "é 中 🧪 cafe\u{301}\r\n",
        "前置 TODO: secret 后置",
    ] {
        let mut request = export_request(text);
        request.name = "合法文本包 é中🧪".into();
        request.description = "普通开发说明 café cafe\u{301}".into();
        let prepared = observe("pure export valid Unicode", || {
            export_package(&request, |_| false, 7)
        })
        .expect("clean/warning valid UTF-8 export remains permitted");
        assert_eq!(prepared.files, request.files);
        let entry = &prepared.manifest.entries[0];
        assert_eq!(entry.path, "skill.md");
        assert_eq!(entry.size_bytes, text.len() as u64);
        assert_eq!(entry.compressed_bytes, text.len() as u64);
        assert_eq!(entry.digest_sha256, hash(text.as_bytes()));
        assert_eq!(prepared.manifest.name, request.name);
        assert_eq!(prepared.manifest.description, request.description);
        let expected = if text.contains("TODO: secret") {
            RedactionStatus::Warning
        } else {
            RedactionStatus::Clean
        };
        assert_eq!(prepared.manifest.redaction_report.status, expected);
    }
}

fn report_without_only_actual_time(report: RedactionReport) -> Value {
    let mut value = serde_json::to_value(report).expect("serialize actual scanner report");
    assert!(
        value
            .as_object_mut()
            .unwrap()
            .remove("scanned_at")
            .is_some()
    );
    value
}

fn actual_scan_material(text: &str) -> Value {
    let report = observe("actual golden scan", || scan_privacy(text));
    let gate = observe("actual golden privacy gate", || privacy_block(text));
    let gate = match gate {
        Ok(()) => json!({"result": "ok"}),
        Err(Error::Invalid(message)) => json!({"result": "invalid", "message": message}),
        Err(error) => panic!("unexpected privacy error: {error:?}"),
    };
    json!({"original_utf8": text, "original_sha256": hash(text.as_bytes()),
           "report": report_without_only_actual_time(report), "privacy_gate": gate})
}

fn actual_export_material(text: &str) -> Value {
    let request = export_request(text);
    let prepared = observe("actual golden pure export", || {
        export_package(&request, |_| false, 7)
    })
    .expect("positive-control pure export must succeed");
    let mut actual = serde_json::to_value(prepared).expect("serialize actual pure export");
    assert!(
        actual["manifest"]["redaction_report"]
            .as_object_mut()
            .unwrap()
            .remove("scanned_at")
            .is_some()
    );
    json!({"request": request, "prepared": actual})
}

fn write_actual_golden(name: &str, material: Value) {
    let Some(directory) = std::env::var_os("AG085_GOLDEN_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    assert!(
        directory
            == Path::new(
                "/Users/wrok/Desktop/RSIAgent/out/scratch/ag-085-impl/parent-phase/golden-parent"
            )
            || directory
                == Path::new(
                    "/Users/wrok/Desktop/RSIAgent/out/scratch/ag-085-impl/head-phase/golden-head"
                )
    );
    std::fs::create_dir_all(&directory).expect("create authorized own golden directory");
    let mut bytes = serde_json::to_vec_pretty(&material).expect("serialize actual golden material");
    bytes.push(b'\n');
    let path = directory.join(format!("{name}.json"));
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("preserve any previous golden without overwriting");
    output.write_all(&bytes).expect("save actual golden bytes");
    println!(
        "AG085_GOLDEN {name} bytes={} sha256={}",
        bytes.len(),
        hash(&bytes)
    );
}

#[test]
fn actual_ascii_sensitive_clean_warning_and_multiple_findings_golden() {
    let inputs = [
        "sk-abcdefghijklmnop",
        "/Users/abcdefghijklmnopqrstuv",
        "10.0.abcdefghijklmnop",
        ASCII_MULTI,
        "public development text",
        "TODO: secret review",
    ];
    let scans: Vec<Value> = inputs
        .iter()
        .map(|text| actual_scan_material(text))
        .collect();
    assert_eq!(scans[0]["report"]["status"], "blocked");
    assert_eq!(scans[1]["report"]["status"], "blocked");
    assert_eq!(scans[2]["report"]["status"], "blocked");
    assert_eq!(scans[3]["report"]["findings_count"], 7);
    assert_eq!(scans[4]["report"]["status"], "clean");
    assert_eq!(scans[5]["report"]["status"], "warning");
    write_actual_golden(
        "ascii",
        json!({"scans": scans, "clean_export": actual_export_material(inputs[4]),
        "warning_export": actual_export_material(inputs[5])}),
    );
}

#[test]
fn actual_unicode_clean_warning_and_original_identity_golden() {
    let clean = "é 中 🧪 café cafe\u{301}\r\n普通开发说明";
    let warning = "é中🧪 前置 TODO: secret 后置";
    let scans = [actual_scan_material(clean), actual_scan_material(warning)];
    assert_eq!(scans[0]["report"]["status"], "clean");
    assert_eq!(scans[1]["report"]["status"], "warning");
    write_actual_golden(
        "unicode",
        json!({"scans": scans, "clean_export": actual_export_material(clean),
        "warning_export": actual_export_material(warning)}),
    );
}
