//! Optional extensions stay disabled until a revised plan is reviewed.
use crate::{Error, Result};

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
}
