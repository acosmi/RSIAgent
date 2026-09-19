//! Integration test suite for E17: Development Proxy Evaluator Evolution Rejection Paths (V040).
use evo_core::Error;
use evo_core::features::{
    ScorerUpdateProposal, enable_agent_scorer_evolution, validate_e17_scorer_gates,
};

#[test]
fn test_v040_feature_flag_is_disabled() {
    assert!(enable_agent_scorer_evolution(true).is_err());
    let err = enable_agent_scorer_evolution(true).unwrap_err();
    let msg = format!("{:?}", err);
    assert!(msg.contains("E17 remains disabled: final grader is not candidate-writable"));

    // Default false is allowed
    assert!(enable_agent_scorer_evolution(false).is_ok());
}

#[test]
fn test_v040_candidate_cannot_modify_final_grader() {
    let final_graders = [
        "final_acceptance_grader",
        "acceptance_policy",
        "independent_evaluator",
    ];

    for grader in final_graders {
        let p = ScorerUpdateProposal {
            candidate_id: "candidate_x".into(),
            proposed_scorer_id: "scorer_modified".into(),
            target_grader_name: grader.into(),
            is_self_scoring: false,
            source_epoch: 1,
            target_epoch: 1,
            has_external_anchor: true,
        };
        let res = validate_e17_scorer_gates(&p);
        assert!(
            matches!(res, Err(Error::Invalid(msg)) if msg.contains("final acceptance grader is immutable"))
        );
    }
}

#[test]
fn test_v040_self_scoring_rejected() {
    let p = ScorerUpdateProposal {
        candidate_id: "candidate_x".into(),
        proposed_scorer_id: "scorer_self".into(),
        target_grader_name: "dev_proxy_scorer".into(),
        is_self_scoring: true, // Self-scoring!
        source_epoch: 1,
        target_epoch: 1,
        has_external_anchor: true,
    };
    let res = validate_e17_scorer_gates(&p);
    assert!(
        matches!(res, Err(Error::Invalid(msg)) if msg.contains("candidate cannot score itself"))
    );
}

#[test]
fn test_v040_cross_epoch_mixing_without_anchor_rejected() {
    let p = ScorerUpdateProposal {
        candidate_id: "candidate_x".into(),
        proposed_scorer_id: "scorer_epoch_jump".into(),
        target_grader_name: "dev_proxy_scorer".into(),
        is_self_scoring: false,
        source_epoch: 1,
        target_epoch: 2, // Different epoch!
        has_external_anchor: false,
    };
    let res = validate_e17_scorer_gates(&p);
    assert!(
        matches!(res, Err(Error::Conflict(msg)) if msg.contains("cross-epoch scores cannot be directly mixed"))
    );
}
