//! Drift and retention. Old gain proofs do not survive environment change.
use evo_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFingerprint {
    pub model: String,
    pub tools: String,
    pub host: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillFate {
    Keep,
    Unused,
    NoGain,
    Harmful,
    Stale,
}

pub fn drift_invalidates_gain(old: &EnvironmentFingerprint, now: &EnvironmentFingerprint) -> bool {
    old.model != now.model || old.tools != now.tools || old.host != now.host
}

pub fn retention_matrix(family_pass: &BTreeMap<String, bool>) -> Result<()> {
    if family_pass.is_empty() {
        return Err(Error::Invalid("insufficient retention sample".into()));
    }
    if family_pass.values().any(|ok| !ok) {
        return Err(Error::Conflict(
            "family retention failed; stop new use".into(),
        ));
    }
    Ok(())
}

pub fn classify(adopted: bool, gain: bool, harmful: bool, stale: bool) -> SkillFate {
    if harmful {
        SkillFate::Harmful
    } else if stale {
        SkillFate::Stale
    } else if !adopted {
        SkillFate::Unused
    } else if !gain {
        SkillFate::NoGain
    } else {
        SkillFate::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_change_invalidates_old_gain() {
        let a = EnvironmentFingerprint {
            model: "m1".into(),
            tools: "t".into(),
            host: "h".into(),
        };
        let b = EnvironmentFingerprint {
            model: "m2".into(),
            ..a.clone()
        };
        assert!(drift_invalidates_gain(&a, &b));
        assert!(!drift_invalidates_gain(&a, &a));
    }

    #[test]
    fn harmful_stops_immediately() {
        assert_eq!(classify(true, true, true, false), SkillFate::Harmful);
        let mut m = BTreeMap::new();
        m.insert("fam".into(), false);
        assert!(retention_matrix(&m).is_err());
    }
}
