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
