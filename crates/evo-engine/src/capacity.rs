//! MVP capacity. Exceeding the cap refuses new derive; it does not silent-truncate.
use evo_core::{Error, Result};

pub const MAX_RUNS: u64 = 1_000;
pub const MAX_EVENTS: u64 = 10_000;
pub const MAX_SKILLS: u64 = 1_000;
pub const MAX_PREPARE: u64 = 5;

#[derive(Debug, Clone, Copy)]
pub struct Usage {
    pub runs: u64,
    pub events: u64,
    pub skills: u64,
    pub inflight_prepare: u64,
}

pub fn admit(u: Usage) -> Result<()> {
    if u.runs >= MAX_RUNS
        || u.events >= MAX_EVENTS
        || u.skills >= MAX_SKILLS
        || u.inflight_prepare >= MAX_PREPARE
    {
        return Err(Error::Conflict(
            "MVP capacity exceeded; stop new derive and scale or clean up".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_cap_stops_not_truncates() {
        assert!(
            admit(Usage {
                runs: 0,
                events: 0,
                skills: 0,
                inflight_prepare: 0
            })
            .is_ok()
        );
        assert!(
            admit(Usage {
                runs: MAX_RUNS,
                events: 0,
                skills: 0,
                inflight_prepare: 0
            })
            .is_err()
        );
    }
}
