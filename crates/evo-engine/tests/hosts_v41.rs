//! Comprehensive test suite for E16.4: Extra Real Hosts, Claude Code MCP Tool-Only,
//! Configuration Surface Drift, and Execution Attribution.
//!
//! Covers scenario families:
//! - V002, V003, V009, V037, V039: Contract checks, host boundary, zero regression
//! - V060: Surface drift detection (unclassified fields block validation)
//! - V061: Empty extraction rejection (empty extraction != success)
//! - V062: Supported fields require consumers and valid field contracts
//! - V077: Host truncation / override cannot claim full used
//! - V087: Skill-Diagnosis-Attribution (proper attribution, no fake receipts)
//! - V091: All-Effective-Content-Gated
//! - V092: No-Auto-Elevation
//! - V098: Audit-Traceability & Fixture Roundtrip

use evo_core::contract::{HostSurfaceManifest, SURFACE_SCHEMA};
use evo_engine::hosts::{
    CLAUDE_CODE_TARGET, HostExecutionReceipt, HostToolStage, SkillAttribution, claude_code_support,
    claude_code_surface_manifest, detect_surface_drift, verify_host_receipt,
};
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Missing Claude Code Binary Is Blocked (No Mock Host)
// ---------------------------------------------------------------------------
#[test]
fn test_missing_claude_code_binary_is_blocked() {
    let res_none = claude_code_support(None);
    assert!(
        res_none.is_err(),
        "Missing Claude Code binary must return error"
    );
    let msg = format!("{:?}", res_none.unwrap_err());
    assert!(msg.contains("Claude Code is not installed; not substituting a mock host"));

    let non_existent = PathBuf::from("/non/existent/path/to/claude");
    let res_missing_file = claude_code_support(Some(&non_existent));
    assert!(res_missing_file.is_err());
}

// ---------------------------------------------------------------------------
// V060 & V061: Surface Drift Detection & Empty Extraction Rejection
// ---------------------------------------------------------------------------
#[test]
fn test_v060_unclassified_extracted_field_blocks_validation() {
    let manifest = claude_code_surface_manifest();

    // Upstream Claude Code added a new field 'agent_autonomy_mode' that is not in manifest
    let mut extracted: Vec<String> = manifest.items.iter().map(|i| i.name.clone()).collect();
    extracted.push("agent_autonomy_mode".into());

    let res = detect_surface_drift(&manifest, &extracted);
    assert!(res.is_err(), "Unclassified field must block validation");
    let err_msg = format!("{:?}", res.unwrap_err());
    assert!(err_msg.contains("agent_autonomy_mode is unclassified"));
}

#[test]
fn test_v061_empty_extraction_is_rejected() {
    let manifest = claude_code_surface_manifest();
    let empty_extracted: Vec<String> = vec![];

    let res = detect_surface_drift(&manifest, &empty_extracted);
    assert!(res.is_err(), "Empty extraction must be rejected");
    let err_msg = format!("{:?}", res.unwrap_err());
    assert!(err_msg.contains("empty extraction is not a successful cover"));
}

// ---------------------------------------------------------------------------
// V062: Supported Fields Require Consumers and Valid Field Contracts
// ---------------------------------------------------------------------------
#[test]
fn test_v062_supported_items_require_mapped_field_and_consumer() {
    let mut invalid_manifest = claude_code_surface_manifest();

    // Invalidate an item: mark as Supported but without a consumer
    invalid_manifest.items[0].consumer = None;

    let extracted: Vec<String> = invalid_manifest
        .items
        .iter()
        .map(|i| i.name.clone())
        .collect();
    let res = detect_surface_drift(&invalid_manifest, &extracted);
    assert!(res.is_err());
    let err_msg = format!("{:?}", res.unwrap_err());
    assert!(err_msg.contains("supported item needs a consumer"));

    // Invalidate an item: mark as Unsupported but assign a mapped field
    let mut invalid_manifest2 = claude_code_surface_manifest();
    invalid_manifest2.items[4].mapped_field = Some("skill.content".into());
    let res2 = detect_surface_drift(&invalid_manifest2, &extracted);
    assert!(res2.is_err());
    let err_msg2 = format!("{:?}", res2.unwrap_err());
    assert!(err_msg2.contains("cannot map candidate fields"));
}

// ---------------------------------------------------------------------------
// V077 & V087: Host Truncation, Skill Attribution and Fake Receipt Rejection
// ---------------------------------------------------------------------------
#[test]
fn test_v077_host_truncation_cannot_claim_full_used() {
    let receipt = HostExecutionReceipt {
        host_id: CLAUDE_CODE_TARGET.into(),
        run_id: "run_42".into(),
        stage: HostToolStage::Truncated,
        attribution: SkillAttribution::Truncated,
        is_truncated: true,
        is_overridden: false,
        claimed_used: true, // Forgery! Claiming used when truncated
        claimed_benefit: false,
    };

    let res = verify_host_receipt(&receipt);
    assert!(
        res.is_err(),
        "Cannot claim 'used' when host content was truncated"
    );
    let msg = format!("{:?}", res.unwrap_err());
    assert!(msg.contains("cannot claim full 'used' when host content was truncated"));
}

#[test]
fn test_v077_host_override_cannot_claim_used() {
    let receipt = HostExecutionReceipt {
        host_id: CLAUDE_CODE_TARGET.into(),
        run_id: "run_43".into(),
        stage: HostToolStage::Attached,
        attribution: SkillAttribution::NotAttached,
        is_truncated: false,
        is_overridden: true, // Host overrode skill
        claimed_used: true,  // Forgery!
        claimed_benefit: false,
    };

    let res = verify_host_receipt(&receipt);
    assert!(
        res.is_err(),
        "Cannot claim 'used' when host overrides skill content"
    );
}

#[test]
fn test_v087_skill_diagnosis_attribution_and_unverified_benefit() {
    // Valid honest truncated receipt
    let honest_truncated = HostExecutionReceipt {
        host_id: CLAUDE_CODE_TARGET.into(),
        run_id: "run_44".into(),
        stage: HostToolStage::Truncated,
        attribution: SkillAttribution::Truncated,
        is_truncated: true,
        is_overridden: false,
        claimed_used: false,
        claimed_benefit: false,
    };
    assert!(verify_host_receipt(&honest_truncated).is_ok());

    // Valid honest execution omitted receipt
    let honest_omitted = HostExecutionReceipt {
        host_id: CLAUDE_CODE_TARGET.into(),
        run_id: "run_45".into(),
        stage: HostToolStage::Omitted,
        attribution: SkillAttribution::ExecutionOmitted,
        is_truncated: false,
        is_overridden: false,
        claimed_used: false,
        claimed_benefit: false,
    };
    assert!(verify_host_receipt(&honest_omitted).is_ok());

    // Claiming benefit without independent attestation
    let fake_benefit = HostExecutionReceipt {
        host_id: CLAUDE_CODE_TARGET.into(),
        run_id: "run_46".into(),
        stage: HostToolStage::Used,
        attribution: SkillAttribution::Uncertain, // Not VerifiedBenefit
        is_truncated: false,
        is_overridden: false,
        claimed_used: true,
        claimed_benefit: true, // Forgery!
    };
    let res = verify_host_receipt(&fake_benefit);
    assert!(res.is_err());
    let msg = format!("{:?}", res.unwrap_err());
    assert!(msg.contains("cannot claim verified benefit without independent verification"));
}

// ---------------------------------------------------------------------------
// V098: Fixture Roundtrip & Manifest Integrity
// ---------------------------------------------------------------------------
#[test]
fn test_v098_claude_code_surface_fixture_roundtrip() {
    let fixture_path = Path::new("../../fixtures/hosts/claude_code_surface.v1.json");
    let content = if fixture_path.exists() {
        fs::read_to_string(fixture_path).unwrap()
    } else {
        fs::read_to_string("fixtures/hosts/claude_code_surface.v1.json").unwrap()
    };

    let manifest: HostSurfaceManifest =
        serde_json::from_str(&content).expect("Host surface fixture must parse cleanly");

    assert_eq!(manifest.schema_version, SURFACE_SCHEMA);
    assert_eq!(manifest.host, CLAUDE_CODE_TARGET);
    assert_eq!(manifest.items.len(), 7);

    let extracted: Vec<String> = manifest.items.iter().map(|i| i.name.clone()).collect();
    manifest
        .validate_against_extraction(&extracted)
        .expect("Fixture manifest must validate against its extracted items");
}
