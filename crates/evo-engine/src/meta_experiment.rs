//! Successor-quality experiment. Three streams × three rounds prove orchestration only.
use evo_core::evaluation::{ControlCondition, Verdict};
use evo_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamRound {
    pub stream: u8,
    pub round: u8,
    pub condition: ControlCondition,
    pub successor_digest: String,
}

pub fn orchestration_only(rounds: &[StreamRound]) -> Result<bool> {
    let streams: std::collections::BTreeSet<u8> = rounds.iter().map(|r| r.stream).collect();
    let max_round = rounds.iter().map(|r| r.round).max().unwrap_or(0);
    if streams.len() == 3 && max_round == 3 {
        return Ok(false);
    }
    Err(Error::Invalid("incomplete orchestration schedule".into()))
}

pub fn d_minus_c(d: Verdict, c: Verdict) -> Result<&'static str> {
    match (d, c) {
        (Verdict::Improved, _) => Ok("mechanism_plus_gain_not_claimed_without_n"),
        (_, Verdict::Improved) if d != Verdict::Improved => {
            Ok("no_extra_gain_from_evolving_improver")
        }
        _ => Ok("bounded_mechanism_reuse"),
    }
}

pub fn cannot_claim_l5() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::evaluation::ControlCondition;

    #[test]
    fn three_by_three_is_not_statistical_power() {
        let mut rows = Vec::new();
        for s in 1..=3 {
            for r in 1..=3 {
                rows.push(StreamRound {
                    stream: s,
                    round: r,
                    condition: ControlCondition::FixedImproverC,
                    successor_digest: format!("d{s}{r}"),
                });
            }
        }
        assert!(!orchestration_only(&rows).unwrap());
        assert!(cannot_claim_l5());
        assert_eq!(
            d_minus_c(Verdict::Inconclusive, Verdict::Inconclusive).unwrap(),
            "bounded_mechanism_reuse"
        );
    }
}
