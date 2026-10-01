//! E12 / E04 (plan v4.2 §1.4, AG-050): the legacy `step` selects before it reserves
//! and keeps the typed selection error.
//!
//! Selection is a pure computation. The legacy `step` used to reserve the budget
//! first and select afterwards, and it folded every selection error into `NotFound`:
//! a selection that could never succeed still held reserved budget, and the cause was
//! lost. Now the feasibility check runs first. An infeasible selection reserves nothing
//! and returns its own typed error; only a feasible selection reserves `cost`; a failed
//! reservation returns the budget's own error and the selected task is not handed out.
//! The v2 path (`PersistentCurriculumCoordinator`) is not touched.

use evo_core::Error;
use evo_core::curriculum::{LearnerState, TaskProposal, proposal_from_text};
use evo_core::evaluation::DataUse;
use evo_engine::curriculum::step;
use evo_engine::executor::{BudgetPhase, RootBudget};

const LEASE: &str = "lease-1";

fn open_budget(limit: i64) -> RootBudget {
    RootBudget::open("budget-1", "scope-1", limit, LEASE, 0).unwrap()
}

fn learner(clusters: &[&str]) -> LearnerState {
    LearnerState {
        checkpoint: "checkpoint-1".into(),
        failure_clusters: clusters.iter().map(|cluster| (*cluster).into()).collect(),
    }
}

fn proposal(id: &str, family: &str, oracle_ok: bool) -> TaskProposal {
    TaskProposal {
        id: id.into(),
        parent_family: family.into(),
        data_use: DataUse::Development,
        difficulty: 0.5,
        learning_value: 0.5,
        correct: false,
        oracle_ok,
    }
}

/// Everything a rejected step must leave untouched.
fn ledger(budget: &RootBudget) -> (i64, i64, BudgetPhase) {
    (budget.reserved, budget.spent, budget.phase)
}

/// A budget that already holds a reservation, so "unchanged" means "kept as it was",
/// not merely "zero".
fn budget_with_prior_reservation() -> RootBudget {
    let mut budget = open_budget(10);
    let prior = [proposal("t-prior", "fam-a", true)];
    assert_eq!(
        step(&mut budget, &learner(&["fam-a"]), &prior, 2).unwrap(),
        "t-prior"
    );
    assert_eq!(ledger(&budget), (2, 0, BudgetPhase::Reserved));
    budget
}

#[test]
fn no_matching_failure_cluster_reserves_nothing_and_keeps_not_found() {
    let mut budget = budget_with_prior_reservation();
    let before = ledger(&budget);
    let pool = [proposal("t1", "fam-other", true)];
    assert!(matches!(
        step(&mut budget, &learner(&["fam-a"]), &pool, 3),
        Err(Error::NotFound)
    ));
    assert_eq!(ledger(&budget), before);
}

#[test]
fn empty_pool_reserves_nothing_and_keeps_not_found() {
    let mut budget = budget_with_prior_reservation();
    let before = ledger(&budget);
    assert!(matches!(
        step(&mut budget, &learner(&["fam-a"]), &[], 3),
        Err(Error::NotFound)
    ));
    assert_eq!(ledger(&budget), before);
}

#[test]
fn only_quarantined_proposals_reserve_nothing() {
    let mut budget = budget_with_prior_reservation();
    let before = ledger(&budget);
    let pool = [proposal("t1", "fam-a", false)];
    assert!(matches!(
        step(&mut budget, &learner(&["fam-a"]), &pool, 3),
        Err(Error::NotFound)
    ));
    assert_eq!(ledger(&budget), before);
}

#[test]
fn empty_learner_state_keeps_its_typed_invalid_error() {
    let mut budget = budget_with_prior_reservation();
    let before = ledger(&budget);
    let pool = [proposal("t1", "fam-a", true)];
    let error = step(&mut budget, &learner(&[]), &pool, 3).unwrap_err();
    assert!(
        matches!(error, Error::Invalid(_)),
        "the selection error must not be folded into NotFound: {error:?}"
    );
    assert_eq!(ledger(&budget), before);
}

#[test]
fn malformed_checkpoint_keeps_its_typed_invalid_error() {
    let mut budget = budget_with_prior_reservation();
    let before = ledger(&budget);
    let state = LearnerState {
        checkpoint: "not an identifier".into(),
        failure_clusters: vec!["fam-a".into()],
    };
    let pool = [proposal("t1", "fam-a", true)];
    let error = step(&mut budget, &state, &pool, 3).unwrap_err();
    assert!(
        matches!(error, Error::Invalid(_)),
        "the selection error must not be folded into NotFound: {error:?}"
    );
    assert_eq!(ledger(&budget), before);
}

#[test]
fn selection_feasibility_is_checked_before_the_budget() {
    // Neither selection can succeed and the budget could not cover the cost either:
    // the selection error is what the caller sees, and nothing is reserved.
    let mut budget = open_budget(1);
    assert!(matches!(
        step(&mut budget, &learner(&["fam-a"]), &[], 5),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        step(&mut budget, &learner(&[]), &[], 5),
        Err(Error::Invalid(_))
    ));
    assert_eq!(ledger(&budget), (0, 0, BudgetPhase::Reserved));
}

#[test]
fn feasible_selection_reserves_exactly_once() {
    let mut budget = open_budget(10);
    let pool = [proposal("t1", "fam-a", true)];
    let state = learner(&["fam-a"]);
    assert_eq!(step(&mut budget, &state, &pool, 3).unwrap(), "t1");
    assert_eq!(ledger(&budget), (3, 0, BudgetPhase::Reserved));
    assert_eq!(step(&mut budget, &state, &pool, 4).unwrap(), "t1");
    assert_eq!(ledger(&budget), (7, 0, BudgetPhase::Reserved));
}

#[test]
fn insufficient_budget_returns_the_budget_error_and_reserves_nothing() {
    let mut budget = open_budget(5);
    let pool = [proposal("t1", "fam-a", true)];
    let state = learner(&["fam-a"]);
    step(&mut budget, &state, &pool, 3).unwrap();
    let before = ledger(&budget);
    assert!(matches!(
        step(&mut budget, &state, &pool, 3),
        Err(Error::Budget)
    ));
    assert_eq!(ledger(&budget), before);
    // The limit itself is still reachable exactly; one unit more is not.
    step(&mut budget, &state, &pool, 2).unwrap();
    assert_eq!(ledger(&budget), (5, 0, BudgetPhase::Reserved));
    assert!(matches!(
        step(&mut budget, &state, &pool, 1),
        Err(Error::Budget)
    ));
    assert_eq!(ledger(&budget), (5, 0, BudgetPhase::Reserved));
}

#[test]
fn reservation_errors_stay_typed_after_a_feasible_selection() {
    let pool = [proposal("t1", "fam-a", true)];
    let state = learner(&["fam-a"]);
    // A reservation must be positive.
    let mut budget = open_budget(10);
    for cost in [0, -1] {
        assert!(matches!(
            step(&mut budget, &state, &pool, cost),
            Err(Error::Invalid(_))
        ));
        assert_eq!(ledger(&budget), (0, 0, BudgetPhase::Reserved));
    }
    // A budget that is no longer in its reservable phase refuses the reservation.
    let mut dispatched = open_budget(10);
    dispatched.dispatch(LEASE, 0).unwrap();
    assert!(matches!(
        step(&mut dispatched, &state, &pool, 3),
        Err(Error::Conflict(_))
    ));
    assert_eq!(ledger(&dispatched), (0, 0, BudgetPhase::Dispatched));
}

#[test]
fn a_text_derived_proposal_is_neither_selected_nor_paid_for() {
    let mut budget = open_budget(10);
    let state = learner(&["fam-a"]);
    let text_derived =
        proposal_from_text("t-text", "fam-a", "Return the sum of two integers.").unwrap();
    assert!(matches!(
        step(&mut budget, &state, std::slice::from_ref(&text_derived), 3),
        Err(Error::NotFound)
    ));
    assert_eq!(ledger(&budget), (0, 0, BudgetPhase::Reserved));
    // A compliant proposal in the same pool is the one selected and the only thing paid for.
    let pool = [text_derived, proposal("t-ok", "fam-a", true)];
    assert_eq!(step(&mut budget, &state, &pool, 3).unwrap(), "t-ok");
    assert_eq!(ledger(&budget), (3, 0, BudgetPhase::Reserved));
}
