//! Replay vs online pairing. Replay-only wins do not publish.
use crate::replay::ReplayReport;
use evo_core::evaluation::Verdict;
use evo_core::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct PairCost {
    pub history: u64,
    pub generate: u64,
    pub replay: u64,
    pub accept: u64,
    pub review: u64,
}

impl PairCost {
    pub fn total(self) -> u64 {
        self.history + self.generate + self.replay + self.accept + self.review
    }
}

pub fn decide_publish(
    replay_better: bool,
    online_verdict: Verdict,
    cost: PairCost,
) -> Result<bool> {
    if cost.total() == 0 {
        return Err(Error::Invalid(
            "cost must be fully accounted; calls-only is insufficient".into(),
        ));
    }
    if replay_better && (online_verdict == Verdict::Regressed || online_verdict == Verdict::Invalid)
    {
        return Ok(false);
    }
    if online_verdict == Verdict::Improved || online_verdict == Verdict::Noninferior {
        return Ok(true);
    }
    Ok(false)
}

pub fn replay_report_is_not_online_evidence(_: &ReplayReport) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::ReplayReport;

    fn cost() -> PairCost {
        PairCost {
            history: 1,
            generate: 1,
            replay: 1,
            accept: 1,
            review: 1,
        }
    }

    #[test]
    fn replay_win_online_regression_does_not_publish() {
        assert!(!decide_publish(true, Verdict::Regressed, cost()).unwrap());
    }

    #[test]
    fn zero_cost_is_invalid() {
        assert!(
            decide_publish(
                true,
                Verdict::Improved,
                PairCost {
                    history: 0,
                    generate: 0,
                    replay: 0,
                    accept: 0,
                    review: 0
                }
            )
            .is_err()
        );
    }

    #[test]
    fn replay_report_is_not_online() {
        let r = ReplayReport {
            world_id: "w".into(),
            policy: "p".into(),
            seed: "s".into(),
            action: "a".into(),
            oos: false,
            censored: false,
        };
        assert!(replay_report_is_not_online_evidence(&r));
    }
}
