//! Restricted improver inheritance. Recursion depth 1. Final grader is not writable.
use evo_core::contract::assert_candidate_may_write;
use evo_core::{ArtifactKind, Error, Result};
use serde::{Deserialize, Serialize};

pub const META_DEPTH_CAP: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaCandidate {
    pub depth: u8,
    pub mechanism: String,
    pub used_in_next_job: bool,
    pub parent_improver: String,
}

impl MetaCandidate {
    pub fn validate(&self) -> Result<()> {
        if self.depth > META_DEPTH_CAP {
            return Err(Error::Invalid("meta recursion depth cap is 1".into()));
        }
        match self.mechanism.as_str() {
            "generation" | "exploration" => Ok(()),
            "acquisition" => Err(Error::Invalid("AcquisitionPolicy has no consumer".into())),
            "grader" | "budget" | "approval" | "goal" => Err(Error::Forbidden),
            _ => Err(Error::Invalid("unknown meta mechanism".into())),
        }
    }
}

pub fn i1_must_run_next_job(c: &MetaCandidate) -> Result<()> {
    c.validate()?;
    if !c.used_in_next_job {
        return Err(Error::Invalid(
            "I1 must actually be used to propose/filter I2".into(),
        ));
    }
    Ok(())
}

pub fn cannot_write_protected(field: &str) -> Result<()> {
    assert_candidate_may_write(ArtifactKind::Improver, field)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_and_protected_fields() {
        let mut c = MetaCandidate {
            depth: 2,
            mechanism: "generation".into(),
            used_in_next_job: true,
            parent_improver: "i0".into(),
        };
        assert!(c.validate().is_err());
        c.depth = 1;
        c.validate().unwrap();
        c.used_in_next_job = false;
        assert!(i1_must_run_next_job(&c).is_err());
        assert!(cannot_write_protected("budget").is_err());
        assert!(cannot_write_protected("improver.instruction").is_ok());
    }
}
