//! Independent grader. Replay and self-reports cannot mint a FormalEvaluation.
use evo_core::evaluation::{
    BernsteinReport, ClusterObservation, ExperimentPlan, QueryBook, QueryTicket, Verdict, decide,
    empirical_bernstein,
};
use evo_core::{Context, Error, Result, Role, fingerprint, identifier};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReport {
    pub ticket_id: String,
    pub environment_digest: String,
    pub outputs_complete: bool,
    pub units: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReport {
    pub world_id: String,
    pub policy_digest: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalEvaluation {
    pub id: String,
    pub plan_digest: String,
    pub candidate_digest: String,
    pub stats_version: String,
    pub verdict: Verdict,
    pub report: BernsteinReport,
}

impl FormalEvaluation {
    pub fn from_replay(_: &ReplayReport) -> Result<Self> {
        Err(Error::Invalid(
            "ReplayReport cannot be converted into FormalEvaluation".into(),
        ))
    }
}

pub struct GradeRequest<'a> {
    pub plan: &'a ExperimentPlan,
    pub book: &'a mut QueryBook,
    pub ticket_id: &'a str,
    pub exec: &'a ExecutionReport,
    pub rows: &'a [ClusterObservation],
    pub alpha_i: f64,
    pub cost_ratio: f64,
    pub p95_ratio: f64,
    pub safety_ok: bool,
}

pub struct Evaluator;

impl Evaluator {
    pub fn start(
        ctx: &Context,
        plan: &mut ExperimentPlan,
        candidate: &str,
        ticket: QueryTicket,
        book: &mut QueryBook,
    ) -> Result<()> {
        ctx.require(&[Role::Evaluator])?;
        if !plan.frozen {
            return Err(Error::Invalid(
                "plan must be frozen before evaluation".into(),
            ));
        }
        if plan.candidate_digest.is_none() {
            plan.bind_candidate(candidate)?;
        }
        identifier(&ticket.id)?;
        book.reserve(ticket)
    }

    pub fn grade(ctx: &Context, req: GradeRequest<'_>) -> Result<FormalEvaluation> {
        ctx.require(&[Role::Evaluator])?;
        if req.exec.ticket_id != req.ticket_id {
            req.book.fail(req.ticket_id)?;
            return Err(Error::Conflict(
                "execution report is bound to a different ticket".into(),
            ));
        }
        if !req.exec.outputs_complete {
            req.book.fail(req.ticket_id)?;
            return Err(Error::Invalid("incomplete execution is invalid".into()));
        }
        let evaluated = (|| {
            let raw = empirical_bernstein(req.rows, req.alpha_i)?;
            let report = decide(req.plan, raw, req.cost_ratio, req.p95_ratio, req.safety_ok)?;
            let candidate = req
                .plan
                .candidate_digest
                .clone()
                .ok_or_else(|| Error::Invalid("candidate unbound".into()))?;
            Ok(FormalEvaluation {
                id: format!("ev-{}", req.ticket_id),
                plan_digest: req.plan.digest()?,
                candidate_digest: candidate,
                stats_version: report.stats_version.clone(),
                verdict: report.verdict,
                report,
            })
        })();
        match evaluated {
            Ok(formal) => {
                req.book.complete(req.ticket_id)?;
                Ok(formal)
            }
            Err(error) => {
                req.book.fail(req.ticket_id)?;
                Err(error)
            }
        }
    }
}

pub fn self_report_is_not_formal(ctx: &Context) -> Result<()> {
    if ctx.role() != Role::Evaluator {
        return Err(Error::Forbidden);
    }
    Ok(())
}

pub fn evaluation_identity(eval: &FormalEvaluation) -> Result<String> {
    fingerprint(eval)
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::Role;
    use evo_core::evaluation::ExperimentPlan;

    fn frozen() -> (ExperimentPlan, QueryBook, QueryTicket) {
        let mut plan = ExperimentPlan::first_low_risk("exp").unwrap();
        plan.freeze(1).unwrap();
        plan.bind_candidate("cand").unwrap();
        let book = QueryBook::open(&plan).unwrap();
        let ticket = QueryTicket {
            id: "t1".into(),
            plan_digest: plan.digest().unwrap(),
            candidate_digest: "cand".into(),
            dataset_epoch: plan.dataset_epoch.clone(),
            seed: "s1".into(),
        };
        (plan, book, ticket)
    }

    #[test]
    fn agent_cannot_grade() {
        let ctx = Context::new("n", "a", Role::Agent).unwrap();
        let (mut plan, mut book, ticket) = frozen();
        assert!(Evaluator::start(&ctx, &mut plan, "cand", ticket, &mut book).is_err());
    }

    #[test]
    fn replay_cannot_become_formal() {
        let replay = ReplayReport {
            world_id: "w".into(),
            policy_digest: "p".into(),
            score: 1.0,
        };
        assert!(FormalEvaluation::from_replay(&replay).is_err());
    }

    #[test]
    fn incomplete_rows_fail_the_ticket_without_refund() {
        let ctx = Context::new("n", "e", Role::Evaluator).unwrap();
        let (mut plan, mut book, ticket) = frozen();
        Evaluator::start(&ctx, &mut plan, "cand", ticket.clone(), &mut book).unwrap();
        let exec = ExecutionReport {
            ticket_id: "t1".into(),
            environment_digest: "env".into(),
            outputs_complete: false,
            units: 1,
        };
        assert!(
            Evaluator::grade(
                &ctx,
                GradeRequest {
                    plan: &plan,
                    book: &mut book,
                    ticket_id: "t1",
                    exec: &exec,
                    rows: &[],
                    alpha_i: 0.05,
                    cost_ratio: 1.0,
                    p95_ratio: 1.0,
                    safety_ok: true,
                }
            )
            .is_err()
        );
        assert!(book.reserve(ticket).is_err());
    }

    #[test]
    fn mismatched_execution_ticket_fails_without_consumed_success() {
        let ctx = Context::new("n", "e", Role::Evaluator).unwrap();
        let (mut plan, mut book, ticket) = frozen();
        Evaluator::start(&ctx, &mut plan, "cand", ticket.clone(), &mut book).unwrap();
        let exec = ExecutionReport {
            ticket_id: "forged-ticket".into(),
            environment_digest: "env".into(),
            outputs_complete: true,
            units: 1,
        };
        assert!(
            Evaluator::grade(
                &ctx,
                GradeRequest {
                    plan: &plan,
                    book: &mut book,
                    ticket_id: "t1",
                    exec: &exec,
                    rows: &[],
                    alpha_i: 0.05,
                    cost_ratio: 1.0,
                    p95_ratio: 1.0,
                    safety_ok: true,
                }
            )
            .is_err()
        );
        assert!(book.reserve(ticket).is_err());
    }

    #[test]
    fn invalid_statistics_fail_before_ticket_completion() {
        let ctx = Context::new("n", "e", Role::Evaluator).unwrap();
        let (mut plan, mut book, ticket) = frozen();
        Evaluator::start(&ctx, &mut plan, "cand", ticket.clone(), &mut book).unwrap();
        let exec = ExecutionReport {
            ticket_id: "t1".into(),
            environment_digest: "env".into(),
            outputs_complete: true,
            units: 1,
        };
        let rows = [
            ClusterObservation {
                cluster_id: "c0".into(),
                d: 0.0,
                weight: 1.0,
            },
            ClusterObservation {
                cluster_id: "c0".into(),
                d: 0.0,
                weight: 1.0,
            },
        ];
        assert!(
            Evaluator::grade(
                &ctx,
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
                }
            )
            .is_err()
        );
        assert!(book.reserve(ticket).is_err());
    }

    #[test]
    fn zero_effect_is_kept_as_inconclusive_not_improved() {
        let ctx = Context::new("n", "e", Role::Evaluator).unwrap();
        let (mut plan, mut book, ticket) = frozen();
        Evaluator::start(&ctx, &mut plan, "cand", ticket, &mut book).unwrap();
        let exec = ExecutionReport {
            ticket_id: "t1".into(),
            environment_digest: "env".into(),
            outputs_complete: true,
            units: 1,
        };
        let rows: Vec<_> = (0..60)
            .map(|i| ClusterObservation {
                cluster_id: format!("c{i}"),
                d: 0.0,
                weight: 1.0,
            })
            .collect();
        let formal = Evaluator::grade(
            &ctx,
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
        )
        .unwrap();
        assert_ne!(formal.verdict, Verdict::Improved);
    }
}
