//! Structural fixture for the closed loop shape.
//!
//! This module does not prove a real model call, independent evaluation,
//! approval, application or benefit. Production entry points live in
//! `service`; this fixture cannot promote a release.
use crate::evaluator::{Evaluator, ExecutionReport, GradeRequest};
use crate::evidence::ingest_trusted_runs;
use crate::releases::Release;
use evo_core::contract::{
    AppliedReceipt, CapabilityLevel, CompileParts, HostCapabilities, ImproverPatch, Profile,
    SkillPatch, SkillSnapshot, compile_bundle,
};
use evo_core::evaluation::{ClusterObservation, ExperimentPlan, QueryBook, QueryTicket, Verdict};
use evo_core::evidence::{ModelEvidenceRequest, Purpose, RouteClass, SourceSelection, route};
use evo_core::{Context, Error, Result, Role, Strategy};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopStep {
    FailedRun,
    Aggregate,
    Route,
    Compile,
    Evaluate,
    Approve,
    Apply,
    Revoke,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopTrace {
    pub steps: Vec<LoopStep>,
    pub route_class: RouteClass,
    pub verdict: Option<Verdict>,
    pub used_real_model: bool,
    pub auto_promote: bool,
}

pub fn require_real_model(credentials_present: bool) -> Result<()> {
    let _ = credentials_present;
    Err(Error::Invalid(
        "credential presence does not prove a configured real model transport".into(),
    ))
}

pub fn mock_cannot_prove_model() -> bool {
    true
}

pub fn run_structural_loop(
    verdict: Verdict,
    credentials_present: bool,
) -> Result<(LoopTrace, Release, AppliedReceipt)> {
    let host = Context::new("n", "host", Role::Host)?;
    let eval = Context::new("n", "eval", Role::Evaluator)?;
    let sel = SourceSelection {
        roots: vec!["/authorized".into()],
        run_ids: vec!["run1".into(), "run2".into()],
        purpose: Purpose::Generation,
        allow_model_excerpts: false,
    };
    let set = ingest_trusted_runs(
        &sel,
        &[("run1", b"fail config"), ("run2", b"fail config retry")],
    )?;
    let req = ModelEvidenceRequest::from_set(&set, vec!["fail config".into()])?;
    if req.source_ids.len() < 2 {
        return Err(Error::Invalid("need two sources".into()));
    }
    let routing = route(RouteClass::Procedural, false);
    let profile = Profile {
        id: "p1".into(),
        evolution_enabled: true,
        parent_digest: "parent1".into(),
        baseline_digest: "base1".into(),
    };
    let parent = SkillSnapshot::empty();
    let baseline = SkillSnapshot::empty();
    let parent_strategy = Strategy::default();
    let baseline_strategy = Strategy::default();
    let skill_patch = SkillPatch::default();
    let improver_patch = ImproverPatch::default();
    let caps = HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    };
    let revoked = BTreeSet::new();
    let bundle = compile_bundle(CompileParts {
        profile: &profile,
        parent: &parent,
        baseline: &baseline,
        parent_strategy: &parent_strategy,
        baseline_strategy: &baseline_strategy,
        skill_patch: &skill_patch,
        improver_patch: &improver_patch,
        caps: &caps,
        revoked: &revoked,
    })?;
    let mut plan = ExperimentPlan::first_low_risk("mvc")?;
    plan.freeze(1)?;
    plan.bind_candidate("cand1")?;
    let mut book = QueryBook::open(&plan)?;
    let ticket = QueryTicket {
        id: "t1".into(),
        plan_digest: plan.digest()?,
        candidate_digest: "cand1".into(),
        dataset_epoch: plan.dataset_epoch.clone(),
        seed: "s1".into(),
    };
    Evaluator::start(&eval, &mut plan, "cand1", ticket, &mut book)?;
    let exec = ExecutionReport {
        ticket_id: "t1".into(),
        environment_digest: "env".into(),
        outputs_complete: true,
        units: 1,
    };
    let d = match verdict {
        Verdict::Improved => 1.0,
        Verdict::Regressed => -1.0,
        _ => 0.0,
    };
    let rows: Vec<ClusterObservation> = (0..60)
        .map(|i| ClusterObservation {
            cluster_id: format!("c{i}"),
            d,
            weight: 1.0,
        })
        .collect();
    let formal = Evaluator::grade(
        &eval,
        GradeRequest {
            plan: &plan,
            book: &mut book,
            ticket_id: "t1",
            exec: &exec,
            rows: &rows,
            alpha_i: 0.05,
            cost_ratio: 1.0,
            p95_ratio: 1.0,
            safety_ok: true,
        },
    )?;
    // `credentials_present` is retained only for source compatibility with the
    // old example. It is not transport evidence and grants no authority.
    let _ = credentials_present;
    let auto_promote = false;
    let release = Release {
        id: "rel-fixture-hold".into(),
        bundle_digest: bundle.digest.clone(),
        evaluation_id: formal.id.clone(),
        parent_pointer: bundle.parent_digest.clone(),
        state: crate::releases::ReleaseState::Evaluated,
        approved_by: None,
    };
    let receipt = AppliedReceipt {
        offered: vec!["evo_prepare".into()],
        attached: Vec::new(),
        used: Vec::new(),
        verified_benefit: Vec::new(),
        bundle_digest: bundle.digest.clone(),
        request_digest: "req-mvc".into(),
        capability_level: CapabilityLevel::ToolOnly,
        truncated: false,
        attested_by: host.actor().into(),
    };
    let _ = routing;
    Ok((
        LoopTrace {
            steps: vec![
                LoopStep::FailedRun,
                LoopStep::Aggregate,
                LoopStep::Route,
                LoopStep::Compile,
                LoopStep::Evaluate,
            ],
            route_class: RouteClass::Procedural,
            verdict: Some(formal.verdict),
            used_real_model: false,
            auto_promote,
        },
        release,
        receipt,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_loop_closes_zero_effect_without_auto_promote() {
        let (trace, rel, receipt) = run_structural_loop(Verdict::Inconclusive, false).unwrap();
        assert!(!trace.used_real_model);
        assert!(!trace.auto_promote);
        assert_eq!(trace.verdict, Some(Verdict::Inconclusive));
        assert!(receipt.verified_benefit.is_empty());
        assert_ne!(rel.state, crate::releases::ReleaseState::Active);
    }

    #[test]
    fn missing_credentials_block_real_model() {
        assert!(require_real_model(false).is_err());
    }

    #[test]
    fn mock_flag_is_explicit() {
        assert!(mock_cannot_prove_model());
    }

    #[test]
    fn simulated_gain_without_credentials_does_not_activate() {
        let (trace, rel, _) = run_structural_loop(Verdict::Improved, false).unwrap();
        assert!(!trace.auto_promote);
        assert_ne!(rel.state, crate::releases::ReleaseState::Active);
    }

    #[test]
    fn credentials_flag_never_proves_real_model_or_promotes() {
        assert!(require_real_model(true).is_err());
        let (trace, release, _) = run_structural_loop(Verdict::Improved, true).unwrap();
        assert!(!trace.used_real_model);
        assert!(!trace.auto_promote);
        assert_eq!(
            trace.steps,
            vec![
                LoopStep::FailedRun,
                LoopStep::Aggregate,
                LoopStep::Route,
                LoopStep::Compile,
                LoopStep::Evaluate,
            ]
        );
        assert_eq!(release.state, crate::releases::ReleaseState::Evaluated);
    }
}
