//! Optional extensions stay disabled until a revised plan is reviewed.
//! Pure rejection gates for E17 (evaluator evolution) and E18 (automatic code PRs).
use crate::{Error, Result, identifier};
use serde::{Deserialize, Serialize};

pub fn enable_agent_scorer_evolution(on: bool) -> Result<()> {
    if on {
        return Err(Error::Invalid(
            "E17 remains disabled: final grader is not candidate-writable".into(),
        ));
    }
    Ok(())
}

pub fn enable_automatic_code_prs(on: bool) -> Result<()> {
    if on {
        return Err(Error::Invalid(
            "E18 remains disabled: no isolation runner and no human merge authority in-process"
                .into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScorerUpdateProposal {
    pub candidate_id: String,
    pub proposed_scorer_id: String,
    pub target_grader_name: String,
    pub is_self_scoring: bool,
    pub source_epoch: u64,
    pub target_epoch: u64,
    pub has_external_anchor: bool,
}

/// V040: Evaluator evolution rejection gates.
/// Rejects any attempt by a candidate to modify the final grader, score itself,
/// or directly average scores across different epochs without external anchoring.
pub fn validate_e17_scorer_gates(proposal: &ScorerUpdateProposal) -> Result<()> {
    identifier(&proposal.candidate_id)?;
    identifier(&proposal.proposed_scorer_id)?;
    identifier(&proposal.target_grader_name)?;

    // 1. Candidate attempting to modify final acceptance grader
    if proposal.target_grader_name == "final_acceptance_grader"
        || proposal.target_grader_name == "acceptance_policy"
        || proposal.target_grader_name == "independent_evaluator"
    {
        return Err(Error::Invalid(
            "V040 rejected: final acceptance grader is immutable and cannot be modified by candidates".into(),
        ));
    }

    // 2. Self-scoring or self-approval is forbidden
    if proposal.is_self_scoring {
        return Err(Error::Invalid(
            "V040 rejected: candidate cannot score itself or self-approve scorer updates".into(),
        ));
    }

    // 3. Scores across different epochs cannot be directly mixed/averaged without external anchor
    if proposal.source_epoch != proposal.target_epoch && !proposal.has_external_anchor {
        return Err(Error::Conflict(
            "V040 rejected: cross-epoch scores cannot be directly mixed without external anchor re-evaluation".into(),
        ));
    }

    // 4. Feature flag check: E17 is disabled
    enable_agent_scorer_evolution(true)?;

    Ok(())
}

pub const MAX_CODE_PATCH_DIFF_BYTES: usize = 500_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodePatchProposal {
    pub candidate_id: String,
    pub target_repo: String,
    pub modified_files: Vec<String>,
    pub has_dependency_changes: bool,
    pub has_build_script_changes: bool,
    pub auto_merge: bool,
    pub auto_deploy: bool,
    pub isolated_sandbox: bool,
    pub human_approval_token: Option<String>,
    pub diff_bytes: usize,
}

pub fn is_protected_code_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let normalized = lower.replace('\\', "/");

    // Path traversal, absolute paths, sockets, system directories
    if normalized.contains("..")
        || normalized.starts_with('/')
        || normalized.contains("docker.sock")
        || normalized.contains("/etc/")
        || normalized.contains("/var/")
    {
        return true;
    }

    let file_name = normalized.rsplit('/').next().unwrap_or(&normalized);

    // Build, lock & packaging files
    if file_name == "cargo.toml"
        || file_name == "cargo.lock"
        || file_name == "build.rs"
        || file_name == "makefile"
        || file_name == "dockerfile"
    {
        return true;
    }

    // Protected categories: acceptance, grader, evaluator, scoring
    if normalized.contains("evaluat")
        || normalized.contains("acceptance")
        || normalized.contains("grader")
        || normalized.contains("scorer")
        || normalized.contains("scoring")
        || normalized.contains("formal_eval")
    {
        return true;
    }

    // Protected categories: security, approval, tokens, secrets, credentials, sandbox
    if normalized.contains("approval")
        || normalized.contains("security")
        || normalized.contains("secret")
        || normalized.contains("token")
        || normalized.contains("credential")
        || normalized.contains("sandbox")
        || normalized.contains("isolation")
    {
        return true;
    }

    // Protected categories: budget, billing, ledger
    if normalized.contains("budget")
        || normalized.contains("billing")
        || normalized.contains("ledger")
    {
        return true;
    }

    false
}

/// V041: Automatic code PR rejection gates.
/// Rejects attempts to modify acceptance, approval, security, budget, build files,
/// or docker sockets; forbids auto-merge, auto-deploy, un-sandboxed execution,
/// missing human approval, excessive diff sizes, and un-audited dependency/build changes.
/// Without explicit feature enablement, automatic code PRs remain strictly disabled.
pub fn validate_e18_code_pr_gates(proposal: &CodePatchProposal) -> Result<()> {
    identifier(&proposal.candidate_id)?;
    if proposal.target_repo.trim().is_empty() {
        return Err(Error::Invalid("target_repo cannot be empty".into()));
    }

    // 1. Modifying protected files is forbidden
    for file in &proposal.modified_files {
        if is_protected_code_path(file) {
            return Err(Error::Invalid(format!(
                "V041 rejected: cannot modify protected acceptance, approval, security, budget, or core file '{}'",
                file
            )));
        }
    }

    // 2. Dependency changes require separate security audit
    if proposal.has_dependency_changes {
        return Err(Error::Invalid(
            "V041 rejected: dependency modifications require separate human security audit".into(),
        ));
    }

    // 3. Build script changes require separate security audit
    if proposal.has_build_script_changes {
        return Err(Error::Invalid(
            "V041 rejected: build script modifications require separate human security audit"
                .into(),
        ));
    }

    // 4. Automatic merge is strictly forbidden
    if proposal.auto_merge {
        return Err(Error::Invalid(
            "V041 rejected: automatic merge is strictly forbidden; human review required".into(),
        ));
    }

    // 5. Automatic deploy / core process replacement is strictly forbidden
    if proposal.auto_deploy {
        return Err(Error::Invalid(
            "V041 rejected: automatic deploy or core process replacement is strictly forbidden"
                .into(),
        ));
    }

    // 6. Must run in isolated disposable sandbox
    if !proposal.isolated_sandbox {
        return Err(Error::Invalid(
            "V041 rejected: patch build and test must run in isolated disposable sandbox".into(),
        ));
    }

    // 7. Human approval token is mandatory
    if proposal.human_approval_token.is_none() {
        return Err(Error::Invalid(
            "V041 rejected: code PR creation requires human review approval token".into(),
        ));
    }

    // 8. Diff size limit check
    if proposal.diff_bytes > MAX_CODE_PATCH_DIFF_BYTES {
        return Err(Error::Invalid(format!(
            "V041 rejected: patch diff size ({} bytes) exceeds maximum allowable limit ({} bytes)",
            proposal.diff_bytes, MAX_CODE_PATCH_DIFF_BYTES
        )));
    }

    // 9. Feature flag check: E18 remains strictly disabled
    enable_automatic_code_prs(true)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_extensions_cannot_be_flipped_on() {
        assert!(enable_agent_scorer_evolution(true).is_err());
        assert!(enable_automatic_code_prs(true).is_err());
        enable_agent_scorer_evolution(false).unwrap();
        enable_automatic_code_prs(false).unwrap();
    }

    #[test]
    fn v040_scorer_rejection_gates() {
        let base = ScorerUpdateProposal {
            candidate_id: "cand_01".into(),
            proposed_scorer_id: "scorer_dev_v2".into(),
            target_grader_name: "dev_proxy_scorer".into(),
            is_self_scoring: false,
            source_epoch: 1,
            target_epoch: 1,
            has_external_anchor: false,
        };

        // Self-scoring rejected
        let mut p_self = base.clone();
        p_self.is_self_scoring = true;
        assert!(validate_e17_scorer_gates(&p_self).is_err());

        // Modifying final acceptance grader rejected
        let mut p_final = base.clone();
        p_final.target_grader_name = "final_acceptance_grader".into();
        assert!(validate_e17_scorer_gates(&p_final).is_err());

        // Cross epoch mixing without external anchor rejected
        let mut p_epoch = base.clone();
        p_epoch.target_epoch = 2;
        assert!(validate_e17_scorer_gates(&p_epoch).is_err());

        // Feature flag itself is disabled
        let mut p_valid = base;
        p_valid.has_external_anchor = true;
        assert!(validate_e17_scorer_gates(&p_valid).is_err());
    }

    #[test]
    fn v041_code_pr_rejection_gates() {
        let valid_base = CodePatchProposal {
            candidate_id: "patch_cand_01".into(),
            target_repo: "acosmi/RSIAgent".into(),
            modified_files: vec!["crates/evo-engine/src/worker.rs".into()],
            has_dependency_changes: false,
            has_build_script_changes: false,
            auto_merge: false,
            auto_deploy: false,
            isolated_sandbox: true,
            human_approval_token: Some("human_token_valid_01".into()),
            diff_bytes: 1024,
        };

        // Protected files: acceptance / grader rejected
        let mut p_acceptance = valid_base.clone();
        p_acceptance.modified_files = vec!["crates/evo-engine/src/evaluator.rs".into()];
        assert!(validate_e18_code_pr_gates(&p_acceptance).is_err());

        // Protected files: Cargo.toml rejected
        let mut p_cargo = valid_base.clone();
        p_cargo.modified_files = vec!["Cargo.toml".into()];
        assert!(validate_e18_code_pr_gates(&p_cargo).is_err());

        // Protected files: budget / security rejected
        let mut p_budget = valid_base.clone();
        p_budget.modified_files = vec!["crates/evo-storage/src/budget.rs".into()];
        assert!(validate_e18_code_pr_gates(&p_budget).is_err());

        // Path traversal / socket rejected
        let mut p_traversal = valid_base.clone();
        p_traversal.modified_files = vec!["../etc/passwd".into()];
        assert!(validate_e18_code_pr_gates(&p_traversal).is_err());

        // Dependency modifications rejected
        let mut p_dep = valid_base.clone();
        p_dep.has_dependency_changes = true;
        assert!(validate_e18_code_pr_gates(&p_dep).is_err());

        // Build script modifications rejected
        let mut p_build = valid_base.clone();
        p_build.has_build_script_changes = true;
        assert!(validate_e18_code_pr_gates(&p_build).is_err());

        // Auto merge rejected
        let mut p_merge = valid_base.clone();
        p_merge.auto_merge = true;
        assert!(validate_e18_code_pr_gates(&p_merge).is_err());

        // Auto deploy rejected
        let mut p_deploy = valid_base.clone();
        p_deploy.auto_deploy = true;
        assert!(validate_e18_code_pr_gates(&p_deploy).is_err());

        // Non-isolated sandbox rejected
        let mut p_sandbox = valid_base.clone();
        p_sandbox.isolated_sandbox = false;
        assert!(validate_e18_code_pr_gates(&p_sandbox).is_err());

        // Missing human approval token rejected
        let mut p_approval = valid_base.clone();
        p_approval.human_approval_token = None;
        assert!(validate_e18_code_pr_gates(&p_approval).is_err());

        // Diff limit exceeded rejected
        let mut p_diff = valid_base.clone();
        p_diff.diff_bytes = MAX_CODE_PATCH_DIFF_BYTES + 1;
        assert!(validate_e18_code_pr_gates(&p_diff).is_err());

        // Valid base proposal is STILL rejected because E18 feature flag is disabled
        assert!(validate_e18_code_pr_gates(&valid_base).is_err());
    }
}
