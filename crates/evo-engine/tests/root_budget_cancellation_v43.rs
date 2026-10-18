//! E04 / V008: cancellation releases only undispatched reservations and keeps
//! dispatched usage uncertain. This covers the in-memory library contract.

use evo_core::Error;
use evo_engine::executor::{BudgetPhase, RootBudget};
use serde_json::Value;

const LEASE: &str = "lease-1";

fn budget_in_phase(phase: BudgetPhase) -> RootBudget {
    let mut budget = RootBudget::open("budget-1", "scope-1", 100, LEASE, 0).unwrap();
    budget.reserve(40).unwrap();
    match phase {
        BudgetPhase::Reserved => {}
        BudgetPhase::Dispatched => budget.dispatch(LEASE, 0).unwrap(),
        BudgetPhase::Uncertain => {
            budget.dispatch(LEASE, 0).unwrap();
            budget.timeout_uncertain(LEASE, 1).unwrap();
        }
        BudgetPhase::Cancelled => budget.cancel(LEASE, 0).unwrap(),
        BudgetPhase::Finalized => {
            budget.dispatch(LEASE, 0).unwrap();
            budget.finalize(LEASE, 1, 20).unwrap();
        }
    }
    budget
}

/// Include every public field when checking that fencing or replay did no work.
fn snapshot(budget: &RootBudget) -> Value {
    serde_json::to_value(budget).unwrap()
}

fn all_phases() -> [BudgetPhase; 5] {
    [
        BudgetPhase::Reserved,
        BudgetPhase::Dispatched,
        BudgetPhase::Finalized,
        BudgetPhase::Uncertain,
        BudgetPhase::Cancelled,
    ]
}

#[test]
fn reserved_cancel_releases_reservation_and_repeats_without_mutation() {
    let mut budget = budget_in_phase(BudgetPhase::Reserved);
    let mut expected = budget.clone();
    expected.reserved = 0;
    expected.phase = BudgetPhase::Cancelled;

    budget.cancel(LEASE, 1).unwrap();
    assert_eq!(snapshot(&budget), snapshot(&expected));
    assert_eq!((budget.reserved, budget.spent), (0, 0));

    let cancelled = snapshot(&budget);
    budget.cancel(LEASE, 2).unwrap();
    assert_eq!(snapshot(&budget), cancelled);
}

#[test]
fn dispatched_cancel_keeps_reservation_uncertain_and_repeats_without_mutation() {
    let mut budget = budget_in_phase(BudgetPhase::Dispatched);
    let mut expected = budget.clone();
    expected.phase = BudgetPhase::Uncertain;

    budget.cancel(LEASE, 1).unwrap();
    assert_eq!(snapshot(&budget), snapshot(&expected));
    assert_eq!((budget.reserved, budget.spent), (40, 0));

    let uncertain = snapshot(&budget);
    budget.cancel(LEASE, 2).unwrap();
    assert_eq!(snapshot(&budget), uncertain);
}

#[test]
fn timeout_uncertain_cancel_is_an_idempotent_request() {
    let mut budget = budget_in_phase(BudgetPhase::Uncertain);
    let before = snapshot(&budget);

    budget.cancel(LEASE, 2).unwrap();
    assert_eq!(snapshot(&budget), before);
    budget.cancel(LEASE, 3).unwrap();
    assert_eq!(snapshot(&budget), before);
}

#[test]
fn finalized_cancel_rejects_without_refunding_paid_usage() {
    let mut budget = budget_in_phase(BudgetPhase::Finalized);
    let before = snapshot(&budget);
    assert_eq!((budget.reserved, budget.spent), (0, 20));

    assert!(matches!(budget.cancel(LEASE, 2), Err(Error::Conflict(_))));
    assert_eq!(snapshot(&budget), before);
}

#[test]
fn wrong_token_cancel_preserves_every_field_in_each_phase() {
    for phase in all_phases() {
        let mut budget = budget_in_phase(phase);
        let before = snapshot(&budget);

        assert!(matches!(
            budget.cancel("old-lease", 2),
            Err(Error::Cancelled)
        ));
        assert_eq!(snapshot(&budget), before, "phase: {phase:?}");
    }
}

#[test]
fn expired_lease_cancel_preserves_every_field_in_each_phase() {
    for phase in all_phases() {
        let mut budget = budget_in_phase(phase);
        let before = snapshot(&budget);
        let expired = budget.lease_until + 1;

        assert!(matches!(
            budget.cancel(LEASE, expired),
            Err(Error::Cancelled)
        ));
        assert_eq!(snapshot(&budget), before, "phase: {phase:?}");
    }
}

#[test]
fn exact_lease_boundary_retains_the_phase_specific_cancel_contract() {
    for phase in all_phases() {
        let mut budget = budget_in_phase(phase);
        let boundary = budget.lease_until;
        let mut expected = budget.clone();
        match phase {
            BudgetPhase::Reserved => {
                expected.reserved = 0;
                expected.phase = BudgetPhase::Cancelled;
            }
            BudgetPhase::Dispatched => expected.phase = BudgetPhase::Uncertain,
            _ => {}
        }

        if phase == BudgetPhase::Finalized {
            assert!(matches!(
                budget.cancel(LEASE, boundary),
                Err(Error::Conflict(_))
            ));
        } else {
            budget.cancel(LEASE, boundary).unwrap();
        }
        assert_eq!(snapshot(&budget), snapshot(&expected), "phase: {phase:?}");
    }
}

#[test]
fn cancelled_and_uncertain_budgets_cannot_reopen_or_release_unknown_usage() {
    let mut cancelled_dispatch = budget_in_phase(BudgetPhase::Dispatched);
    cancelled_dispatch.cancel(LEASE, 1).unwrap();

    for mut budget in [
        budget_in_phase(BudgetPhase::Cancelled),
        cancelled_dispatch,
        budget_in_phase(BudgetPhase::Uncertain),
    ] {
        let before = snapshot(&budget);

        assert!(matches!(budget.reserve(1), Err(Error::Conflict(_))));
        assert_eq!(snapshot(&budget), before);
        assert!(matches!(budget.dispatch(LEASE, 2), Err(Error::Conflict(_))));
        assert_eq!(snapshot(&budget), before);
        assert!(matches!(
            budget.finalize(LEASE, 2, 20),
            Err(Error::Conflict(_))
        ));
        assert_eq!(snapshot(&budget), before);
        assert!(matches!(
            budget.release_after_uncertain(),
            Err(Error::Conflict(_))
        ));
        assert_eq!(snapshot(&budget), before);
    }
}
