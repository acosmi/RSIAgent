# Evaluation plan (E01)

This is an **unfunded preparation document**, describing what E01 must register before a real experiment. It does not freeze the first task profile, deterministic oracle, answers, anchors, sampling distribution, or payment authorization. Those inputs still need to be supplied and reviewed. The tables and templates below are incomplete documentation, not a runnable profile or evidence that data are isolated. Missing registration values block candidate generation and formal execution.

## Registration and starting values

Before any candidate generation, freeze the main hypothesis and comparison, task specification, versioned success/failure and scoring rules, independent unit and assumptions, source distribution and family weights, data uses, context limits, candidate attempts, sample size, error allocations, stopping rules, anchor coverage, and root cost/latency limits. The candidate and baseline must subsequently be frozen before the independent broker issues a protected holdout manifest. Changing the optimizer implementation, model, distribution, oracle, or scoring rule requires a new version and declared scope.

The existing low-risk helper supplies the following design starting values. They are not measured effects, universal thresholds, or an authorized experiment. E01 must assess and freeze the actual values before seeing candidate results.

| Field | Value | Registration meaning |
|---|---|---|
| plan schema | `rsia.experiment_plan.v1` | Existing serialized plan contract. |
| objective_version | `rsia.attainment_auc.v1` | Freeze the applicable objective. |
| stats_version (new) | `rsia.empirical_bernstein.v2` | Fixed-sample approval method for new plans. |
| stats_version (kept) | `rsia.hoeffding.v1` | Preserve the interpretation of existing v1 experiments. |
| profile | `quality_gain` | Select either this or `noninferior_savings` before results; never combine them after selection. |
| min_effect | 0.02 | Proposed minimum quality gain. |
| noninferior_bound | -0.01 | Proposed quality noninferiority boundary. |
| savings_ratio | 0.10 | Proposed required cost reduction for the savings profile. |
| max_cost_ratio | 1.10 | Proposed quality-gain cost cap relative to the same baseline. |
| max_p95_latency_ratio | 1.20 | Proposed end-to-end tail-latency cap for either profile. |
| alpha_total | 0.05 | One research-family budget shared across attempts and formal claims. |
| n_planned | Explicit caller input, validated in `2..=100000` | E01 determines affordability and power; this validation range supplies neither. |
| query_limit | 3 | Helper starting value; register all planned candidate attempts and queries. |
| first-round conditions | A / B1 / C | A scoped mechanism experiment, not complete five-condition acceptance. |
| monetary_budget | `0` | Nonzero money needs explicit administrator authorization. |
| paid pilot | Not authorized | Prepare estimators only when no funding is authorized. |

`ExperimentPlan::first_low_risk(id, n_planned)` requires the caller's sample size and runs the existing validation without clamping or falling back to 60. This is a breaking Rust source API change: migrate `ExperimentPlan::first_low_risk(id)?` to `ExperimentPlan::first_low_risk(id, n_planned)?`, using the size established by the registered design. Existing structural tests preserving their old value use `ExperimentPlan::first_low_risk("fixture-id", 60)?`; the closed-loop fixture's 60 is a structural example, not a funded or powered formal sample size.

The wire format has not changed. Existing frozen `rsia.experiment_plan.v1` records, including records with 60 or other sample sizes, retain their serialized fields and fingerprints. `n_planned` remains required on deserialization. Do not rewrite historical frozen plans or infer a missing value from the helper.

## Conditions and the comparison being tested

| Condition | Frozen content and permitted changes | Purpose |
|---|---|---|
| A `frozen` | Same base model, tools, and initial system; no experience update across rounds. | Measure performance without evolution. |
| B0 `memory_only` | Retrieve history under a context cap; do not generate improvement methods. | Separate access to extra context from improvement. |
| B1 `static_harness` | A reasonable, manually fixed harness and rules. | Compare with static engineering. |
| C `fixed_improver` | Fixed improver implementation; controlled Skill updates are allowed. | Test the experience-to-Skill loop. |
| D `evolving_improver` | Start from the same fixed C; bounded improver changes may be inherited by the next round. | Compare D with that corresponding frozen C. |

Within C, `C_simple` uses a preregistered simple candidate generator and `C_skillopt` uses the fixed trajectory-analysis, suggestion-aggregation, bounded-editing, and development-selection chain. Match the base model, initial S0, tools, authorized material opportunities, task partitions, effective context cap, and root budget cap. Internal generation counts and actual costs may differ and must all be reported. Do not weaken C_simple or give C_skillopt free reflection material.

D's main control is the fixed C implementation from which D started: D initialized from C_skillopt is compared with frozen C_skillopt. Improvement over a weaker C_simple cannot all be attributed to meta-evolution. Start with the scoped A/B1/C mechanism chain, then the fixed-optimizer comparison when independently sampled and affordable, and later one interpretable ablation at a time. Replay matches historical access opportunities; curriculum comparisons match source opportunities and total experience-acquisition budget. Retain a simple fixed/random-search development baseline where appropriate. Do not expand all switches into a Cartesian product.

## Outcomes, development selection, and valid pairs

Define success and failure in the registered task specification and protected, versioned deterministic oracle/scoring rules. The scorer maps quality `q` into `[0,1]`; family weights and cluster aggregation are frozen in advance. Record quality, instruction-following, environment, selection, and cost failure categories separately. A valid observation compares candidate and baseline on the same frozen task slice, oracle, scoring version, and statistical unit.

Development selection scores, optimizer explanations, model self-scores, text length, token counts, or the best of repeated attempts do not establish formal effect. Optimizer contribution explanations remain hypotheses; controlled development comparisons of original Skill, edited Skill, and local edit withdrawal may investigate them under registered budgets and data uses. The formal result concerns the complete system/Bundle.

A timeout, cancellation, invalid response, or missing member follows the preregistered task-failure rule to form a complete unit, or terminates the experiment as invalid. It cannot be skipped to keep only favorable pairs. Rejected, unchanged, invalid, failed, cancelled, zero/negative, and early-stopped jobs all remain in the attempt denominator and cost account.

## Data partitions and protected holdout

Keep `development`, `replay_train` / `replay_select`, `acceptance_epoch`, and `operational_monitoring` separate. Replay is development use. Formal acceptance data must not enter development prompts, FTS, memory, curriculum, candidate generation, or replay. Only the current task input is supplied under an execution ticket; answers and selected scoring details remain protected.

Partition by original task cluster: repository, template, source, problem family, session, parent/derived tasks, and semantic near-duplicates. Tree relatives cannot cross development and acceptance. Repeating a task, changing its seed, or rewriting a derived task does not create a new independent unit. Record exact hashes, normalized summaries, and lineage; combine near-duplicate screening with human sampling. Different hashes alone do not prove independence. Development-pilot rows never enter the formal holdout.

The protection has two separately reported layers, with **no fixed 20%/80% ratio**:

| Layer | Frozen rule | Gate and sample accounting |
|---|---|---|
| `anchor_suite` | Independent evaluation owner freezes a version covering every required critical constraint and retained capability. Candidates cannot edit or delete anchors. | Each confirmed critical regression rejects promotion; missing coverage is invalid. Repeated anchor runs add no new independent gain samples. |
| `rotating_holdout` | Independent broker samples unexposed clusters from preregistered sources/rules, after candidate and baseline freeze. The slice stays fixed within their comparison. | Formal gain uses only supported independent units. Insufficient independent holdout yields invalid/inconclusive, not a weaker gate. |

A fixed pure generator may be shared with development, but mutable selection state, development examples/seeds, hidden answers, and selection authority may not. Common templates and derived/near-duplicate tasks still inherit their source cluster. Unsupported source/use isolation keeps generated tasks in development or quarantine.

### Anchor coverage worksheet — UNFILLED

This worksheet is a documentation placeholder for `AnchorCoverageMatrix` (`rsia.anchor_coverage_matrix.v1`), not a real frozen matrix. The owner must supply the actual profile/version and every required capability; none is invented here.

| Required registration | Value to supply |
|---|---|
| `profile_id`, `version` | UNFILLED — actual task profile and independently frozen anchor version. |
| `critical_capabilities[].capability_id` | UNFILLED — one row for every required safety constraint or retained capability. |
| `critical_capabilities[].anchor_ids` | UNFILLED — concrete protected anchors covering that capability, with their oracle/rules and evidence. |
| Coverage review and digest | UNFILLED — independent owner verifies complete coverage and records the matrix digest. |
| Per-anchor result and grader receipt | UNFILLED — a confirmed critical failure is a hard veto; dynamic tasks cannot substitute for missing results. |

### Protected HoldoutManifest worksheet — UNFILLED

This is an incomplete documentation template for the existing `HoldoutManifest`, not executable JSON, a new persistent API, or a supplied task/answer set. Register the source rules, weights, independent unit, attempt count, and budget before candidate generation; the independent broker fills and freezes each protected manifest only after the candidate/baseline pair is frozen. It must not choose or replace tasks based on candidate performance.

| Existing field | Value to supply or bind |
|---|---|
| `schema_version` | `rsia.dual_timescale_holdout.v1`; remaining fields are UNFILLED. |
| `research_family_id`, `alpha_plan_digest` | UNFILLED — bind the original family and its once-allocated formal-claim budget. |
| `v1_plan_snapshot_digest`, `candidate_pair_digest` | UNFILLED — bind the frozen plan and corresponding candidate/baseline pair. |
| `sequential_mode` | UNFILLED — explicitly choose `disabled` or reject-only with its frozen early-stop-plan digest. |
| `epoch_id`, `slice_id` | UNFILLED — protected epoch and unexposed slice identity. |
| `ordered_task_cluster_ids`, `source_closure_digests` | UNFILLED — protected task/independent-cluster lineage and source closure in the frozen order. |
| `sampler_version`, `oracle_version` | UNFILLED — independently frozen sampling and deterministic oracle versions. |
| `anchor_coverage_digest`, `grader_digest` | UNFILLED — complete anchor coverage and versioned scoring rules. |
| `sequence_commitment` | UNFILLED — commitment to the broker's preregistered evaluation order. |
| `environment_digest`, `private_seed_commitment` | UNFILLED — fixed environment and private random-seed commitment. |
| `issued_at_unix_ms` | UNFILLED — issue only after candidate and baseline freeze. |

Protect the manifest body, answers, private seeds, and identifiers that reveal answers. Ordinary inspection exposes only authorized summaries, scope, and statistics. Candidates, generation workers, and the development curriculum engine cannot select acceptance tasks, edit answers, or clear exposure records.

### Exposure, rotation, and epochs

Reserve a query before execution, binding research family, candidate/baseline, epoch/slice, clusters, and ticket. Once dispatched or used as selection feedback, the **whole slice** is consumed for this research, including unexecuted members after early stopping. Failed, cancelled, missing-row, or abandoned tickets do not grant a free replacement seed/slice. Only proven undispatched monetary reservations may be released; query/statistical attempt quotas are not refunded.

Do not rotate data or scoring rules during an active comparison. A new candidate attempt receives a new qualified slice; an exhausted epoch closes. A changed environment, target distribution, sampling rule, oracle, or grader needs a new version and applicability scope; absolute scores from incompatible epochs cannot be subtracted as gain. Exposure/contamination history and research-family error budgets persist across epochs, candidate renaming, and schema changes.

Contamination freezes the affected acceptance epoch and removes associated reports' promotion eligibility, with tracing and review of derived/published objects. Do not merely delete a question and retain the old claim. Leaked anchors may remain known regression tests but lose any hidden-test claim; the independent owner versions replacements and preserves reasons, history, and bills.

## Independent units, formal n, and fixed-sample approval

Aggregate each independent cluster to `d_i = q_new,i - q_old,i` in `[-1,1]`, using frozen within-cluster weights. Statistical units are equally weighted; family weighting uses the preregistered sampling distribution or supported independent complete stratified blocks. Both fixed-sample and sequential methods require justified independence and a fixed common-mean distribution. Fields, hashes, seeds, repeated tasks, and generations do not prove these assumptions; report conservative source clustering and coarser-cluster/environment-block sensitivity. Unsupported assumptions prevent a significant-gain claim.

Use **30–60 independent development clusters only for a pilot** of difficulty, variance, and cost. Estimate formal `n_planned` and power from the frozen effect hypothesis, independent unit, variance, error allocation, and available root budget. **60 is not a mandatory or sufficient formal n.** Meta-experiment n counts independent successor streams, not their internal tasks or rounds; three streams over three rounds test orchestration/inheritance structure only.

The new fixed-sample approval method is paired empirical-Bernstein v2. Preserve frozen v1 experiments and their old algorithm/meaning. For independent differences and sample variance `s²`, use `R=2` and the once-allocated one-sided `alpha_i`:

```text
radius = sqrt(2 * s² * ln(2 / alpha_i) / n)
         + 7 * R * ln(2 / alpha_i) / (3 * (n - 1))
LCB = mean(d) - radius
```

Approval uses the complete planned sample, complete anchors, and resource gates. The quality-gain lower bound must exceed the frozen minimum; the savings profile must meet its frozen noninferiority and bounded, reconcilable cost-evidence conditions. Baseline cost zero or unobservable cannot justify a savings ratio. Report tail-latency sample size and uncertainty; a small sample does not guarantee p95.

Sample-size estimators, numeric goldens, boundary/duplicate tests, and fixed-seed zero-effect/regression simulations validate software and its observed implementation error rates, not product gain or guaranteed power for every accepted n. HCI is not a run trigger; SPRT is not a drop-in replacement. If adequate independent units, power, and authorized resources are incompatible, narrow the claim or stop promotion experiments; do not lower thresholds to force a pass.

## Versioned early rejection

New plans explicitly choose `disabled` or `rsia.sequential_reject_only.v1` after the required numerical, simulation, cancellation, and state-machine validation. The helper alone does not enable early stopping. Existing profiles retain their fixed-sample interpretation. Freeze the following supported contract before results; the three thresholds are low-risk design starting values, not universal settings or inferred defaults for every early-stop plan.

| Field | Value | Contract |
|---|---|---|
| early_stop_version | `rsia.sequential_reject_only.v1` | Early rejection only; never early approval. |
| cs_version | `rsia.normal_mixture_bounded.v1` | Versioned two-sided normal-mixture confidence sequence. |
| rho | 1 | Fixed by this confidence-sequence version. |
| R | 2 | Difference range; not a sample-variance estimate. |
| max_regression | 0.01 | Positive proposed regression magnitude; freeze explicitly. |
| noninferiority_margin | 0.01 | Positive proposed noninferiority magnitude; freeze explicitly. |
| min_gain | 0.02 | Positive proposed gain threshold; freeze explicitly. |
| alpha_stop_i | In `(0,1)`, allocated once | Bind the research-family allocation; no per-prefix or per-threshold Bonferroni. |

For complete, verifiable independent pairs in the frozen contiguous prefix:

```text
v_k      = k * R² / 4 = k
log_term = ln(v_k + rho) - ln(rho) - 2 * ln(alpha_stop_i)
r_k      = sqrt((v_k + rho) * log_term) / k
L_k      = max(-1, mean(d_1..d_k) - r_k)
U_k      = min( 1, mean(d_1..d_k) + r_k)
```

At k=0 return collecting. From k=1 only completed pairs/complete registered units advance k. Reject non-finite or out-of-range values and invalid alpha/rho. Use conservative outward numerical bounds, not rounded display values; uncertainty continues collection rather than causing a false stop. Do not repeatedly inspect ordinary fixed-sample intervals to decide stopping, and do not relabel this confidence sequence as SPRT.

The broker freezes unit order and candidate/baseline execution interleaving. Concurrent returns enter only the complete continuous prefix, never arrival/score/latency order. A failed prefix member becomes a complete unit under the registered failure rule or terminates invalid; it cannot be skipped. Apply this priority:

| Priority | Condition | Decision |
|---|---|---|
| 1 | Confirmed critical safety/anchor failure. | `critical_regression`; stop immediately. |
| 2 | `U_k < -max_regression`. | `regressed`, reason `sequential_regression`. |
| 3 | Quality gain and `U_k < min_gain`. | `inconclusive`, reason `futility_quality_gain`. |
| 4 | Noninferior savings and `U_k < -noninferiority_margin`. | `inconclusive`, reason `futility_noninferiority`. |
| 5 | No earlier stop and `k < n_planned`. | Continue the original frozen plan. |
| 6 | Complete planned n with no earlier/safety failure. | Final fixed-sample decision plus complete anchor/resource gates. |

A low LCB alone does not prove regression; the corresponding UCB condition is required. Futility does not automatically mean harmfulness. Allocate final gain/cost, early rejection/regression, and other formal claims within the same research-family total, once; final `alpha_gain_i` remains for the fixed-sample decision. Candidates cannot change rho, n, thresholds, or allocations. Report all early stops alongside completed attempts, because reporting only completers introduces selection.

An early-stop certificate cannot authorize release. After terminal stopping, dispatch no new members; record every member's terminal state, leave unrun members unscored, retain executed/late/unknown costs for reconciliation, and preserve the endpoint across restart. Do not append a favorable suffix, swap seeds, or reopen a stopped decision. No fixed stopping step or percentage cost-saving promise follows from this algorithm.

## Full costs, context, and reporting

All stages share the root account: historical collection and trajectory filtering; candidate/strategy generation; replay CPU/storage; reflection; suggestion aggregation/merging; ranking; compilation; development execution/scoring; rejection-history retrieval; same-task retries and contrastive practice; retention probes; consolidation; meta-guidance; independent acceptance; gray rollout/operations; human review; and cache maintenance/storage. There is no free overnight quota. Count failed, rejected, unchanged, invalid, cancelled, and early-stopped work, along with input Skill context and pending/late calls.

Report task quality/success rate and failure classes, per-family capability retention, cost per attempt and per successful task, complete optimization cost, p50/p95 latency with sample sizes/uncertainty, effective context tokens or a credible upper bound, raw/deduplicated/adopted/rejected/mismatched edit counts, and call/failure counts. Actual monetary cost uses paid currency and pricing version; token/character estimates are planning only. Unreconciled usage is `usage_uncertain`, not zero or proven savings.

Report marginal cost, then cumulative cost and amortization assumptions. Net savings for N tasks is `N * (old task cost - new task cost) - additional one-time cost`; calculate break-even only when the difference is positive and supported. Fewer calls do not imply the same percentage of monetary savings. Register currency, pricing version, per-call cap, per-experiment total, and payer; the default monetary budget is zero and a model's price estimate grants no authority.

Budget exhaustion, unsupported independence, insufficient gain evidence, critical regression, environmental drift, or inadequate world support may stop work and retain the incumbent. Continued non-update is a valid result. Preserve bills, pilot observations, exposure, attempted/rejected candidates, and negative/zero results; do not keep trying until chance yields a positive example. Real task/oracle registration, paid execution, formal effect, and deployment still require their respective evidence and authorization; this document and software fixtures do not supply them.
