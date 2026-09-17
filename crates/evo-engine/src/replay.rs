//! Lookup-only replay. No model or task-code execution.
use evo_core::evaluation::Verdict;
use evo_core::{Error, Result, identifier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const OBJECTIVE: &str = "rsia.attainment_auc.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayWorld {
    pub id: String,
    pub cluster_id: String,
    pub generation_signature: String,
    pub sealed: bool,
    pub actions: BTreeMap<String, String>,
}

impl ReplayWorld {
    pub fn seal(&mut self) {
        self.sealed = true;
    }

    pub fn lookup(&self, policy: &str, seed: &str) -> Result<String> {
        if !self.sealed {
            return Err(Error::Invalid("world not sealed".into()));
        }
        identifier(&self.id)?;
        let key = format!("{policy}:{seed}");
        self.actions
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::Invalid("unsupported or censored prefix".into()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReport {
    pub world_id: String,
    pub policy: String,
    pub seed: String,
    pub action: String,
    pub oos: bool,
    pub censored: bool,
}

pub fn replay(world: &ReplayWorld, policy: &str, seed: &str) -> Result<ReplayReport> {
    match world.lookup(policy, seed) {
        Ok(action) => Ok(ReplayReport {
            world_id: world.id.clone(),
            policy: policy.into(),
            seed: seed.into(),
            action,
            oos: false,
            censored: false,
        }),
        Err(_) => Ok(ReplayReport {
            world_id: world.id.clone(),
            policy: policy.into(),
            seed: seed.into(),
            action: String::new(),
            oos: true,
            censored: true,
        }),
    }
}

pub fn replay_is_not_formal(report: &ReplayReport) -> Result<Verdict> {
    let _ = report;
    Err(Error::Invalid("ReplayReport is development-only".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> ReplayWorld {
        let mut w = ReplayWorld {
            id: "w1".into(),
            cluster_id: "c1".into(),
            generation_signature: "g1".into(),
            sealed: false,
            actions: BTreeMap::new(),
        };
        w.actions.insert("pol:s1".into(), "act-a".into());
        w.seal();
        w
    }

    #[test]
    fn same_world_policy_seed_same_action() {
        let w = world();
        let a = replay(&w, "pol", "s1").unwrap();
        let b = replay(&w, "pol", "s1").unwrap();
        assert_eq!(a.action, b.action);
        assert!(!a.oos);
    }

    #[test]
    fn opaque_id_rename_does_not_change_lookup_key() {
        let mut w = world();
        w.id = "other-id".into();
        assert_eq!(replay(&w, "pol", "s1").unwrap().action, "act-a");
    }

    #[test]
    fn missing_prefix_is_oos_not_a_fake_score() {
        let r = replay(&world(), "pol", "missing").unwrap();
        assert!(r.oos);
        assert!(replay_is_not_formal(&r).is_err());
    }
}
