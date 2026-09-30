//! Integration test suite for E18: Automatic Code PR Rejection Paths (V041).
use evo_core::Error;
use evo_core::features::{
    CodePatchProposal, MAX_CODE_PATCH_DIFF_BYTES, enable_automatic_code_prs,
    is_protected_code_path, validate_e18_code_pr_gates,
};

#[test]
fn test_e18_feature_flag_disabled_by_default() {
    assert!(enable_automatic_code_prs(true).is_err());
    assert!(enable_automatic_code_prs(false).is_ok());
}

#[test]
fn test_e18_protected_paths_classification() {
    // Build and packaging files
    assert!(is_protected_code_path("Cargo.toml"));
    assert!(is_protected_code_path("crates/evo-core/Cargo.toml"));
    assert!(is_protected_code_path("Cargo.lock"));
    assert!(is_protected_code_path("build.rs"));
    assert!(is_protected_code_path("Makefile"));
    assert!(is_protected_code_path("Dockerfile"));

    // Path traversal and absolute system paths
    assert!(is_protected_code_path("../secret.txt"));
    assert!(is_protected_code_path("/etc/passwd"));
    assert!(is_protected_code_path("/var/run/docker.sock"));

    // Acceptance, grader, evaluator
    assert!(is_protected_code_path("crates/evo-engine/src/evaluator.rs"));
    assert!(is_protected_code_path("crates/evo-core/src/evaluation.rs"));
    assert!(is_protected_code_path("tests/acceptance_gate.rs"));
    assert!(is_protected_code_path("src/grader_policy.rs"));
    assert!(is_protected_code_path("src/scorer.rs"));
    assert!(is_protected_code_path("src/formal_eval.rs"));

    // Security, approval, budget, tokens
    assert!(is_protected_code_path("src/approval_token.rs"));
    assert!(is_protected_code_path("src/security_gate.rs"));
    assert!(is_protected_code_path("src/budget_broker.rs"));
    assert!(is_protected_code_path("src/billing.rs"));
    assert!(is_protected_code_path("docs/implementation-ledger.md"));

    // Safe regular code path
    assert!(!is_protected_code_path("crates/evo-engine/src/worker.rs"));
    assert!(!is_protected_code_path("crates/evo-storage/src/helpers.rs"));
}

#[test]
fn test_e18_reject_modifying_acceptance_or_evaluator_files() {
    let proposal = CodePatchProposal {
        candidate_id: "cand_patch_01".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec![
            "crates/evo-engine/src/worker.rs".into(),
            "crates/evo-engine/src/evaluator.rs".into(),
        ],
        has_dependency_changes: false,
        has_build_script_changes: false,
        auto_merge: false,
        auto_deploy: false,
        isolated_sandbox: true,
        human_approval_token: Some("token_123".into()),
        diff_bytes: 500,
    };

    let err = validate_e18_code_pr_gates(&proposal).unwrap_err();
    assert!(err.to_string().contains("V041 rejected"));
    assert!(err.to_string().contains("evaluator.rs"));
}

#[test]
fn test_e18_reject_modifying_cargo_or_build_files() {
    let proposal = CodePatchProposal {
        candidate_id: "cand_patch_02".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec!["Cargo.toml".into()],
        has_dependency_changes: false,
        has_build_script_changes: false,
        auto_merge: false,
        auto_deploy: false,
        isolated_sandbox: true,
        human_approval_token: Some("token_123".into()),
        diff_bytes: 500,
    };

    let err = validate_e18_code_pr_gates(&proposal).unwrap_err();
    assert!(err.to_string().contains("V041 rejected"));
    assert!(err.to_string().contains("Cargo.toml"));
}

#[test]
fn test_e18_reject_modifying_security_or_budget_files() {
    let proposal = CodePatchProposal {
        candidate_id: "cand_patch_03".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec!["crates/evo-storage/src/budget.rs".into()],
        has_dependency_changes: false,
        has_build_script_changes: false,
        auto_merge: false,
        auto_deploy: false,
        isolated_sandbox: true,
        human_approval_token: Some("token_123".into()),
        diff_bytes: 500,
    };

    let err = validate_e18_code_pr_gates(&proposal).unwrap_err();
    assert!(err.to_string().contains("V041 rejected"));
    assert!(err.to_string().contains("budget.rs"));
}

#[test]
fn test_e18_reject_path_traversal_or_system_paths() {
    let proposal = CodePatchProposal {
        candidate_id: "cand_patch_04".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec!["../../etc/shadow".into()],
        has_dependency_changes: false,
        has_build_script_changes: false,
        auto_merge: false,
        auto_deploy: false,
        isolated_sandbox: true,
        human_approval_token: Some("token_123".into()),
        diff_bytes: 500,
    };

    let err = validate_e18_code_pr_gates(&proposal).unwrap_err();
    assert!(err.to_string().contains("V041 rejected"));
}

#[test]
fn test_e18_reject_dependency_or_build_script_modifications() {
    let base = CodePatchProposal {
        candidate_id: "cand_patch_05".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec!["crates/evo-engine/src/worker.rs".into()],
        has_dependency_changes: true,
        has_build_script_changes: false,
        auto_merge: false,
        auto_deploy: false,
        isolated_sandbox: true,
        human_approval_token: Some("token_123".into()),
        diff_bytes: 500,
    };

    let err = validate_e18_code_pr_gates(&base).unwrap_err();
    assert!(
        err.to_string()
            .contains("dependency modifications require separate human security audit")
    );

    let mut base_build = base;
    base_build.has_dependency_changes = false;
    base_build.has_build_script_changes = true;
    let err2 = validate_e18_code_pr_gates(&base_build).unwrap_err();
    assert!(
        err2.to_string()
            .contains("build script modifications require separate human security audit")
    );
}

#[test]
fn test_e18_reject_auto_merge_and_auto_deploy() {
    let mut proposal = CodePatchProposal {
        candidate_id: "cand_patch_06".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec!["crates/evo-engine/src/worker.rs".into()],
        has_dependency_changes: false,
        has_build_script_changes: false,
        auto_merge: true,
        auto_deploy: false,
        isolated_sandbox: true,
        human_approval_token: Some("token_123".into()),
        diff_bytes: 500,
    };

    let err = validate_e18_code_pr_gates(&proposal).unwrap_err();
    assert!(
        err.to_string()
            .contains("automatic merge is strictly forbidden")
    );

    proposal.auto_merge = false;
    proposal.auto_deploy = true;
    let err2 = validate_e18_code_pr_gates(&proposal).unwrap_err();
    assert!(
        err2.to_string()
            .contains("automatic deploy or core process replacement is strictly forbidden")
    );
}

#[test]
fn test_e18_reject_unisolated_sandbox() {
    let proposal = CodePatchProposal {
        candidate_id: "cand_patch_07".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec!["crates/evo-engine/src/worker.rs".into()],
        has_dependency_changes: false,
        has_build_script_changes: false,
        auto_merge: false,
        auto_deploy: false,
        isolated_sandbox: false,
        human_approval_token: Some("token_123".into()),
        diff_bytes: 500,
    };

    let err = validate_e18_code_pr_gates(&proposal).unwrap_err();
    assert!(
        err.to_string()
            .contains("must run in isolated disposable sandbox")
    );
}

#[test]
fn test_e18_reject_missing_human_approval() {
    let proposal = CodePatchProposal {
        candidate_id: "cand_patch_08".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec!["crates/evo-engine/src/worker.rs".into()],
        has_dependency_changes: false,
        has_build_script_changes: false,
        auto_merge: false,
        auto_deploy: false,
        isolated_sandbox: true,
        human_approval_token: None,
        diff_bytes: 500,
    };

    let err = validate_e18_code_pr_gates(&proposal).unwrap_err();
    assert!(
        err.to_string()
            .contains("requires human review approval token")
    );
}

#[test]
fn test_e18_reject_diff_size_exceeded() {
    let proposal = CodePatchProposal {
        candidate_id: "cand_patch_09".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec!["crates/evo-engine/src/worker.rs".into()],
        has_dependency_changes: false,
        has_build_script_changes: false,
        auto_merge: false,
        auto_deploy: false,
        isolated_sandbox: true,
        human_approval_token: Some("token_123".into()),
        diff_bytes: MAX_CODE_PATCH_DIFF_BYTES + 10,
    };

    let err = validate_e18_code_pr_gates(&proposal).unwrap_err();
    assert!(err.to_string().contains("patch diff size"));
    assert!(err.to_string().contains("exceeds maximum allowable limit"));
}

#[test]
fn test_e18_valid_proposal_still_rejected_by_default_disabled_extension() {
    let proposal = CodePatchProposal {
        candidate_id: "cand_patch_10".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec!["crates/evo-engine/src/worker.rs".into()],
        has_dependency_changes: false,
        has_build_script_changes: false,
        auto_merge: false,
        auto_deploy: false,
        isolated_sandbox: true,
        human_approval_token: Some("token_123".into()),
        diff_bytes: 1024,
    };

    // Passes all structural gates, but fails because E18 extension is strictly disabled
    let err = validate_e18_code_pr_gates(&proposal).unwrap_err();
    assert!(err.to_string().contains("E18 remains disabled"));
}

// ---------------------------------------------------------------------------
// Controller F23 regressions. Protected path families, blank approval tokens
// and empty file lists must be caught BY THE V041 GATE itself (defense in
// depth), not merely by the always-on "E18 remains disabled" flag that runs last.
// ---------------------------------------------------------------------------

fn f23_base() -> CodePatchProposal {
    CodePatchProposal {
        candidate_id: "cand_f23".into(),
        target_repo: "acosmi/RSIAgent".into(),
        modified_files: vec!["crates/evo-engine/src/worker.rs".into()],
        has_dependency_changes: false,
        has_build_script_changes: false,
        auto_merge: false,
        auto_deploy: false,
        isolated_sandbox: true,
        human_approval_token: Some("human_token_f23".into()),
        diff_bytes: 1024,
    }
}

/// Container, CI, toolchain and build-configuration families; whitespace /
/// control-character decorated names; trailing-slash and empty paths; renamed
/// build scripts.
const F23_PROTECTED_PATH_FAMILIES: &[&str] = &[
    "Dockerfile.dev",
    "Dockerfile.prod",
    "dev.Dockerfile",
    "docker-compose.yml",
    "deploy/docker-compose.override.yaml",
    ".cargo/config.toml",
    "crates/x/.cargo/config",
    "rust-toolchain.toml",
    "rust-toolchain",
    ".github/workflows/ci.yml",
    ".github/CODEOWNERS",
    "Makefile.in",
    "makefile.am",
    "Cargo.toml ",
    " Cargo.toml",
    "Cargo.toml\n",
    "Cargo.toml\t",
    "Cargo\0.toml",
    "Cargo.toml/",
    "crates/evo-core/",
    "",
    "build.rs.disabled",
    "crates/x/build.rs.bak",
];

#[test]
fn f23_protected_path_families_classified_as_protected() {
    let accepted: Vec<&str> = F23_PROTECTED_PATH_FAMILIES
        .iter()
        .copied()
        .filter(|p| !is_protected_code_path(p))
        .collect();
    assert!(
        accepted.is_empty(),
        "paths NOT classified as protected by is_protected_code_path: {:?}",
        accepted
    );
}

#[test]
fn f23_ordinary_paths_not_protected() {
    for path in [
        "crates/evo-core/src/lib.rs",
        "docs/guide.md",
        "src/docker_client.rs",
    ] {
        assert!(
            !is_protected_code_path(path),
            "ordinary path wrongly classified as protected: {:?}",
            path
        );
    }
}

#[test]
fn f23_protected_path_families_rejected_by_protected_file_gate() {
    let mut missed = Vec::new();
    for path in F23_PROTECTED_PATH_FAMILIES {
        let mut p = f23_base();
        p.modified_files = vec!["crates/evo-engine/src/worker.rs".into(), (*path).into()];
        let res = validate_e18_code_pr_gates(&p);
        let by_gate =
            matches!(&res, Err(Error::Invalid(m)) if m.contains("cannot modify protected"));
        if !by_gate {
            missed.push(format!("  path={:?} observed={:?}", path, res));
        }
    }
    assert!(
        missed.is_empty(),
        "protected path families NOT caught by V041 gate 1:\n{}",
        missed.join("\n")
    );
}

#[test]
fn f23_blank_approval_token_rejected_by_approval_gate() {
    let mut missed = Vec::new();
    for token in ["", " ", "   ", "\t\n ", "\u{a0}"] {
        let mut p = f23_base();
        p.human_approval_token = Some(token.into());
        let res = validate_e18_code_pr_gates(&p);
        let by_gate = matches!(&res, Err(Error::Invalid(m)) if m.contains("requires human review approval token"));
        if !by_gate {
            missed.push(format!("  token={:?} observed={:?}", token, res));
        }
    }
    assert!(
        missed.is_empty(),
        "blank approval tokens NOT caught by V041 gate 7:\n{}",
        missed.join("\n")
    );
}

#[test]
fn f23_empty_modified_files_rejected_by_file_list_gate() {
    let mut p = f23_base();
    p.modified_files = vec![];
    let res = validate_e18_code_pr_gates(&p);
    assert!(
        matches!(&res, Err(Error::Invalid(m)) if m.contains("a code PR proposal must name at least one modified file")),
        "observed={:?}",
        res
    );
}

#[test]
fn f23_ordinary_proposal_passes_gates_but_e18_stays_disabled() {
    // Positive control: the hardened gates must not over-reject an ordinary
    // proposal, which must still end at the default-disabled flag.
    let res = validate_e18_code_pr_gates(&f23_base());
    assert!(
        matches!(&res, Err(Error::Invalid(m)) if m.contains("E18 remains disabled")),
        "observed={:?}",
        res
    );
}
