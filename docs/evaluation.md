# Evaluation plan (E01)

This file freezes the first **unfunded** experiment contract. Numbers are design starting values, not measured effects. HCI is not a run trigger. SPRT does not replace statistics v1.

## Frozen choices (before any candidate generation)

| Field | Value |
|---|---|
| plan schema | `rsia.experiment_plan.v1` |
| objective_version | `rsia.attainment_auc.v1` |
| stats_version (new) | `rsia.empirical_bernstein.v2` |
| stats_version (kept) | `rsia.hoeffding.v1` via existing `evaluate` |
| profile | `quality_gain` (mutually exclusive with `noninferior_savings`) |
| min_effect | 0.02 |
| noninferior_bound | −0.01 |
| savings_ratio | 0.10 |
| max_cost_ratio | 1.10 |
| max_p95_latency_ratio | 1.20 |
| alpha_total | 0.05, allocated once |
| first-round conditions | A / B1 / C only |
| independent unit | task cluster (`d_i = q_new − q_old ∈ [−1,1]`) |
| monetary_budget | `0` (any non-zero amount needs admin authorization) |
| paid pilot | not authorized this round; estimator scripts only |

## Data uses

`development`, `replay_train` / `replay_select`, `acceptance_epoch`, `operational_monitoring`.

A family, parent, or near-duplicate must not cross development and `acceptance_epoch`. Dev-pilot rows never enter the formal holdout.

## Stop rules

- Safety / directly harmful behavior: stop without waiting for significance.
- If estimated `n` and the zero monetary budget are incompatible, do **not** lower thresholds; mark the formal experiment infeasible.
- Failed, cancelled, or missing-row tickets do not refund a new seed.

## What this does not claim

Estimator output and Bernstein goldens test the implementation. They are not product benefit, host acceptance, or a paid study.
