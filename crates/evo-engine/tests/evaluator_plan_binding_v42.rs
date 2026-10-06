//! Legacy synthetic grader contracts; booleans here do not attest real execution.

use evo_core::evaluation::{
    ClusterObservation, ExperimentPlan, ProfileKind, QueryBook, QueryTicket, STATS_HOEFFDING_V1,
    Verdict,
};
use evo_core::{Context, Error, Role, hash};
use evo_engine::evaluator::{
    Evaluator, ExecutionReport, FormalEvaluation, GradeRequest, evaluation_identity,
};
use serde::Serialize;

const SECRET: &str = "AG080_SECRET";
const BINDING_ERROR: &str = "experiment plan does not match query book";
const CANDIDATE_ERROR: &str = "candidate does not match frozen plan";

fn context(role: Role) -> Context {
    Context::new("ag080-namespace", "synthetic-grader", role).unwrap()
}

fn frozen_plan() -> ExperimentPlan {
    let mut plan = ExperimentPlan::first_low_risk("ag080-engine-plan", 60).unwrap();
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

fn execution(id: &str) -> ExecutionReport {
    ExecutionReport {
        ticket_id: id.into(),
        environment_digest: "synthetic-environment".into(),
        outputs_complete: true,
        units: 1,
    }
}

fn rows(count: usize, effect: f64) -> Vec<ClusterObservation> {
    (0..count)
        .map(|i| ClusterObservation {
            cluster_id: format!("cluster-{i}"),
            d: effect,
            weight: 1.0,
        })
        .collect()
}

fn grade(
    ctx: &Context,
    plan: &ExperimentPlan,
    book: &mut QueryBook,
    value: &QueryTicket,
    exec: &ExecutionReport,
    observations: &[ClusterObservation],
) -> evo_core::Result<FormalEvaluation> {
    Evaluator::grade(
        ctx,
        GradeRequest {
            plan,
            book,
            ticket_id: &value.id,
            exec,
            rows: observations,
            alpha_i: 0.05,
            cost_ratio: 1.0,
            p95_ratio: 1.0,
            safety_ok: true,
        },
    )
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

fn altered_plans(plan: &ExperimentPlan) -> Vec<(&'static str, ExperimentPlan)> {
    type PlanMutation = fn(&mut ExperimentPlan);
    let changes: [(&str, PlanMutation); 20] = [
        ("min_effect", |p| p.min_effect = 0.03),
        ("noninferior_bound", |p| p.noninferior_bound = -0.02),
        ("savings_ratio", |p| p.savings_ratio = 0.2),
        ("max_cost_ratio", |p| p.max_cost_ratio = 1.5),
        ("max_p95_latency_ratio", |p| p.max_p95_latency_ratio = 1.5),
        ("alpha_total", |p| p.alpha_total = 0.03),
        ("n", |p| p.n_planned = 137),
        ("query_limit", |p| p.query_limit = 5),
        ("money_lexeme", |p| p.monetary_budget.amount = "+0".into()),
        ("money_nonzero", |p| {
            p.monetary_budget.amount = "1.000".into()
        }),
        ("money_negative", |p| p.monetary_budget.amount = "-1".into()),
        ("currency", |p| p.monetary_budget.currency = "EUR".into()),
        ("pricing", |p| {
            p.monetary_budget.pricing_version = "different-pricing".into()
        }),
        ("epoch", |p| p.dataset_epoch = format!("{SECRET}_epoch")),
        ("candidate", |p| {
            p.candidate_digest = Some(format!("{SECRET}_candidate"))
        }),
        ("timestamp", |p| p.frozen_at = Some(124)),
        ("stop_on_harm", |p| p.stop_on_harm = false),
        ("profile", |p| p.profile = ProfileKind::NoninferiorSavings),
        ("condition_order", |p| p.conditions.reverse()),
        ("first_round", |p| p.first_round = false),
    ];
    changes
        .into_iter()
        .map(|(name, change)| {
            let mut changed = plan.clone();
            change(&mut changed);
            changed.validate().unwrap();
            assert_ne!(changed.digest().unwrap(), plan.digest().unwrap());
            (name, changed)
        })
        .collect()
}

#[test]
fn start_refuses_every_changed_legal_plan_without_any_write() {
    let original = frozen_plan();
    let value = ticket(&original, "start");
    let ctx = context(Role::Evaluator);
    let mut failures = Vec::new();
    for (name, mut changed) in altered_plans(&original) {
        let mut book = QueryBook::open(&original).unwrap();
        let before_book = format!("{book:?}");
        let before_plan = serde_json::to_vec(&changed).unwrap();
        let candidate = changed.candidate_digest.clone().unwrap();
        let result = Evaluator::start(&ctx, &mut changed, &candidate, value.clone(), &mut book);
        match result {
            Err(error) if same_error(&Error::Conflict(BINDING_ERROR.into()), &error) => {
                assert!(!error.to_string().contains(SECRET))
            }
            actual => failures.push(format!(
                "{name}: expected binding refusal, got {actual:?}; current digest={}, original={}",
                changed.digest().unwrap(),
                original.digest().unwrap()
            )),
        }
        if format!("{book:?}") != before_book {
            failures.push(format!("{name}: refused start changed book to {book:?}"));
        }
        assert_eq!(serde_json::to_vec(&changed).unwrap(), before_plan);
        let mut good_plan = original.clone();
        Evaluator::start(
            &ctx,
            &mut good_plan,
            "candidate-a",
            value.clone(),
            &mut book,
        )
        .unwrap();
        assert_eq!(book.used(), 1);
    }
    assert!(
        failures.is_empty(),
        "all 20 changed plans were tested:\n{}",
        failures.join("\n")
    );
}

#[test]
fn an_already_bound_plan_cannot_ignore_a_different_candidate_argument() {
    let mut plan = frozen_plan();
    let value = ticket(&plan, "candidate-argument");
    let mut book = QueryBook::open(&plan).unwrap();
    let before_plan = serde_json::to_vec(&plan).unwrap();
    let before_book = format!("{book:?}");
    assert_error(
        Error::Conflict(CANDIDATE_ERROR.into()),
        Evaluator::start(
            &context(Role::Evaluator),
            &mut plan,
            &format!("{SECRET}_candidate"),
            value.clone(),
            &mut book,
        )
        .unwrap_err(),
    );
    assert_eq!(serde_json::to_vec(&plan).unwrap(), before_plan);
    assert_eq!(format!("{book:?}"), before_book);
    Evaluator::start(
        &context(Role::Evaluator),
        &mut plan,
        "candidate-a",
        value,
        &mut book,
    )
    .unwrap();
}

#[test]
fn unbound_local_candidate_is_not_committed_after_ticket_refusal() {
    let bound = frozen_plan();
    let mut plan = bound.clone();
    plan.candidate_digest = None;
    let mut book = QueryBook::open(&bound).unwrap();
    let good = ticket(&bound, "unbound");
    let mut bad = good.clone();
    bad.plan_digest = SECRET.into();
    let before = serde_json::to_vec(&plan).unwrap();
    let before_book = format!("{book:?}");
    assert_error(
        Error::Conflict("ticket does not match frozen plan/candidate".into()),
        Evaluator::start(
            &context(Role::Evaluator),
            &mut plan,
            "candidate-a",
            bad,
            &mut book,
        )
        .unwrap_err(),
    );
    assert_eq!(
        serde_json::to_vec(&plan).unwrap(),
        before,
        "failed start must retain None"
    );
    assert_eq!(format!("{book:?}"), before_book);
    Evaluator::start(
        &context(Role::Evaluator),
        &mut plan,
        "candidate-a",
        good,
        &mut book,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&plan).unwrap(),
        serde_json::to_vec(&bound).unwrap()
    );
    assert_eq!(book.used(), 1);
}

#[test]
fn unbound_local_candidate_must_match_the_original_book_before_reserve() {
    let bound = frozen_plan();
    let mut plan = bound.clone();
    plan.candidate_digest = None;
    let mut book = QueryBook::open(&bound).unwrap();
    let value = ticket(&bound, "unbound-candidate");
    let before = serde_json::to_vec(&plan).unwrap();
    let before_book = format!("{book:?}");
    assert_error(
        Error::Conflict(BINDING_ERROR.into()),
        Evaluator::start(
            &context(Role::Evaluator),
            &mut plan,
            &format!("{SECRET}_candidate"),
            value.clone(),
            &mut book,
        )
        .unwrap_err(),
    );
    assert_eq!(serde_json::to_vec(&plan).unwrap(), before);
    assert_eq!(format!("{book:?}"), before_book);
    Evaluator::start(
        &context(Role::Evaluator),
        &mut plan,
        "candidate-a",
        value,
        &mut book,
    )
    .unwrap();
    assert_eq!(book.used(), 1);
}

#[test]
fn unbound_success_and_reserved_idempotency_keep_legacy_outputs() {
    let bound = frozen_plan();
    let mut plan = bound.clone();
    plan.candidate_digest = None;
    let mut book = QueryBook::open(&bound).unwrap();
    let value = ticket(&bound, "unbound-success");
    Evaluator::start(
        &context(Role::Evaluator),
        &mut plan,
        "candidate-a",
        value.clone(),
        &mut book,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&plan).unwrap(),
        serde_json::to_vec(&bound).unwrap()
    );
    let before = format!("{book:?}");
    Evaluator::start(
        &context(Role::Evaluator),
        &mut plan,
        "candidate-a",
        value,
        &mut book,
    )
    .unwrap();
    assert_eq!(format!("{book:?}"), before);
    assert_eq!(book.used(), 1);
}

#[test]
fn start_permission_and_original_frozen_errors_stay_first() {
    let bound = frozen_plan();
    let value = ticket(&bound, "priority");
    for role in [Role::Agent, Role::Host, Role::Admin, Role::Worker] {
        let mut plan = bound.clone();
        plan.frozen = false;
        plan.schema_version = SECRET.into();
        let mut book = QueryBook::open(&bound).unwrap();
        let before = (format!("{plan:?}"), format!("{book:?}"));
        assert_error(
            Error::Forbidden,
            Evaluator::start(&context(role), &mut plan, SECRET, value.clone(), &mut book)
                .unwrap_err(),
        );
        assert_eq!((format!("{plan:?}"), format!("{book:?}")), before);
    }
    let mut plan = bound.clone();
    plan.frozen = false;
    plan.schema_version = SECRET.into();
    let mut book = QueryBook::open(&bound).unwrap();
    assert_error(
        Error::Invalid("plan must be frozen before evaluation".into()),
        Evaluator::start(
            &context(Role::Evaluator),
            &mut plan,
            SECRET,
            value,
            &mut book,
        )
        .unwrap_err(),
    );
    assert_eq!(book.used(), 0);
}

#[test]
fn start_preserves_ticket_and_candidate_identifier_failures_without_write() {
    let bound = frozen_plan();
    let mut book = QueryBook::open(&bound).unwrap();
    let before_book = format!("{book:?}");
    let mut plan = bound.clone();
    let mut value = ticket(&bound, "identifier");
    value.id.clear();
    assert_error(
        Error::Invalid("identifier: expected nonempty text <= 128 bytes".into()),
        Evaluator::start(
            &context(Role::Evaluator),
            &mut plan,
            "candidate-a",
            value,
            &mut book,
        )
        .unwrap_err(),
    );
    assert_eq!(format!("{book:?}"), before_book);
    plan.candidate_digest = None;
    let before_plan = serde_json::to_vec(&plan).unwrap();
    assert_error(
        Error::Invalid("invalid identifier".into()),
        Evaluator::start(
            &context(Role::Evaluator),
            &mut plan,
            &format!("{SECRET}/candidate"),
            ticket(&bound, "identifier"),
            &mut book,
        )
        .unwrap_err(),
    );
    assert_eq!(serde_json::to_vec(&plan).unwrap(), before_plan);
    assert_eq!(format!("{book:?}"), before_book);
}

fn assert_grade_relation_refusal(
    original: &ExperimentPlan,
    changed: &ExperimentPlan,
    name: &str,
) -> Vec<String> {
    let mut failures = Vec::new();
    let ctx = context(Role::Evaluator);
    let mut plan = original.clone();
    let mut book = QueryBook::open(&plan).unwrap();
    let value = ticket(&plan, "grade");
    Evaluator::start(&ctx, &mut plan, "candidate-a", value.clone(), &mut book).unwrap();
    assert_eq!(book.used(), 1);
    let before = serde_json::to_vec(changed).unwrap();
    let exec = execution(&value.id);
    let observations = rows(60, 0.0);
    match grade(&ctx, changed, &mut book, &value, &exec, &observations) {
        Err(error) if same_error(&Error::Conflict(BINDING_ERROR.into()), &error) => {
            assert!(!error.to_string().contains(SECRET))
        }
        actual => failures.push(format!(
            "{name}: expected binding refusal, got {actual:?}; frozen digest={}",
            original.digest().unwrap()
        )),
    }
    assert_eq!(serde_json::to_vec(changed).unwrap(), before);
    assert_eq!(
        book.used(),
        1,
        "relationship failure cannot refund the query"
    );
    if !format!("{book:?}").contains("state: Failed") {
        failures.push(format!(
            "{name}: relationship failure did not fail the ticket: {book:?}"
        ));
    }
    match book.reserve(value.clone()) {
        Err(error)
            if same_error(
                &Error::Conflict("failed or cancelled tickets do not refund a new seed".into()),
                &error,
            ) => {}
        actual => failures.push(format!(
            "{name}: failed ticket was not preserved: {actual:?}"
        )),
    }
    assert_error(
        Error::Conflict("ticket is not reserved".into()),
        grade(&ctx, original, &mut book, &value, &exec, &observations).unwrap_err(),
    );
    assert_error(
        Error::Conflict("ticket is not reserved".into()),
        book.complete(&value.id).unwrap_err(),
    );
    assert_error(
        Error::Conflict("ticket is not reserved".into()),
        book.fail(&value.id).unwrap_err(),
    );
    book.reserve(ticket(original, "following")).unwrap();
    assert_eq!(book.used(), 2);
    failures
}

#[test]
fn grade_refuses_every_changed_legal_plan_and_does_not_refund() {
    let original = frozen_plan();
    let mut failures = Vec::new();
    for (name, changed) in altered_plans(&original) {
        failures.extend(assert_grade_relation_refusal(&original, &changed, name));
    }
    assert!(
        failures.is_empty(),
        "all 20 changed plans were tested:\n{}",
        failures.join("\n")
    );
}

#[test]
fn grade_refuses_a_separate_complete_same_shape_plan() {
    let original = frozen_plan();
    let mut replacement =
        ExperimentPlan::first_low_risk("replacement-plan", original.n_planned).unwrap();
    replacement.freeze(original.frozen_at.unwrap()).unwrap();
    replacement.bind_candidate("candidate-a").unwrap();
    replacement.validate().unwrap();
    assert_ne!(replacement.digest().unwrap(), original.digest().unwrap());
    let failures = assert_grade_relation_refusal(&original, &replacement, "complete replacement");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn grade_original_execution_refusals_precede_plan_binding() {
    for incomplete in [false, true] {
        let mut plan = frozen_plan();
        let value = ticket(&plan, "exec-priority");
        let mut book = QueryBook::open(&plan).unwrap();
        Evaluator::start(
            &context(Role::Evaluator),
            &mut plan,
            "candidate-a",
            value.clone(),
            &mut book,
        )
        .unwrap();
        let mut changed = plan.clone();
        changed.min_effect = 0.03;
        changed.schema_version = SECRET.into();
        let mut exec = execution(&value.id);
        let expected = if incomplete {
            exec.outputs_complete = false;
            Error::Invalid("incomplete execution is invalid".into())
        } else {
            exec.ticket_id = "different-ticket".into();
            Error::Conflict("execution report is bound to a different ticket".into())
        };
        assert_error(
            expected,
            grade(
                &context(Role::Evaluator),
                &changed,
                &mut book,
                &value,
                &exec,
                &[],
            )
            .unwrap_err(),
        );
        assert_eq!(book.used(), 1);
        assert!(format!("{book:?}").contains("state: Failed"));
        assert_error(
            Error::Conflict("failed or cancelled tickets do not refund a new seed".into()),
            book.reserve(value).unwrap_err(),
        );
    }
}

#[test]
fn grade_permissions_precede_all_plan_execution_and_statistics_checks() {
    for role in [Role::Agent, Role::Host, Role::Admin, Role::Worker] {
        let plan = frozen_plan();
        let value = ticket(&plan, "grade-role");
        let mut book = QueryBook::open(&plan).unwrap();
        book.reserve(value.clone()).unwrap();
        let mut changed = plan.clone();
        changed.schema_version = SECRET.into();
        let mut exec = execution("different-ticket");
        exec.outputs_complete = false;
        let before = format!("{book:?}");
        assert_error(
            Error::Forbidden,
            grade(&context(role), &changed, &mut book, &value, &exec, &[]).unwrap_err(),
        );
        assert_eq!(format!("{book:?}"), before);
        assert_eq!(book.used(), 1);
    }
}

#[test]
fn grade_preserves_duplicate_cluster_failure_without_refund() {
    let plan = frozen_plan();
    let value = ticket(&plan, "duplicates");
    let mut book = QueryBook::open(&plan).unwrap();
    book.reserve(value.clone()).unwrap();
    let mut observations = rows(2, 0.0);
    observations[1].cluster_id = observations[0].cluster_id.clone();
    assert_error(
        Error::Invalid("duplicate cluster, not independent evidence".into()),
        grade(
            &context(Role::Evaluator),
            &plan,
            &mut book,
            &value,
            &execution(&value.id),
            &observations,
        )
        .unwrap_err(),
    );
    assert!(format!("{book:?}").contains("state: Failed"));
    assert_eq!(book.used(), 1);
}

#[test]
fn grade_unknown_and_terminal_ids_keep_original_errors() {
    for state in 0..4 {
        for changed in [false, true] {
            let plan = frozen_plan();
            let value = ticket(&plan, "state");
            let mut book = QueryBook::open(&plan).unwrap();
            if state != 0 {
                book.reserve(value.clone()).unwrap();
                match state {
                    1 => book.complete(&value.id),
                    2 => book.fail(&value.id),
                    _ => book.cancel(&value.id),
                }
                .unwrap();
            }
            let before = format!("{book:?}");
            let mut current = plan.clone();
            if changed {
                current.min_effect = 0.03;
            }
            let expected = if state == 0 {
                Error::NotFound
            } else {
                Error::Conflict("ticket is not reserved".into())
            };
            assert_error(
                expected,
                grade(
                    &context(Role::Evaluator),
                    &current,
                    &mut book,
                    &value,
                    &execution(&value.id),
                    &rows(60, 0.0),
                )
                .unwrap_err(),
            );
            assert_eq!(format!("{book:?}"), before);
            assert_eq!(book.used(), u32::from(state != 0));
        }
    }
}

#[test]
fn unchanged_plan_serde_roundtrip_keeps_success_and_terminal_count() {
    let plan = frozen_plan();
    let mut decoded: ExperimentPlan =
        serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let value = ticket(&plan, "roundtrip");
    let mut book = QueryBook::open(&plan).unwrap();
    Evaluator::start(
        &context(Role::Evaluator),
        &mut decoded,
        "candidate-a",
        value.clone(),
        &mut book,
    )
    .unwrap();
    let formal = grade(
        &context(Role::Evaluator),
        &decoded,
        &mut book,
        &value,
        &execution(&value.id),
        &rows(60, 0.0),
    )
    .unwrap();
    assert_eq!(formal.plan_digest, plan.digest().unwrap());
    assert_eq!(formal.candidate_digest, "candidate-a");
    assert_eq!(formal.verdict, Verdict::Inconclusive);
    let wire = serde_json::to_vec(&formal).unwrap();
    let again: FormalEvaluation = serde_json::from_slice(&wire).unwrap();
    assert_eq!(serde_json::to_vec(&again).unwrap(), wire);
    assert_error(
        Error::Conflict("ticket already consumed; replay the stored result".into()),
        book.reserve(value).unwrap_err(),
    );
    assert_eq!(book.used(), 1);
}

#[test]
fn legacy_v1_statistics_refusal_and_invalid_reports_are_preserved() {
    let mut plan = frozen_plan();
    plan.stats_version = STATS_HOEFFDING_V1.into();
    let value = ticket(&plan, "v1");
    let mut book = QueryBook::open(&plan).unwrap();
    book.reserve(value.clone()).unwrap();
    assert_error(
        Error::Invalid("v2 decision requires empirical_bernstein.v2".into()),
        grade(
            &context(Role::Evaluator),
            &plan,
            &mut book,
            &value,
            &execution(&value.id),
            &rows(60, 0.0),
        )
        .unwrap_err(),
    );
    assert_eq!(book.used(), 1);
    // Preserve the existing algorithm's Invalid report for n<2 rather than
    // claiming that this relationship fix supplies a new statistical policy.
    let plan = frozen_plan();
    let value = ticket(&plan, "invalid-n");
    let mut book = QueryBook::open(&plan).unwrap();
    book.reserve(value.clone()).unwrap();
    let formal = grade(
        &context(Role::Evaluator),
        &plan,
        &mut book,
        &value,
        &execution(&value.id),
        &rows(1, 0.0),
    )
    .unwrap();
    assert_eq!(formal.verdict, Verdict::Invalid);
    assert_eq!(formal.report.n, 0);
    assert_eq!(book.used(), 1);
}

#[derive(Serialize)]
struct WireEntry {
    scenario: &'static str,
    plan: ExperimentPlan,
    ticket: QueryTicket,
    execution: ExecutionReport,
    formal: FormalEvaluation,
    formal_digest: String,
}

fn valid_wire() -> Vec<u8> {
    let mut entries = Vec::new();
    for amount in ["0", "+0", " 0 ", "001.000", "-2.50"] {
        for (name, effect, n, profile, safe, cost) in [
            ("zero", 0.0, 60, ProfileKind::QualityGain, true, 1.0),
            ("gain", 1.0, 60, ProfileKind::QualityGain, true, 1.0),
            ("regression", -1.0, 60, ProfileKind::QualityGain, true, 1.0),
            ("safety", 1.0, 60, ProfileKind::QualityGain, false, 1.0),
            (
                "noninferior",
                1.0,
                60,
                ProfileKind::NoninferiorSavings,
                true,
                0.5,
            ),
            ("invalid_n", 0.0, 1, ProfileKind::QualityGain, true, 1.0),
        ] {
            let mut plan = frozen_plan();
            plan.profile = profile;
            plan.monetary_budget.amount = amount.into();
            plan.validate().unwrap();
            let value = ticket(&plan, name);
            let mut book = QueryBook::open(&plan).unwrap();
            Evaluator::start(
                &context(Role::Evaluator),
                &mut plan,
                "candidate-a",
                value.clone(),
                &mut book,
            )
            .unwrap();
            let exec = execution(&value.id);
            let observations = rows(n, effect);
            let formal = Evaluator::grade(
                &context(Role::Evaluator),
                GradeRequest {
                    plan: &plan,
                    book: &mut book,
                    ticket_id: &value.id,
                    exec: &exec,
                    rows: &observations,
                    alpha_i: 0.05,
                    cost_ratio: cost,
                    p95_ratio: 1.0,
                    safety_ok: safe,
                },
            )
            .unwrap();
            assert_eq!(book.used(), 1);
            entries.push(WireEntry {
                scenario: name,
                formal_digest: evaluation_identity(&formal).unwrap(),
                plan,
                ticket: value,
                execution: exec,
                formal,
            });
        }
    }
    assert_eq!(entries.len(), 30);
    serde_json::to_vec(&entries).unwrap()
}

// Frozen from the unchanged public-API generator actually run on parent 212c784.
#[test]
fn frozen_parent_wire_matches_legacy_bytes() {
    let wire = valid_wire();
    assert_eq!(
        hash(&wire),
        "c5d7cbcc4fd9eb60aee287e5349e251fd3e01d49ba287d55d49895393a749d1e"
    );
    println!("AG080_ENGINE_WIRE_BEGIN");
    println!("{}", String::from_utf8(wire).unwrap());
    println!("AG080_ENGINE_WIRE_END");
}
