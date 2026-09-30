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

// ---------------------------------------------------------------------------
// Controller F22 regressions. Relabelled grader names and structural
// self-scoring must be caught BY THE V040 GATE itself (defense in depth), not
// merely by the always-on "E17 remains disabled" flag that runs last.
// ---------------------------------------------------------------------------

fn f22_base() -> ScorerUpdateProposal {
    ScorerUpdateProposal {
        candidate_id: "cand_f22".into(),
        proposed_scorer_id: "scorer_f22".into(),
        target_grader_name: "dev_proxy_scorer".into(),
        is_self_scoring: false,
        source_epoch: 1,
        target_epoch: 1,
        has_external_anchor: true,
    }
}

/// Alternative casings, whitespace decoration, path spellings, `.rs` / `.v<n>`
/// suffixes, `:<label>` suffixes and embedded protected names.
const F22_RELABELLED_GRADERS: &[&str] = &[
    "Final_Acceptance_Grader",
    "FINAL_ACCEPTANCE_GRADER",
    " final_acceptance_grader ",
    "final_acceptance_grader.rs",
    "FINAL_ACCEPTANCE_GRADER.RS",
    "src/../final_acceptance_grader.rs",
    "src.final_acceptance_grader",
    "final_acceptance_grader_v2",
    "Acceptance_Policy",
    "acceptance_policy.v2",
    "acceptance_policy.v12.rs",
    "Independent_Evaluator",
    "independent_evaluator:main",
    "independent_evaluator.rs:v3",
];

#[test]
fn f22_relabelled_grader_names_rejected_by_immutable_grader_gate() {
    let mut missed = Vec::new();
    for grader in F22_RELABELLED_GRADERS {
        let mut p = f22_base();
        p.target_grader_name = (*grader).into();
        let res = validate_e17_scorer_gates(&p);
        let by_gate = matches!(&res, Err(Error::Invalid(m)) if m.contains("final acceptance grader is immutable"));
        if !by_gate {
            missed.push(format!("  input={:?} observed={:?}", grader, res));
        }
    }
    assert!(
        missed.is_empty(),
        "relabelled grader names NOT caught by V040 gate 1:\n{}",
        missed.join("\n")
    );
}

/// Structural self-scoring with `is_self_scoring` left false: any two of
/// candidate_id / proposed_scorer_id / target_grader_name coinciding, including
/// after case, `.rs`, `.v<n>` and `:<label>` normalisation.
fn f22_structural_self_scoring_cases() -> Vec<(&'static str, ScorerUpdateProposal)> {
    let mut a = f22_base();
    a.proposed_scorer_id = a.candidate_id.clone();
    let mut b = f22_base();
    b.target_grader_name = b.candidate_id.clone();
    let mut c = f22_base();
    c.proposed_scorer_id = c.candidate_id.clone();
    c.target_grader_name = c.candidate_id.clone();
    let mut d = f22_base();
    d.proposed_scorer_id = d.target_grader_name.clone();
    let mut e = f22_base();
    e.proposed_scorer_id = "CAND_F22".into();
    let mut f = f22_base();
    f.proposed_scorer_id = "cand_f22.v2".into();
    let mut g = f22_base();
    g.target_grader_name = "cand_f22.rs:main".into();
    let mut h = f22_base();
    h.proposed_scorer_id = "Dev_Proxy_Scorer.v7".into();
    vec![
        ("candidate_id == proposed_scorer_id", a),
        ("candidate_id == target_grader_name", b),
        (
            "candidate_id == proposed_scorer_id == target_grader_name",
            c,
        ),
        ("proposed_scorer_id == target_grader_name", d),
        ("proposed_scorer_id == candidate_id (case)", e),
        ("proposed_scorer_id == candidate_id (.v2)", f),
        ("target_grader_name == candidate_id (.rs:main)", g),
        ("proposed_scorer_id == target_grader_name (case + .v7)", h),
    ]
}

#[test]
fn f22_structural_self_scoring_rejected_by_self_scoring_gate() {
    let mut missed = Vec::new();
    for (label, p) in f22_structural_self_scoring_cases() {
        assert!(
            !p.is_self_scoring,
            "case={label}: probe must not self-declare"
        );
        let res = validate_e17_scorer_gates(&p);
        let by_gate =
            matches!(&res, Err(Error::Invalid(m)) if m.contains("candidate cannot score itself"));
        if !by_gate {
            missed.push(format!("  case={} observed={:?}", label, res));
        }
    }
    assert!(
        missed.is_empty(),
        "structural self-scoring NOT caught by V040 gate 2:\n{}",
        missed.join("\n")
    );
}

#[test]
fn f22_distinct_ids_pass_gates_but_e17_stays_disabled() {
    // Positive control: with pairwise-distinct, non-protected names the
    // hardened gates must not over-reject, and the proposal must still end at
    // the default-disabled flag.
    let res = validate_e17_scorer_gates(&f22_base());
    assert!(
        matches!(&res, Err(Error::Invalid(m)) if m.contains("E17 remains disabled")),
        "observed={:?}",
        res
    );
}
