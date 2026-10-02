//! Legacy in-memory shape/count contracts. These fixtures do not authorize funding.

use evo_core::evaluation::{
    ControlCondition, ExperimentPlan, Money, ProfileKind, QueryBook, QueryTicket,
    STATS_HOEFFDING_V1,
};
use evo_core::{Error, hash};
use serde::Serialize;

const SECRET: &str = "AG080_SECRET";
const EPOCH_ERROR: &str = "ticket does not match frozen dataset epoch";

fn frozen_plan() -> ExperimentPlan {
    let mut plan = ExperimentPlan::first_low_risk("ag080-plan", 60).unwrap();
    plan.freeze(123).unwrap();
    plan.bind_candidate("candidate-a").unwrap();
    plan
}

fn ticket(plan: &ExperimentPlan, id: &str) -> QueryTicket {
    QueryTicket {
        id: id.into(),
        plan_digest: plan.digest().unwrap(),
        candidate_digest: plan.candidate_digest.clone().unwrap(),
        dataset_epoch: plan.dataset_epoch.clone(),
        seed: format!("seed-{id}"),
    }
}

fn same_error(expected: &Error, actual: &Error) -> bool {
    std::mem::discriminant(expected) == std::mem::discriminant(actual)
        && expected.to_string() == actual.to_string()
}

fn assert_error(expected: Error, actual: Error) {
    assert!(
        same_error(&expected, &actual),
        "expected {expected:?}, got {actual:?}"
    );
    assert!(!actual.to_string().contains(SECRET));
}

struct BadPlan {
    axis: &'static str,
    name: String,
    plan: ExperimentPlan,
}

fn invalid_plans() -> Vec<BadPlan> {
    let mut cases = Vec::new();
    let mut push = |axis, name: &str, change: fn(&mut ExperimentPlan)| {
        let mut plan = frozen_plan();
        change(&mut plan);
        cases.push(BadPlan {
            axis,
            name: name.into(),
            plan,
        });
    };
    push("versions", "schema", |p| p.schema_version = SECRET.into());
    push("versions", "objective", |p| {
        p.objective_version = SECRET.into()
    });
    push("versions", "stats", |p| p.stats_version = SECRET.into());
    push("identifiers", "empty_id", |p| p.id.clear());
    push("identifiers", "bad_id", |p| p.id = format!("{SECRET}/plan"));
    push("identifiers", "unicode_id", |p| p.id = "计划".into());
    push("identifiers", "long_id", |p| p.id = "p".repeat(129));
    push("identifiers", "empty_epoch", |p| p.dataset_epoch.clear());
    push("identifiers", "bad_epoch", |p| {
        p.dataset_epoch = format!("{SECRET}/epoch")
    });
    push("identifiers", "long_epoch", |p| {
        p.dataset_epoch = "e".repeat(129)
    });
    push("identifiers", "empty_candidate", |p| {
        p.candidate_digest = Some(String::new())
    });
    push("identifiers", "bad_candidate", |p| {
        p.candidate_digest = Some(format!("{SECRET}/candidate"))
    });
    push("identifiers", "long_candidate", |p| {
        p.candidate_digest = Some("c".repeat(129))
    });
    push("conditions", "empty", |p| p.conditions.clear());
    push("conditions", "duplicate", |p| {
        p.conditions.push(p.conditions[0])
    });
    push("conditions", "too_many", |p| {
        p.conditions = vec![ControlCondition::FrozenA; 6]
    });
    push("conditions", "first_b0", |p| {
        p.conditions.push(ControlCondition::MemoryOnlyB0)
    });
    push("conditions", "first_d", |p| {
        p.conditions.push(ControlCondition::EvolvingImproverD)
    });
    push("bounds", "n_zero", |p| p.n_planned = 0);
    push("bounds", "n_one", |p| p.n_planned = 1);
    push("bounds", "n_large", |p| p.n_planned = 100_001);
    push("bounds", "query_zero", |p| p.query_limit = 0);
    push("bounds", "query_large", |p| p.query_limit = 21);
    push("bounds", "effect_negative", |p| p.min_effect = -0.01);
    push("bounds", "effect_large", |p| p.min_effect = 1.01);
    push("bounds", "noninferior_positive", |p| {
        p.noninferior_bound = 0.01
    });
    push("bounds", "noninferior_small", |p| {
        p.noninferior_bound = -1.01
    });
    push("bounds", "savings_negative", |p| p.savings_ratio = -0.01);
    push("bounds", "savings_large", |p| p.savings_ratio = 1.01);
    push("bounds", "cost_small", |p| p.max_cost_ratio = 0.09);
    push("bounds", "cost_large", |p| p.max_cost_ratio = 10.01);
    push("bounds", "latency_small", |p| {
        p.max_p95_latency_ratio = 0.09
    });
    push("bounds", "latency_large", |p| {
        p.max_p95_latency_ratio = 10.01
    });
    push("bounds", "alpha_zero", |p| p.alpha_total = 0.0);
    push("bounds", "alpha_small", |p| p.alpha_total = 0.00009);
    push("bounds", "alpha_large", |p| p.alpha_total = 0.5);
    push("money", "empty_amount", |p| {
        p.monetary_budget.amount.clear()
    });
    push("money", "secret_amount", |p| {
        p.monetary_budget.amount = SECRET.into()
    });
    push("money", "nan_amount", |p| {
        p.monetary_budget.amount = "NaN".into()
    });
    push("money", "inf_amount", |p| {
        p.monetary_budget.amount = "+Inf".into()
    });
    push("money", "negative_zero", |p| {
        p.monetary_budget.amount = "-0".into()
    });
    push("money", "over_precision", |p| {
        p.monetary_budget.amount = "0.123456789".into()
    });
    push("money", "leading_decimal", |p| {
        p.monetary_budget.amount = ".1".into()
    });
    push("money", "trailing_decimal", |p| {
        p.monetary_budget.amount = "1.".into()
    });
    push("money", "overflow", |p| {
        p.monetary_budget.amount = u128::MAX.to_string()
    });
    push("money", "empty_currency", |p| {
        p.monetary_budget.currency.clear()
    });
    push("money", "bad_currency", |p| {
        p.monetary_budget.currency = format!("{SECRET}/currency")
    });
    push("money", "empty_pricing", |p| {
        p.monetary_budget.pricing_version.clear()
    });
    push("money", "blank_pricing", |p| {
        p.monetary_budget.pricing_version = " \n ".into()
    });
    push("money", "long_pricing", |p| {
        p.monetary_budget.pricing_version = "v".repeat(129)
    });
    push("money", "nul_pricing", |p| {
        p.monetary_budget.pricing_version = "v\0".into()
    });
    push("metadata", "missing_timestamp", |p| p.frozen_at = None);
    for (field, change) in [
        (
            "min_effect",
            (|p: &mut ExperimentPlan, v| p.min_effect = v) as fn(&mut ExperimentPlan, f64),
        ),
        ("noninferior_bound", |p: &mut ExperimentPlan, v| {
            p.noninferior_bound = v
        }),
        ("savings_ratio", |p: &mut ExperimentPlan, v| {
            p.savings_ratio = v
        }),
        ("max_cost_ratio", |p: &mut ExperimentPlan, v| {
            p.max_cost_ratio = v
        }),
        ("max_p95_latency_ratio", |p: &mut ExperimentPlan, v| {
            p.max_p95_latency_ratio = v
        }),
        ("alpha_total", |p: &mut ExperimentPlan, v| p.alpha_total = v),
    ] {
        for (name, value) in [
            ("nan", f64::NAN),
            ("positive_inf", f64::INFINITY),
            ("negative_inf", f64::NEG_INFINITY),
        ] {
            let mut plan = frozen_plan();
            change(&mut plan, value);
            cases.push(BadPlan {
                axis: "nonfinite",
                name: format!("{field}_{name}"),
                plan,
            });
        }
    }
    cases
}

fn assert_open_axis(axis: &str) {
    let mut failures = Vec::new();
    let cases: Vec<_> = invalid_plans()
        .into_iter()
        .filter(|case| case.axis == axis)
        .collect();
    assert!(!cases.is_empty());
    for case in &cases {
        let expected = case.plan.validate().unwrap_err();
        let before = format!("{:?}", case.plan);
        for repeat in 0..2 {
            match QueryBook::open(&case.plan) {
                Err(error) if same_error(&expected, &error) => {
                    assert!(!error.to_string().contains(SECRET))
                }
                actual => failures.push(format!(
                    "{}/{} #{repeat}: expected {expected:?}, got {actual:?}",
                    case.axis, case.name
                )),
            }
        }
        assert_eq!(format!("{:?}", case.plan), before);
    }
    assert!(
        failures.is_empty(),
        "{axis}: {} cases, {} wrong results:\n{}",
        cases.len(),
        failures.len(),
        failures.join("\n")
    );
}

macro_rules! open_axis {
    ($name:ident, $axis:literal) => {
        #[test]
        fn $name() {
            assert_open_axis($axis);
        }
    };
}

open_axis!(open_reuses_existing_version_errors, "versions");
open_axis!(open_reuses_existing_identifier_errors, "identifiers");
open_axis!(open_reuses_existing_condition_errors, "conditions");
open_axis!(open_reuses_existing_bound_errors, "bounds");
open_axis!(open_reuses_existing_nonfinite_errors, "nonfinite");
open_axis!(open_reuses_existing_money_errors, "money");
open_axis!(open_reuses_existing_freeze_metadata_errors, "metadata");

#[test]
fn open_preserves_frozen_then_bound_candidate_error_priority() {
    let mut plan = frozen_plan();
    plan.schema_version = SECRET.into();
    plan.frozen = false;
    plan.candidate_digest = None;
    assert_error(
        Error::Invalid("query book requires a frozen plan".into()),
        QueryBook::open(&plan).unwrap_err(),
    );
    plan.frozen = true;
    assert_error(
        Error::Invalid("query book requires a bound candidate".into()),
        QueryBook::open(&plan).unwrap_err(),
    );
}

#[test]
fn existing_valid_shapes_and_money_lexemes_remain_accepted() {
    for amount in [
        "0",
        "+0",
        "00.00000000",
        " 0 ",
        "1",
        "+1.2300",
        "001.0",
        "-1",
        " -2.50000000 ",
        "0.00000001",
    ] {
        let mut plan = frozen_plan();
        // Legacy validation permits these quantities; editing a frozen fixture
        // does not establish a real funding authorization.
        plan.monetary_budget = Money {
            currency: "USD:legacy".into(),
            pricing_version: " 定价 v1 ".into(),
            amount: amount.into(),
        };
        plan.validate().unwrap();
        let before = serde_json::to_vec(&plan).unwrap();
        let mut book = QueryBook::open(&plan).unwrap();
        book.reserve(ticket(&plan, "valid-money")).unwrap();
        assert_eq!(book.used(), 1);
        assert_eq!(serde_json::to_vec(&plan).unwrap(), before);
    }
    for n in [2, 100_000] {
        for limit in [1, 20] {
            let mut plan = frozen_plan();
            plan.n_planned = n;
            plan.query_limit = limit;
            plan.id = format!(".:_-{}", "p".repeat(124));
            plan.dataset_epoch = ".:_-".into();
            plan.candidate_digest = Some(".:_-".into());
            plan.first_round = false;
            plan.conditions = vec![
                ControlCondition::FrozenA,
                ControlCondition::MemoryOnlyB0,
                ControlCondition::StaticHarnessB1,
                ControlCondition::FixedImproverC,
                ControlCondition::EvolvingImproverD,
            ];
            plan.stats_version = STATS_HOEFFDING_V1.into();
            plan.profile = ProfileKind::NoninferiorSavings;
            plan.min_effect = 1.0;
            plan.noninferior_bound = -1.0;
            plan.savings_ratio = 1.0;
            plan.max_cost_ratio = 0.1;
            plan.max_p95_latency_ratio = 10.0;
            plan.alpha_total = 0.0001;
            plan.frozen_at = Some(-1);
            plan.validate().unwrap();
            let mut book = QueryBook::open(&plan).unwrap();
            book.reserve(ticket(&plan, "valid-bounds")).unwrap();
            assert_eq!(book.used(), 1);
        }
    }
}

#[test]
fn nonzero_funding_still_cannot_freeze_and_zero_lexemes_can() {
    for amount in ["1", "+1.0000", "-1", " 1 "] {
        let mut plan = ExperimentPlan::first_low_risk("unfunded", 60).unwrap();
        plan.monetary_budget.amount = amount.into();
        let before = serde_json::to_vec(&plan).unwrap();
        assert_error(Error::Budget, plan.freeze(123).unwrap_err());
        assert_eq!(serde_json::to_vec(&plan).unwrap(), before);
    }
    for amount in ["0", "+0", "000.00000000", " 0 "] {
        let mut plan = ExperimentPlan::first_low_risk("zero", 60).unwrap();
        plan.monetary_budget.amount = amount.into();
        plan.freeze(123).unwrap();
        assert!(plan.frozen);
        assert_eq!(plan.monetary_budget.amount, amount);
    }
}

#[test]
fn first_wrong_epoch_is_refused_without_partial_write() {
    let plan = frozen_plan();
    let mut book = QueryBook::open(&plan).unwrap();
    let good = ticket(&plan, "first");
    let mut wrong = good.clone();
    wrong.dataset_epoch = format!("{SECRET}_epoch");
    let before = format!("{book:?}");
    let result = book.reserve(wrong);
    assert_error(Error::Conflict(EPOCH_ERROR.into()), result.unwrap_err());
    assert_eq!(format!("{book:?}"), before);
    assert_eq!(book.used(), 0);
    book.reserve(good).unwrap();
    assert_eq!(book.used(), 1);
}

#[test]
fn reserved_wrong_epoch_is_refused_without_replacing_original() {
    let plan = frozen_plan();
    let mut book = QueryBook::open(&plan).unwrap();
    let good = ticket(&plan, "reserved");
    book.reserve(good.clone()).unwrap();
    let mut wrong = good.clone();
    wrong.dataset_epoch = format!("{SECRET}_epoch");
    let before = format!("{book:?}");
    assert_error(
        Error::Conflict(EPOCH_ERROR.into()),
        book.reserve(wrong).unwrap_err(),
    );
    assert_eq!(format!("{book:?}"), before);
    assert_eq!(book.used(), 1);
    book.reserve(good).unwrap();
    assert_eq!(format!("{book:?}"), before);
}

#[test]
fn reserve_preserves_legacy_errors_and_defines_epoch_precedence() {
    let plan = frozen_plan();
    let good = ticket(&plan, "priority");
    let mut book = QueryBook::open(&plan).unwrap();
    book.reserve(good.clone()).unwrap();
    let unchanged = format!("{book:?}");
    type TicketMutation = fn(&mut QueryTicket);
    let mutations: [(TicketMutation, Error); 6] = [
        (
            |t| t.id.clear(),
            Error::Invalid("identifier: expected nonempty text <= 128 bytes".into()),
        ),
        (
            |t| t.seed.clear(),
            Error::Invalid("identifier: expected nonempty text <= 128 bytes".into()),
        ),
        (
            |t| t.dataset_epoch = format!("{SECRET}/epoch"),
            Error::Invalid("invalid identifier".into()),
        ),
        (
            |t| t.plan_digest = SECRET.into(),
            Error::Conflict("ticket does not match frozen plan/candidate".into()),
        ),
        (
            |t| t.candidate_digest = SECRET.into(),
            Error::Conflict("ticket does not match frozen plan/candidate".into()),
        ),
        (
            |t| t.seed = "other-seed".into(),
            Error::Conflict("ticket cannot change seed".into()),
        ),
    ];
    for (mutate, expected) in mutations {
        let mut changed = good.clone();
        mutate(&mut changed);
        assert_error(expected, book.reserve(changed).unwrap_err());
        assert_eq!(format!("{book:?}"), unchanged);
    }
    let mut wrong = good.clone();
    wrong.dataset_epoch = format!("{SECRET}_epoch");
    wrong.seed = "different-seed".into();
    assert_error(
        Error::Conflict(EPOCH_ERROR.into()),
        book.reserve(wrong.clone()).unwrap_err(),
    );
    assert_eq!(format!("{book:?}"), unchanged);
    wrong.plan_digest = SECRET.into();
    assert_error(
        Error::Conflict("ticket does not match frozen plan/candidate".into()),
        book.reserve(wrong).unwrap_err(),
    );
    assert_eq!(format!("{book:?}"), unchanged);
}

#[test]
fn reserved_idempotency_survives_full_limit_without_refund() {
    let mut plan = frozen_plan();
    plan.query_limit = 1;
    let good = ticket(&plan, "only");
    let mut book = QueryBook::open(&plan).unwrap();
    book.reserve(good.clone()).unwrap();
    let full = format!("{book:?}");
    book.reserve(good.clone()).unwrap();
    assert_eq!(format!("{book:?}"), full);
    assert_error(
        Error::Budget,
        book.reserve(ticket(&plan, "extra")).unwrap_err(),
    );
    assert_eq!(format!("{book:?}"), full);
    let mut wrong = ticket(&plan, "extra");
    wrong.dataset_epoch = format!("{SECRET}_epoch");
    assert_error(
        Error::Conflict(EPOCH_ERROR.into()),
        book.reserve(wrong).unwrap_err(),
    );
    assert_eq!(format!("{book:?}"), full);
    book.cancel(&good.id).unwrap();
    assert_eq!(book.used(), 1);
    assert_error(
        Error::Budget,
        book.reserve(ticket(&plan, "after-cancel")).unwrap_err(),
    );
}

#[test]
fn terminal_tickets_keep_legacy_errors_and_used_count() {
    for terminal in 0..3 {
        let plan = frozen_plan();
        let good = ticket(&plan, "terminal");
        let mut book = QueryBook::open(&plan).unwrap();
        book.reserve(good.clone()).unwrap();
        match terminal {
            0 => book.complete(&good.id),
            1 => book.fail(&good.id),
            _ => book.cancel(&good.id),
        }
        .unwrap();
        let before = format!("{book:?}");
        let reason = if terminal == 0 {
            "ticket already consumed; replay the stored result"
        } else {
            "failed or cancelled tickets do not refund a new seed"
        };
        assert_error(
            Error::Conflict(reason.into()),
            book.reserve(good.clone()).unwrap_err(),
        );
        let mut changed = good;
        changed.seed = "new-seed".into();
        assert_error(
            Error::Conflict("ticket cannot change seed".into()),
            book.reserve(changed).unwrap_err(),
        );
        for finish in [QueryBook::complete, QueryBook::fail, QueryBook::cancel] {
            assert_error(
                Error::Conflict("ticket is not reserved".into()),
                finish(&mut book, "terminal").unwrap_err(),
            );
        }
        assert_eq!(book.used(), 1);
        assert_eq!(format!("{book:?}"), before);
        book.reserve(ticket(&plan, "new-id")).unwrap();
        assert_eq!(book.used(), 2);
    }
}

#[test]
fn unknown_completion_ids_keep_not_found_without_any_write() {
    let plan = frozen_plan();
    let mut book = QueryBook::open(&plan).unwrap();
    let before = format!("{book:?}");
    for finish in [QueryBook::complete, QueryBook::fail, QueryBook::cancel] {
        assert_error(Error::NotFound, finish(&mut book, "unknown").unwrap_err());
        assert_eq!(format!("{book:?}"), before);
    }
    book.reserve(ticket(&plan, "next")).unwrap();
    assert_eq!(book.used(), 1);
}

#[derive(Serialize)]
struct WireEntry {
    plan: ExperimentPlan,
    plan_digest: String,
    ticket: QueryTicket,
}

fn valid_wire() -> Vec<u8> {
    let mut entries = Vec::new();
    for (index, amount) in ["0", "+0", " 0 ", "001.000", "-2.50"]
        .into_iter()
        .enumerate()
    {
        for stats in [evo_core::evaluation::STATS_BERNSTEIN_V2, STATS_HOEFFDING_V1] {
            let mut plan = frozen_plan();
            plan.id = format!("wire-{index}-{stats}");
            plan.monetary_budget.amount = amount.into();
            plan.stats_version = stats.into();
            plan.validate().unwrap();
            let mut book = QueryBook::open(&plan).unwrap();
            let value = ticket(&plan, "wire-ticket");
            book.reserve(value.clone()).unwrap();
            entries.push(WireEntry {
                plan_digest: plan.digest().unwrap(),
                plan,
                ticket: value,
            });
        }
    }
    assert_eq!(entries.len(), 10);
    serde_json::to_vec(&entries).unwrap()
}

// Frozen from the unchanged public-API generator actually run on parent 212c784.
#[test]
fn frozen_parent_wire_matches_legacy_bytes() {
    let wire = valid_wire();
    assert_eq!(
        hash(&wire),
        "4d2a8e2fd0727189c1e15d636b6b091d2da4bda1168cc34968071eb91c9e0f6e"
    );
    println!("AG080_CORE_WIRE_BEGIN");
    println!("{}", String::from_utf8(wire).unwrap());
    println!("AG080_CORE_WIRE_END");
}
