//! Curriculum step uses the original root budget, not a free extra pool.
use crate::executor::RootBudget;
use evo_core::curriculum::{LearnerState, TaskProposal, next_task};
use evo_core::{Error, Result};

pub fn step(
    budget: &mut RootBudget,
    state: &LearnerState,
    pool: &[TaskProposal],
    cost: i64,
) -> Result<String> {
    budget.reserve(cost)?;
    next_task(state, pool).map_err(|_| Error::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::curriculum::TaskProposal;
    use evo_core::evaluation::DataUse;

    #[test]
    fn curriculum_spend_comes_from_root_budget() {
        let mut b = RootBudget::open("b1", "scope", 5, "lease", 0).unwrap();
        let state = LearnerState {
            checkpoint: "c1".into(),
            failure_clusters: vec!["fam".into()],
        };
        let p = TaskProposal {
            id: "t1".into(),
            parent_family: "fam".into(),
            data_use: DataUse::Development,
            difficulty: 0.5,
            learning_value: 0.5,
            correct: false,
            oracle_ok: true,
        };
        assert_eq!(step(&mut b, &state, &[p], 3).unwrap(), "t1");
        assert!(step(&mut b, &state, &[], 1).is_err());
    }
}
