//! Learner-conditioned task proposals. Generated items stay in development.
use crate::evaluation::DataUse;
use crate::{Error, Result, identifier, text};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnerState {
    pub checkpoint: String,
    pub failure_clusters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskProposal {
    pub id: String,
    pub parent_family: String,
    pub data_use: DataUse,
    pub difficulty: f64,
    pub learning_value: f64,
    pub correct: bool,
    pub oracle_ok: bool,
}

impl TaskProposal {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.id)?;
        identifier(&self.parent_family)?;
        if !self.difficulty.is_finite() || !self.learning_value.is_finite() {
            return Err(Error::Invalid("non-finite curriculum scores".into()));
        }
        if self.data_use.is_protected_holdout() {
            return Err(Error::Conflict(
                "curriculum items cannot enter holdout".into(),
            ));
        }
        Ok(())
    }

    pub fn quarantine_reason(&self) -> Option<&'static str> {
        if !self.oracle_ok {
            return Some("no_oracle");
        }
        if self.correct && self.learning_value <= 0.0 {
            return Some("correct_but_no_learning_value");
        }
        None
    }
}

pub fn next_task(state: &LearnerState, pool: &[TaskProposal]) -> Result<String> {
    identifier(&state.checkpoint)?;
    if state.failure_clusters.is_empty() {
        return Err(Error::Invalid(
            "empty learner state cannot drive selection".into(),
        ));
    }
    pool.iter()
        .filter(|p| p.validate().is_ok() && p.quarantine_reason().is_none())
        .find(|p| state.failure_clusters.contains(&p.parent_family))
        .map(|p| p.id.clone())
        .ok_or(Error::NotFound)
}

pub fn proposal_from_text(id: &str, family: &str, body: &str) -> Result<TaskProposal> {
    text(body, "body", 4096)?;
    Ok(TaskProposal {
        id: id.into(),
        parent_family: family.into(),
        data_use: DataUse::Development,
        difficulty: 0.5,
        learning_value: 0.5,
        correct: false,
        oracle_ok: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learner_state_changes_next_task() {
        let a = TaskProposal {
            id: "t1".into(),
            parent_family: "fam-a".into(),
            data_use: DataUse::Development,
            difficulty: 0.2,
            learning_value: 0.8,
            correct: false,
            oracle_ok: true,
        };
        let b = TaskProposal {
            id: "t2".into(),
            parent_family: "fam-b".into(),
            ..a.clone()
        };
        let s1 = LearnerState {
            checkpoint: "c1".into(),
            failure_clusters: vec!["fam-a".into()],
        };
        let s2 = LearnerState {
            checkpoint: "c2".into(),
            failure_clusters: vec!["fam-b".into()],
        };
        let pool = [a, b];
        assert_eq!(next_task(&s1, &pool).unwrap(), "t1");
        assert_eq!(next_task(&s2, &pool).unwrap(), "t2");
    }

    #[test]
    fn holdout_and_no_oracle_rejected() {
        let mut p = TaskProposal {
            id: "t1".into(),
            parent_family: "fam".into(),
            data_use: DataUse::AcceptanceEpoch,
            difficulty: 0.2,
            learning_value: 0.8,
            correct: true,
            oracle_ok: false,
        };
        assert!(p.validate().is_err());
        p.data_use = DataUse::Development;
        p.validate().unwrap();
        assert_eq!(p.quarantine_reason(), Some("no_oracle"));
    }
}
