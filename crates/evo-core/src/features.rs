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
}
