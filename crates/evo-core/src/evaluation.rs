//! Frozen experiment plans, partitions, query tickets, and statistics v2.
//! Simulation and estimators are implementation checks, not product-gain evidence.
use crate::{Error, Result, fingerprint, identifier, text};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PLAN_SCHEMA: &str = "rsia.experiment_plan.v1";
pub const OBJECTIVE_V1: &str = "rsia.attainment_auc.v1";
pub const STATS_HOEFFDING_V1: &str = "rsia.hoeffding.v1";
pub const STATS_BERNSTEIN_V2: &str = "rsia.empirical_bernstein.v2";
pub const SCORE_MICROS_MAX: u32 = 1_000_000;
const RANGE_R: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlCondition {
    FrozenA,
    MemoryOnlyB0,
    StaticHarnessB1,
    FixedImproverC,
    EvolvingImproverD,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataUse {
    Development,
    ReplayTrain,
    ReplaySelect,
    AcceptanceEpoch,
    OperationalMonitoring,
}

impl DataUse {
    pub fn is_protected_holdout(self) -> bool {
        matches!(self, Self::AcceptanceEpoch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    QualityGain,
    NoninferiorSavings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Improved,
    Noninferior,
    Inconclusive,
    Regressed,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Money {
    pub currency: String,
    pub pricing_version: String,
    pub amount: String,
}

impl Money {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.currency)?;
        text(&self.pricing_version, "pricing_version", 128)?;
        parse_decimal(&self.amount)?;
        Ok(())
    }

    pub fn is_zero(&self) -> bool {
        parse_decimal(&self.amount)
            .map(|(neg, units)| !neg && units == 0)
            .unwrap_or(false)
    }
}

/// Decimal quantity as an integer with 8 fractional digits. Rejects NaN/Infinity/empty.
pub fn parse_decimal(raw: &str) -> Result<(bool, u128)> {
    let s = raw.trim();
    if s.is_empty()
        || s.eq_ignore_ascii_case("nan")
        || s.eq_ignore_ascii_case("inf")
        || s.eq_ignore_ascii_case("infinity")
        || s.eq_ignore_ascii_case("+inf")
        || s.eq_ignore_ascii_case("-inf")
    {
        return Err(Error::Invalid("amount must be a finite decimal".into()));
    }
    let (neg, rest) = if let Some(r) = s.strip_prefix('-') {
        (true, r)
    } else {
        (false, s.strip_prefix('+').unwrap_or(s))
    };
    if rest.is_empty() || rest.starts_with('.') || rest.ends_with('.') {
        return Err(Error::Invalid("invalid decimal amount".into()));
    }
    let mut parts = rest.split('.');
    let whole = parts
        .next()
        .ok_or_else(|| Error::Invalid("invalid decimal amount".into()))?;
    let frac = parts.next().unwrap_or("");
    if parts.next().is_some() || whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid("invalid decimal amount".into()));
    }
    if frac.len() > 8 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid("invalid decimal amount".into()));
    }
    let whole_n: u128 = whole
        .parse()
        .map_err(|_| Error::Invalid("decimal overflow".into()))?;
    let mut frac_n: u128 = if frac.is_empty() {
        0
    } else {
        frac.parse()
            .map_err(|_| Error::Invalid("decimal overflow".into()))?
    };
    for _ in frac.len()..8 {
        frac_n = frac_n
            .checked_mul(10)
            .ok_or_else(|| Error::Invalid("decimal overflow".into()))?;
    }
    let units = whole_n
        .checked_mul(100_000_000)
        .and_then(|w| w.checked_add(frac_n))
        .ok_or_else(|| Error::Invalid("decimal overflow".into()))?;
    if neg && units == 0 {
        return Err(Error::Invalid("negative zero is not a quantity".into()));
    }
    Ok((neg, units))
}

pub fn q_from_micros(score_micros: u32) -> Result<f64> {
    if score_micros > SCORE_MICROS_MAX {
        return Err(Error::Invalid("score_micros out of range".into()));
    }
    Ok(f64::from(score_micros) / f64::from(SCORE_MICROS_MAX))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentPlan {
    pub schema_version: String,
    pub id: String,
    pub objective_version: String,
    pub stats_version: String,
    pub profile: ProfileKind,
    pub conditions: Vec<ControlCondition>,
    pub first_round: bool,
    pub min_effect: f64,
    pub noninferior_bound: f64,
    pub savings_ratio: f64,
    pub max_cost_ratio: f64,
    pub max_p95_latency_ratio: f64,
    pub alpha_total: f64,
    pub n_planned: usize,
    pub query_limit: u32,
    pub monetary_budget: Money,
    pub stop_on_harm: bool,
    pub frozen: bool,
    pub frozen_at: Option<i64>,
    pub candidate_digest: Option<String>,
    pub dataset_epoch: String,
}

impl ExperimentPlan {
    pub fn first_low_risk(id: impl Into<String>) -> Result<Self> {
        let plan = Self {
            schema_version: PLAN_SCHEMA.into(),
            id: id.into(),
            objective_version: OBJECTIVE_V1.into(),
            stats_version: STATS_BERNSTEIN_V2.into(),
            profile: ProfileKind::QualityGain,
            conditions: vec![
                ControlCondition::FrozenA,
                ControlCondition::StaticHarnessB1,
                ControlCondition::FixedImproverC,
            ],
            first_round: true,
            min_effect: 0.02,
            noninferior_bound: -0.01,
            savings_ratio: 0.10,
            max_cost_ratio: 1.10,
            max_p95_latency_ratio: 1.20,
            alpha_total: 0.05,
            n_planned: 60,
            query_limit: 3,
            monetary_budget: Money {
                currency: "USD".into(),
                pricing_version: "unset".into(),
                amount: "0".into(),
            },
            stop_on_harm: true,
            frozen: false,
            frozen_at: None,
            candidate_digest: None,
            dataset_epoch: "dev_pilot_unfunded".into(),
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != PLAN_SCHEMA {
            return Err(Error::Invalid("unsupported experiment plan schema".into()));
        }
        identifier(&self.id)?;
        identifier(&self.dataset_epoch)?;
        if self.objective_version != OBJECTIVE_V1 {
            return Err(Error::Invalid("unsupported objective_version".into()));
        }
        if self.stats_version != STATS_HOEFFDING_V1 && self.stats_version != STATS_BERNSTEIN_V2 {
            return Err(Error::Invalid(
                "unsupported stats_version; SPRT is not a drop-in replacement".into(),
            ));
        }
        if self.conditions.is_empty() || self.conditions.len() > 5 {
            return Err(Error::Invalid("invalid control condition set".into()));
        }
        let mut seen = BTreeSet::new();
        for c in &self.conditions {
            if !seen.insert(*c) {
                return Err(Error::Invalid("duplicate control condition".into()));
            }
        }
        if self.first_round
            && self.conditions.iter().any(|c| {
                matches!(
                    c,
                    ControlCondition::MemoryOnlyB0 | ControlCondition::EvolvingImproverD
                )
            })
        {
            return Err(Error::Invalid(
                "first round is A/B1/C only; B0 and D need an explicit later plan".into(),
            ));
        }
        for (v, name) in [
            (self.min_effect, "min_effect"),
            (self.noninferior_bound, "noninferior_bound"),
            (self.savings_ratio, "savings_ratio"),
            (self.max_cost_ratio, "max_cost_ratio"),
            (self.max_p95_latency_ratio, "max_p95_latency_ratio"),
            (self.alpha_total, "alpha_total"),
        ] {
            if !v.is_finite() {
                return Err(Error::Invalid(format!("{name} must be finite")));
            }
        }
        if !(0.0..=1.0).contains(&self.min_effect)
            || !(-1.0..=0.0).contains(&self.noninferior_bound)
            || !(0.0..=1.0).contains(&self.savings_ratio)
            || !(0.1..=10.0).contains(&self.max_cost_ratio)
            || !(0.1..=10.0).contains(&self.max_p95_latency_ratio)
            || !(0.0001..0.5).contains(&self.alpha_total)
            || !(2..=100000).contains(&self.n_planned)
            || !(1..=20).contains(&self.query_limit)
        {
            return Err(Error::Invalid("experiment plan bounds rejected".into()));
        }
        self.monetary_budget.validate()?;
        if self.frozen {
            if self.frozen_at.is_none() {
                return Err(Error::Invalid("frozen plan missing timestamp".into()));
            }
        } else if self.frozen_at.is_some() || self.candidate_digest.is_some() {
            return Err(Error::Invalid(
                "unfrozen plan cannot carry freeze metadata or a candidate".into(),
            ));
        }
        if let Some(d) = &self.candidate_digest {
            identifier(d)?;
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        fingerprint(self)
    }

    pub fn freeze(&mut self, now: i64) -> Result<String> {
        self.validate()?;
        if self.frozen {
            return Err(Error::Conflict("plan already frozen".into()));
        }
        if !self.monetary_budget.is_zero() {
            return Err(Error::Budget);
        }
        self.frozen = true;
        self.frozen_at = Some(now);
        self.validate()?;
        self.digest()
    }

    pub fn bind_candidate(&mut self, digest: impl Into<String>) -> Result<()> {
        if !self.frozen {
            return Err(Error::Invalid(
                "candidate cannot be bound before the plan is frozen".into(),
            ));
        }
        if self.candidate_digest.is_some() {
            return Err(Error::Conflict("candidate already bound".into()));
        }
        let digest = digest.into();
        identifier(&digest)?;
        self.candidate_digest = Some(digest);
        Ok(())
    }

    pub fn retarget_after_results(&mut self, _min_effect: f64) -> Result<()> {
        Err(Error::Conflict(
            "frozen thresholds cannot change after registration".into(),
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterObservation {
    pub cluster_id: String,
    pub d: f64,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BernsteinReport {
    pub stats_version: String,
    pub verdict: Verdict,
    pub n: usize,
    pub mean: f64,
    pub variance: f64,
    pub radius: f64,
    pub lcb: f64,
    pub alpha_i: f64,
    pub reasons: Vec<String>,
}

pub struct AlphaBudget {
    total: f64,
    remaining: f64,
    spent: BTreeMap<String, f64>,
}

impl AlphaBudget {
    pub fn new(total: f64) -> Result<Self> {
        if !total.is_finite() || !(0.0001..0.5).contains(&total) {
            return Err(Error::Invalid("invalid alpha budget".into()));
        }
        Ok(Self {
            total,
            remaining: total,
            spent: BTreeMap::new(),
        })
    }

    pub fn total(&self) -> f64 {
        self.total
    }

    pub fn remaining(&self) -> f64 {
        self.remaining
    }

    pub fn allocate(&mut self, claim: &str, alpha_i: f64) -> Result<f64> {
        identifier(claim)?;
        if self.spent.contains_key(claim) {
            return Err(Error::Conflict("double_alpha_allocation".into()));
        }
        if !alpha_i.is_finite() || alpha_i <= 0.0 || alpha_i > self.remaining + 1e-12 {
            return Err(Error::Invalid(
                "alpha allocation exceeds remaining budget".into(),
            ));
        }
        self.remaining -= alpha_i;
        if self.remaining < 0.0 {
            self.remaining = 0.0;
        }
        self.spent.insert(claim.into(), alpha_i);
        Ok(alpha_i)
    }
}

pub fn empirical_bernstein(rows: &[ClusterObservation], alpha_i: f64) -> Result<BernsteinReport> {
    if !alpha_i.is_finite() || !(0.0001..0.5).contains(&alpha_i) {
        return Ok(invalid("alpha_i out of range", alpha_i));
    }
    if rows.len() < 2 {
        return Ok(invalid("n<2", alpha_i));
    }
    let mut ids = BTreeSet::new();
    let mut weighted = 0.0;
    let mut wsum = 0.0;
    for row in rows {
        identifier(&row.cluster_id)?;
        if !ids.insert(&row.cluster_id) {
            return Err(Error::Invalid(
                "duplicate cluster, not independent evidence".into(),
            ));
        }
        if !row.d.is_finite()
            || !(-1.0..=1.0).contains(&row.d)
            || !row.weight.is_finite()
            || row.weight <= 0.0
        {
            return Ok(invalid(
                "non-finite or out-of-range cluster observation",
                alpha_i,
            ));
        }
        weighted += row.d * row.weight;
        wsum += row.weight;
    }
    if wsum <= 0.0 {
        return Ok(invalid(
            "weights must sum to a positive finite value",
            alpha_i,
        ));
    }
    let n = rows.len();
    let mean = weighted / wsum;
    let mut sse = 0.0;
    for row in rows {
        let err = row.d - mean;
        sse += row.weight * err * err;
    }
    let variance = sse / wsum * (n as f64 / (n as f64 - 1.0));
    if !variance.is_finite() || variance < 0.0 {
        return Ok(invalid("variance not defined", alpha_i));
    }
    let ln = (2.0 / alpha_i).ln();
    if !ln.is_finite() || ln <= 0.0 {
        return Ok(invalid("log term invalid", alpha_i));
    }
    let radius =
        (2.0 * variance * ln / n as f64).sqrt() + 7.0 * RANGE_R * ln / (3.0 * (n as f64 - 1.0));
    if !radius.is_finite() {
        return Ok(invalid("radius not finite", alpha_i));
    }
    let lcb = (mean - radius).max(-1.0);
    Ok(BernsteinReport {
        stats_version: STATS_BERNSTEIN_V2.into(),
        verdict: Verdict::Inconclusive,
        n,
        mean,
        variance,
        radius,
        lcb,
        alpha_i,
        reasons: Vec::new(),
    })
}

fn invalid(reason: &str, alpha_i: f64) -> BernsteinReport {
    BernsteinReport {
        stats_version: STATS_BERNSTEIN_V2.into(),
        verdict: Verdict::Invalid,
        n: 0,
        mean: 0.0,
        variance: 0.0,
        radius: 0.0,
        lcb: 0.0,
        alpha_i,
        reasons: vec![reason.into()],
    }
}

pub fn decide(
    plan: &ExperimentPlan,
    report: BernsteinReport,
    cost_ratio: f64,
    p95_ratio: f64,
    safety_ok: bool,
) -> Result<BernsteinReport> {
    plan.validate()?;
    if !plan.frozen || plan.candidate_digest.is_none() {
        return Err(Error::Invalid(
            "formal decision requires a frozen plan and bound candidate".into(),
        ));
    }
    if plan.stats_version != STATS_BERNSTEIN_V2 {
        return Err(Error::Invalid(
            "v2 decision requires empirical_bernstein.v2".into(),
        ));
    }
    let mut out = report;
    if out.verdict == Verdict::Invalid {
        return Ok(out);
    }
    if !safety_ok {
        out.verdict = Verdict::Regressed;
        out.reasons.push("safety_failed".into());
        return Ok(out);
    }
    if !cost_ratio.is_finite() || !p95_ratio.is_finite() {
        out.verdict = Verdict::Invalid;
        out.reasons.push("cost_or_latency_not_finite".into());
        return Ok(out);
    }
    if cost_ratio == 0.0 {
        out.verdict = Verdict::Invalid;
        out.reasons
            .push("baseline_cost_zero_forbids_ratio_savings".into());
        return Ok(out);
    }
    if p95_ratio > plan.max_p95_latency_ratio {
        out.reasons.push("p95_over_limit".into());
    }
    match plan.profile {
        ProfileKind::QualityGain => {
            if cost_ratio > plan.max_cost_ratio {
                out.reasons.push("cost_over_limit".into());
            }
            if out.lcb > plan.min_effect && out.reasons.is_empty() {
                out.verdict = Verdict::Improved;
            } else if out.lcb < 0.0 && out.mean < 0.0 {
                out.verdict = Verdict::Regressed;
                out.reasons.push("negative_lcb".into());
            } else {
                out.verdict = Verdict::Inconclusive;
                if out.lcb <= plan.min_effect {
                    out.reasons.push("gain_not_demonstrated".into());
                }
            }
        }
        ProfileKind::NoninferiorSavings => {
            let saved = 1.0 - cost_ratio;
            if out.lcb < plan.noninferior_bound {
                out.verdict = Verdict::Inconclusive;
                out.reasons.push("noninferiority_not_demonstrated".into());
            } else if saved + 1e-12 < plan.savings_ratio {
                out.verdict = Verdict::Inconclusive;
                out.reasons.push("savings_not_demonstrated".into());
            } else if out.reasons.is_empty() {
                out.verdict = Verdict::Noninferior;
            } else {
                out.verdict = Verdict::Inconclusive;
            }
        }
    }
    out.reasons.sort();
    out.reasons.dedup();
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskIdentity {
    pub task_id: String,
    pub raw_hash: String,
    pub normalized_digest: String,
    pub family_id: String,
    pub parent_id: Option<String>,
    pub source_repo: Option<String>,
    pub data_use: DataUse,
}

impl TaskIdentity {
    pub fn from_bytes(
        task_id: impl Into<String>,
        raw: &[u8],
        family_id: impl Into<String>,
        data_use: DataUse,
    ) -> Result<Self> {
        let task_id = task_id.into();
        identifier(&task_id)?;
        let family_id = family_id.into();
        identifier(&family_id)?;
        Ok(Self {
            task_id,
            raw_hash: crate::hash(raw),
            normalized_digest: crate::hash(normalize_for_near_dup(raw).as_bytes()),
            family_id,
            parent_id: None,
            source_repo: None,
            data_use,
        })
    }
}

pub fn normalize_for_near_dup(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut out = String::new();
    let mut prev_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            prev_space = false;
        }
    }
    out.trim().to_string()
}

#[derive(Debug, Default)]
pub struct PartitionRegistry {
    families: BTreeMap<String, DataUse>,
    raw: BTreeSet<String>,
    normalized: BTreeSet<String>,
    exposed: BTreeSet<String>,
}

impl PartitionRegistry {
    pub fn admit(&mut self, task: &TaskIdentity) -> Result<()> {
        identifier(&task.task_id)?;
        identifier(&task.family_id)?;
        if !self.raw.insert(task.raw_hash.clone()) {
            return Err(Error::Conflict("duplicate_raw_hash".into()));
        }
        if !self.normalized.insert(task.normalized_digest.clone()) {
            return Err(Error::Conflict("near_duplicate_normalized_digest".into()));
        }
        if let Some(existing) = self.families.get(&task.family_id) {
            if *existing != task.data_use {
                return Err(Error::Conflict(
                    "family_crosses_train_test_or_data_use".into(),
                ));
            }
        } else {
            self.families.insert(task.family_id.clone(), task.data_use);
        }
        if let Some(parent) = &task.parent_id {
            identifier(parent)?;
            if let Some(existing) = self.families.get(parent)
                && *existing != task.data_use
            {
                return Err(Error::Conflict("parent_family_data_use_mismatch".into()));
            }
        }
        if task.data_use.is_protected_holdout() && self.exposed.contains(&task.raw_hash) {
            return Err(Error::Conflict("holdout_previously_exposed".into()));
        }
        Ok(())
    }

    pub fn mark_exposed(&mut self, raw_hash: impl Into<String>) {
        self.exposed.insert(raw_hash.into());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketState {
    Reserved,
    Consumed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryTicket {
    pub id: String,
    pub plan_digest: String,
    pub candidate_digest: String,
    pub dataset_epoch: String,
    pub seed: String,
}

#[derive(Debug, Clone)]
struct TicketRecord {
    ticket: QueryTicket,
    state: TicketState,
}

#[derive(Debug)]
pub struct QueryBook {
    plan_digest: String,
    candidate_digest: String,
    limit: u32,
    used: u32,
    tickets: BTreeMap<String, TicketRecord>,
}

impl QueryBook {
    pub fn open(plan: &ExperimentPlan) -> Result<Self> {
        if !plan.frozen {
            return Err(Error::Invalid("query book requires a frozen plan".into()));
        }
        let candidate = plan
            .candidate_digest
            .clone()
            .ok_or_else(|| Error::Invalid("query book requires a bound candidate".into()))?;
        Ok(Self {
            plan_digest: plan.digest()?,
            candidate_digest: candidate,
            limit: plan.query_limit,
            used: 0,
            tickets: BTreeMap::new(),
        })
    }

    pub fn reserve(&mut self, ticket: QueryTicket) -> Result<()> {
        identifier(&ticket.id)?;
        identifier(&ticket.seed)?;
        identifier(&ticket.dataset_epoch)?;
        if ticket.plan_digest != self.plan_digest
            || ticket.candidate_digest != self.candidate_digest
        {
            return Err(Error::Conflict(
                "ticket does not match frozen plan/candidate".into(),
            ));
        }
        if let Some(existing) = self.tickets.get(&ticket.id) {
            if existing.ticket.seed != ticket.seed {
                return Err(Error::Conflict("ticket cannot change seed".into()));
            }
            return match existing.state {
                TicketState::Consumed => Err(Error::Conflict(
                    "ticket already consumed; replay the stored result".into(),
                )),
                TicketState::Failed | TicketState::Cancelled => Err(Error::Conflict(
                    "failed or cancelled tickets do not refund a new seed".into(),
                )),
                TicketState::Reserved => Ok(()),
            };
        }
        if self.used >= self.limit {
            return Err(Error::Budget);
        }
        self.used += 1;
        self.tickets.insert(
            ticket.id.clone(),
            TicketRecord {
                ticket,
                state: TicketState::Reserved,
            },
        );
        Ok(())
    }

    pub fn complete(&mut self, id: &str) -> Result<()> {
        self.set(id, TicketState::Consumed)
    }

    pub fn fail(&mut self, id: &str) -> Result<()> {
        self.set(id, TicketState::Failed)
    }

    pub fn cancel(&mut self, id: &str) -> Result<()> {
        self.set(id, TicketState::Cancelled)
    }

    pub fn used(&self) -> u32 {
        self.used
    }

    fn set(&mut self, id: &str, state: TicketState) -> Result<()> {
        let rec = self.tickets.get_mut(id).ok_or(Error::NotFound)?;
        if rec.state != TicketState::Reserved {
            return Err(Error::Conflict("ticket is not reserved".into()));
        }
        rec.state = state;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleEstimate {
    pub n: usize,
    pub feasible: bool,
    pub reasons: Vec<String>,
}

pub fn estimate_n(
    variance: f64,
    min_effect: f64,
    alpha_i: f64,
    max_n: usize,
    max_cost_units: u64,
    unit_cost: u64,
) -> Result<SampleEstimate> {
    if !variance.is_finite() || variance < 0.0 || !min_effect.is_finite() || min_effect <= 0.0 {
        return Err(Error::Invalid(
            "estimator inputs must be finite and positive effect".into(),
        ));
    }
    if !(2..=100000).contains(&max_n) {
        return Err(Error::Invalid("invalid max_n".into()));
    }
    for n in 2..=max_n {
        let dummy: Vec<ClusterObservation> = (0..n)
            .map(|i| ClusterObservation {
                cluster_id: format!("c{i}"),
                d: 0.0,
                weight: 1.0,
            })
            .collect();
        let mut report = empirical_bernstein(&dummy, alpha_i)?;
        report.variance = variance;
        let ln = (2.0 / alpha_i).ln();
        report.radius =
            (2.0 * variance * ln / n as f64).sqrt() + 7.0 * RANGE_R * ln / (3.0 * (n as f64 - 1.0));
        if min_effect > report.radius {
            let cost = n as u64 * unit_cost;
            if cost > max_cost_units {
                return Ok(SampleEstimate {
                    n,
                    feasible: false,
                    reasons: vec!["power_requires_n_exceeding_budget".into()],
                });
            }
            return Ok(SampleEstimate {
                n,
                feasible: true,
                reasons: Vec::new(),
            });
        }
    }
    Ok(SampleEstimate {
        n: max_n,
        feasible: false,
        reasons: vec!["n_cap_reached_without_radius_below_min_effect".into()],
    })
}

pub const FORMAL_PLAN_V41_SCHEMA: &str = "rsia.formal_experiment_plan.v4_1";
pub const COMPARISON_CONTRACT_SCHEMA: &str = "rsia.optimizer_comparison_contract.v1";
pub const OPTIMIZATION_BUDGET_SCHEMA: &str = "rsia.optimization_budget_plan.v1";

fn validate_sha256_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(format!(
            "{name} must be a lowercase sha256 digest"
        )));
    }
    Ok(())
}

fn canonical_money_units(money: &Money, name: &str) -> Result<u128> {
    money.validate()?;
    let (negative, units) = parse_decimal(&money.amount)?;
    if negative {
        return Err(Error::Invalid(format!("{name} must not be negative")));
    }
    let whole = units / 100_000_000;
    let fractional = units % 100_000_000;
    let canonical = if fractional == 0 {
        whole.to_string()
    } else {
        let fractional = format!("{fractional:08}").trim_end_matches('0').to_string();
        format!("{whole}.{fractional}")
    };
    if money.amount != canonical {
        return Err(Error::Invalid(format!(
            "{name} must use a canonical decimal amount"
        )));
    }
    Ok(units)
}

fn same_money_units(left: &Money, right: &Money) -> Result<bool> {
    Ok(left.currency == right.currency
        && left.pricing_version == right.pricing_version
        && canonical_money_units(left, "left money")?
            == canonical_money_units(right, "right money")?)
}

impl Money {
    pub fn zero_unfunded(currency: impl Into<String>) -> Self {
        Self {
            currency: currency.into(),
            pricing_version: "unset".into(),
            amount: "0".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationStage {
    HistoryCollection,
    Reflection,
    Merge,
    Ranking,
    DevelopmentExecution,
    DevelopmentScoring,
    Practice,
    Consolidation,
    Guidance,
    HumanReview,
    StorageCpu,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageMoneyBudget {
    pub stage: OptimizationStage,
    pub per_call_limit: Money,
    pub experiment_total: Money,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizationBudgetPlan {
    pub schema_version: String,
    pub root_budget_scope_id: String,
    pub payer_id: String,
    pub root_total: Money,
    pub admin_authorization_receipt_digest: Option<String>,
    pub stages: Vec<StageMoneyBudget>,
}

impl OptimizationBudgetPlan {
    pub fn unfunded(
        root_budget_scope_id: impl Into<String>,
        payer_id: impl Into<String>,
        currency: impl Into<String>,
    ) -> Result<Self> {
        let currency = currency.into();
        let stages = [
            OptimizationStage::HistoryCollection,
            OptimizationStage::Reflection,
            OptimizationStage::Merge,
            OptimizationStage::Ranking,
            OptimizationStage::DevelopmentExecution,
            OptimizationStage::DevelopmentScoring,
            OptimizationStage::Practice,
            OptimizationStage::Consolidation,
            OptimizationStage::Guidance,
            OptimizationStage::HumanReview,
            OptimizationStage::StorageCpu,
        ]
        .into_iter()
        .map(|stage| StageMoneyBudget {
            stage,
            per_call_limit: Money::zero_unfunded(currency.clone()),
            experiment_total: Money::zero_unfunded(currency.clone()),
        })
        .collect();
        let plan = Self {
            schema_version: OPTIMIZATION_BUDGET_SCHEMA.into(),
            root_budget_scope_id: root_budget_scope_id.into(),
            payer_id: payer_id.into(),
            root_total: Money::zero_unfunded(currency),
            admin_authorization_receipt_digest: None,
            stages,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != OPTIMIZATION_BUDGET_SCHEMA {
            return Err(Error::Invalid(
                "unsupported optimization budget schema".into(),
            ));
        }
        identifier(&self.root_budget_scope_id)?;
        identifier(&self.payer_id)?;
        let root_units = canonical_money_units(&self.root_total, "root_total")?;
        if let Some(receipt) = &self.admin_authorization_receipt_digest {
            validate_sha256_digest(receipt, "admin_authorization_receipt_digest")?;
        }
        let required: BTreeSet<_> = [
            OptimizationStage::HistoryCollection,
            OptimizationStage::Reflection,
            OptimizationStage::Merge,
            OptimizationStage::Ranking,
            OptimizationStage::DevelopmentExecution,
            OptimizationStage::DevelopmentScoring,
            OptimizationStage::Practice,
            OptimizationStage::Consolidation,
            OptimizationStage::Guidance,
            OptimizationStage::HumanReview,
            OptimizationStage::StorageCpu,
        ]
        .into_iter()
        .collect();
        let mut actual = BTreeSet::new();
        let mut stage_sum = 0u128;
        for stage in &self.stages {
            if !actual.insert(stage.stage) {
                return Err(Error::Invalid("duplicate optimization budget stage".into()));
            }
            let per_call = canonical_money_units(&stage.per_call_limit, "per_call_limit")?;
            let stage_total = canonical_money_units(&stage.experiment_total, "experiment_total")?;
            if stage.per_call_limit.currency != self.root_total.currency
                || stage.experiment_total.currency != self.root_total.currency
                || stage.per_call_limit.pricing_version != self.root_total.pricing_version
                || stage.experiment_total.pricing_version != self.root_total.pricing_version
            {
                return Err(Error::Invalid(
                    "all stage budgets must use the root currency and pricing version".into(),
                ));
            }
            if per_call > stage_total || stage_total > root_units {
                return Err(Error::Invalid(
                    "budget ordering must satisfy per_call <= stage_total <= root_total".into(),
                ));
            }
            stage_sum = stage_sum
                .checked_add(stage_total)
                .ok_or_else(|| Error::Invalid("stage budget sum overflow".into()))?;
        }
        if actual != required {
            return Err(Error::Invalid(
                "optimization budget omits a charged stage".into(),
            ));
        }
        if stage_sum > root_units {
            return Err(Error::Invalid(
                "sum of stage totals exceeds the root budget".into(),
            ));
        }
        if root_units > 0 && self.admin_authorization_receipt_digest.is_none() {
            return Err(Error::Budget);
        }
        if root_units > 0 && self.root_total.pricing_version == "unset" {
            return Err(Error::Invalid(
                "positive root budget requires an explicit pricing version".into(),
            ));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixedOptimizerArm {
    CSimple,
    CSkillopt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedOptimizerSpec {
    pub arm: FixedOptimizerArm,
    pub implementation_digest: String,
    pub initial_s0_digest: String,
    pub base_model_digest: String,
    pub tools_digest: String,
    pub authorized_materials_digest: String,
    pub task_partition_digest: String,
    pub context_limit_tokens: u64,
    pub root_budget_scope_id: String,
    pub budget_limit: Money,
}

impl FixedOptimizerSpec {
    fn validate(&self) -> Result<()> {
        for (value, name) in [
            (&self.implementation_digest, "implementation_digest"),
            (&self.initial_s0_digest, "initial_s0_digest"),
            (&self.base_model_digest, "base_model_digest"),
            (&self.tools_digest, "tools_digest"),
            (
                &self.authorized_materials_digest,
                "authorized_materials_digest",
            ),
            (&self.task_partition_digest, "task_partition_digest"),
        ] {
            validate_sha256_digest(value, name)?;
        }
        identifier(&self.root_budget_scope_id)?;
        canonical_money_units(&self.budget_limit, "optimizer budget_limit")?;
        if self.context_limit_tokens == 0 {
            return Err(Error::Invalid(
                "optimizer context limit must be explicit".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizerComparisonContract {
    pub schema_version: String,
    pub c_simple: FixedOptimizerSpec,
    pub c_skillopt: FixedOptimizerSpec,
    pub d_starts_from: FixedOptimizerArm,
}

impl OptimizerComparisonContract {
    pub fn new(
        c_simple: FixedOptimizerSpec,
        c_skillopt: FixedOptimizerSpec,
        d_starts_from: FixedOptimizerArm,
    ) -> Result<Self> {
        let contract = Self {
            schema_version: COMPARISON_CONTRACT_SCHEMA.into(),
            c_simple,
            c_skillopt,
            d_starts_from,
        };
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != COMPARISON_CONTRACT_SCHEMA {
            return Err(Error::Invalid(
                "unsupported optimizer comparison schema".into(),
            ));
        }
        self.c_simple.validate()?;
        self.c_skillopt.validate()?;
        if self.c_simple.arm != FixedOptimizerArm::CSimple
            || self.c_skillopt.arm != FixedOptimizerArm::CSkillopt
        {
            return Err(Error::Invalid("fixed optimizer arms are mislabeled".into()));
        }
        for (left, right, name) in [
            (
                &self.c_simple.initial_s0_digest,
                &self.c_skillopt.initial_s0_digest,
                "initial S0",
            ),
            (
                &self.c_simple.base_model_digest,
                &self.c_skillopt.base_model_digest,
                "base model",
            ),
            (
                &self.c_simple.tools_digest,
                &self.c_skillopt.tools_digest,
                "tools",
            ),
            (
                &self.c_simple.authorized_materials_digest,
                &self.c_skillopt.authorized_materials_digest,
                "authorized materials",
            ),
            (
                &self.c_simple.task_partition_digest,
                &self.c_skillopt.task_partition_digest,
                "task partition",
            ),
            (
                &self.c_simple.root_budget_scope_id,
                &self.c_skillopt.root_budget_scope_id,
                "root budget scope",
            ),
        ] {
            if left != right {
                return Err(Error::Invalid(format!(
                    "C_simple and C_skillopt must share {name}"
                )));
            }
        }
        if self.c_simple.context_limit_tokens != self.c_skillopt.context_limit_tokens {
            return Err(Error::Invalid(
                "C_simple and C_skillopt must share the context limit".into(),
            ));
        }
        if !same_money_units(&self.c_simple.budget_limit, &self.c_skillopt.budget_limit)? {
            return Err(Error::Invalid(
                "C_simple and C_skillopt must have the same monetary budget limit".into(),
            ));
        }
        Ok(())
    }

    pub fn d_frozen_start_digest(&self) -> &str {
        match self.d_starts_from {
            FixedOptimizerArm::CSimple => &self.c_simple.implementation_digest,
            FixedOptimizerArm::CSkillopt => &self.c_skillopt.implementation_digest,
        }
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormalStatisticalUnit {
    IndependentTaskCluster,
    IndependentCompleteStratifiedBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormalWeighting {
    EqualUnits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixSelectionRule {
    FrozenContinuousOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormalExecutionAuthority {
    NotIssued,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalExperimentPlanV41 {
    pub schema_version: String,
    pub v1_plan_snapshot_digest: String,
    pub preregistration_receipt_digest: String,
    pub research_family_id: String,
    pub alpha_plan_digest: String,
    pub optimizer_comparison_digest: String,
    pub optimization_budget_digest: String,
    pub anchor_coverage_digest: String,
    pub source_sampling_digest: String,
    pub statistical_assumptions_digest: String,
    pub profile: ProfileKind,
    pub statistical_unit: FormalStatisticalUnit,
    pub weighting: FormalWeighting,
    pub prefix_selection: PrefixSelectionRule,
    pub execution_authority: FormalExecutionAuthority,
    pub n_planned: u32,
}

impl FormalExperimentPlanV41 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        v1_plan_snapshot: &ExperimentPlan,
        preregistration_receipt_digest: impl Into<String>,
        alpha_plan: &crate::sequential::ResearchFamilyAlphaPlan,
        comparison: &OptimizerComparisonContract,
        budget: &OptimizationBudgetPlan,
        anchor_coverage_digest: impl Into<String>,
        source_sampling_digest: impl Into<String>,
        statistical_assumptions_digest: impl Into<String>,
        statistical_unit: FormalStatisticalUnit,
        n_planned: u32,
    ) -> Result<Self> {
        let plan = Self {
            schema_version: FORMAL_PLAN_V41_SCHEMA.into(),
            v1_plan_snapshot_digest: v1_plan_snapshot.digest()?,
            preregistration_receipt_digest: preregistration_receipt_digest.into(),
            research_family_id: alpha_plan.research_family_id.clone(),
            alpha_plan_digest: alpha_plan.digest()?,
            optimizer_comparison_digest: comparison.digest()?,
            optimization_budget_digest: budget.digest()?,
            anchor_coverage_digest: anchor_coverage_digest.into(),
            source_sampling_digest: source_sampling_digest.into(),
            statistical_assumptions_digest: statistical_assumptions_digest.into(),
            profile: v1_plan_snapshot.profile,
            statistical_unit,
            weighting: FormalWeighting::EqualUnits,
            prefix_selection: PrefixSelectionRule::FrozenContinuousOrder,
            execution_authority: FormalExecutionAuthority::NotIssued,
            n_planned,
        };
        plan.validate_against(v1_plan_snapshot, alpha_plan, comparison, budget)?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != FORMAL_PLAN_V41_SCHEMA {
            return Err(Error::Invalid("unsupported v4.1 formal-plan schema".into()));
        }
        identifier(&self.research_family_id)?;
        for (value, name) in [
            (&self.v1_plan_snapshot_digest, "v1_plan_snapshot_digest"),
            (
                &self.preregistration_receipt_digest,
                "preregistration_receipt_digest",
            ),
            (&self.alpha_plan_digest, "alpha_plan_digest"),
            (
                &self.optimizer_comparison_digest,
                "optimizer_comparison_digest",
            ),
            (
                &self.optimization_budget_digest,
                "optimization_budget_digest",
            ),
            (&self.anchor_coverage_digest, "anchor_coverage_digest"),
            (&self.source_sampling_digest, "source_sampling_digest"),
            (
                &self.statistical_assumptions_digest,
                "statistical_assumptions_digest",
            ),
        ] {
            validate_sha256_digest(value, name)?;
        }
        if self.n_planned < 2 {
            return Err(Error::Invalid(
                "formal n_planned must be supplied explicitly and be at least two".into(),
            ));
        }
        Ok(())
    }

    pub fn validate_against(
        &self,
        v1_plan_snapshot: &ExperimentPlan,
        alpha_plan: &crate::sequential::ResearchFamilyAlphaPlan,
        comparison: &OptimizerComparisonContract,
        budget: &OptimizationBudgetPlan,
    ) -> Result<()> {
        self.validate()?;
        v1_plan_snapshot.validate()?;
        alpha_plan.validate()?;
        comparison.validate()?;
        budget.validate()?;
        if !v1_plan_snapshot.frozen
            || v1_plan_snapshot.frozen_at.is_none()
            || v1_plan_snapshot.candidate_digest.is_some()
        {
            return Err(Error::Invalid(
                "v4.1 formal wrapper requires a frozen pre-candidate v1 plan snapshot".into(),
            ));
        }
        if self.v1_plan_snapshot_digest != v1_plan_snapshot.digest()?
            || self.alpha_plan_digest != alpha_plan.digest()?
            || self.optimizer_comparison_digest != comparison.digest()?
            || self.optimization_budget_digest != budget.digest()?
            || self.research_family_id != alpha_plan.research_family_id
        {
            return Err(Error::Invalid(
                "formal wrapper digest bindings do not match the supplied contracts".into(),
            ));
        }
        let snapshot_n = u32::try_from(v1_plan_snapshot.n_planned)
            .map_err(|_| Error::Invalid("v1 n_planned is outside the v4.1 domain".into()))?;
        if self.n_planned != snapshot_n || self.profile != v1_plan_snapshot.profile {
            return Err(Error::Invalid(
                "formal n_planned/profile differ from the frozen v1 snapshot".into(),
            ));
        }
        let (_, alpha_units) = parse_decimal(&alpha_plan.alpha_total)?;
        let alpha_total = alpha_units as f64 / 100_000_000.0;
        if (alpha_total - v1_plan_snapshot.alpha_total).abs() > 1e-15 {
            return Err(Error::Invalid(
                "research-family alpha total differs from the frozen v1 snapshot".into(),
            ));
        }
        if !v1_plan_snapshot
            .conditions
            .contains(&ControlCondition::FixedImproverC)
        {
            return Err(Error::Invalid(
                "optimizer comparison requires FixedImproverC in the v1 plan".into(),
            ));
        }
        if comparison.c_simple.root_budget_scope_id != budget.root_budget_scope_id
            || comparison.c_skillopt.root_budget_scope_id != budget.root_budget_scope_id
            || !same_money_units(&comparison.c_simple.budget_limit, &budget.root_total)?
            || !same_money_units(&comparison.c_skillopt.budget_limit, &budget.root_total)?
            || !same_money_units(&v1_plan_snapshot.monetary_budget, &budget.root_total)?
        {
            return Err(Error::Invalid(
                "formal comparison, v1 plan, and optimization budget must share one root budget"
                    .into(),
            ));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frozen_plan() -> ExperimentPlan {
        let mut p = ExperimentPlan::first_low_risk("exp1").unwrap();
        p.freeze(1).unwrap();
        p.bind_candidate("cand1").unwrap();
        p
    }

    fn zeros(n: usize) -> Vec<ClusterObservation> {
        (0..n)
            .map(|i| ClusterObservation {
                cluster_id: format!("c{i}"),
                d: 0.0,
                weight: 1.0,
            })
            .collect()
    }

    #[test]
    fn unknown_stats_version_rejected() {
        let mut p = ExperimentPlan::first_low_risk("exp1").unwrap();
        p.stats_version = "sprt".into();
        assert!(p.validate().is_err());
    }

    #[test]
    fn first_round_rejects_d() {
        let mut p = ExperimentPlan::first_low_risk("exp1").unwrap();
        p.conditions.push(ControlCondition::EvolvingImproverD);
        assert!(p.validate().is_err());
    }

    #[test]
    fn freeze_then_bind() {
        let mut p = ExperimentPlan::first_low_risk("exp1").unwrap();
        assert!(p.bind_candidate("cand1").is_err());
        p.freeze(9).unwrap();
        p.bind_candidate("cand1").unwrap();
        assert!(p.bind_candidate("cand2").is_err());
        assert!(p.retarget_after_results(0.001).is_err());
    }

    #[test]
    fn nonzero_budget_needs_admin_authorization() {
        let mut p = ExperimentPlan::first_low_risk("exp1").unwrap();
        p.monetary_budget.amount = "1.00".into();
        assert!(matches!(p.freeze(1), Err(Error::Budget)));
    }

    #[test]
    fn nan_amount_rejected() {
        assert!(parse_decimal("NaN").is_err());
        assert!(parse_decimal("Infinity").is_err());
    }

    #[test]
    fn n_less_than_two_invalid() {
        let r = empirical_bernstein(&zeros(1), 0.05).unwrap();
        assert_eq!(r.verdict, Verdict::Invalid);
    }

    #[test]
    fn duplicate_cluster_rejected() {
        let rows = vec![
            ClusterObservation {
                cluster_id: "c0".into(),
                d: 0.1,
                weight: 1.0,
            },
            ClusterObservation {
                cluster_id: "c0".into(),
                d: 0.2,
                weight: 1.0,
            },
        ];
        assert!(empirical_bernstein(&rows, 0.05).is_err());
    }

    #[test]
    fn nan_delta_invalid() {
        let rows = vec![
            ClusterObservation {
                cluster_id: "c0".into(),
                d: f64::NAN,
                weight: 1.0,
            },
            ClusterObservation {
                cluster_id: "c1".into(),
                d: 0.0,
                weight: 1.0,
            },
        ];
        let r = empirical_bernstein(&rows, 0.05).unwrap();
        assert_eq!(r.verdict, Verdict::Invalid);
    }

    #[test]
    fn zero_effect_is_not_improved() {
        let plan = frozen_plan();
        let report = empirical_bernstein(&zeros(60), 0.05).unwrap();
        let decided = decide(&plan, report, 1.0, 1.0, true).unwrap();
        assert_ne!(decided.verdict, Verdict::Improved);
        assert!(decided.lcb < plan.min_effect);
    }

    #[test]
    fn known_degradation_regresses() {
        let plan = frozen_plan();
        let rows: Vec<_> = (0..60)
            .map(|i| ClusterObservation {
                cluster_id: format!("c{i}"),
                d: -1.0,
                weight: 1.0,
            })
            .collect();
        let report = empirical_bernstein(&rows, 0.05).unwrap();
        let decided = decide(&plan, report, 1.0, 1.0, true).unwrap();
        assert_eq!(decided.verdict, Verdict::Regressed);
    }

    #[test]
    fn strong_gain_can_improve() {
        let plan = frozen_plan();
        let rows: Vec<_> = (0..60)
            .map(|i| ClusterObservation {
                cluster_id: format!("c{i}"),
                d: 1.0,
                weight: 1.0,
            })
            .collect();
        let report = empirical_bernstein(&rows, 0.05).unwrap();
        let decided = decide(&plan, report, 1.0, 1.0, true).unwrap();
        assert_eq!(decided.verdict, Verdict::Improved);
    }

    #[test]
    fn double_alpha_allocation_rejected() {
        let mut b = AlphaBudget::new(0.05).unwrap();
        b.allocate("claim_a", 0.05).unwrap();
        assert!(b.allocate("claim_a", 0.05).is_err());
        assert!(b.allocate("claim_b", 0.01).is_err());
    }

    #[test]
    fn tickets_deduct_before_result_and_do_not_refund() {
        let plan = frozen_plan();
        let mut book = QueryBook::open(&plan).unwrap();
        let t = QueryTicket {
            id: "t1".into(),
            plan_digest: plan.digest().unwrap(),
            candidate_digest: "cand1".into(),
            dataset_epoch: plan.dataset_epoch.clone(),
            seed: "s1".into(),
        };
        book.reserve(t.clone()).unwrap();
        assert_eq!(book.used(), 1);
        book.fail("t1").unwrap();
        let mut t2 = t.clone();
        t2.id = "t2".into();
        t2.seed = "s2".into();
        book.reserve(t2).unwrap();
        let mut t3 = t;
        t3.seed = "s3".into();
        assert!(book.reserve(t3).is_err());
    }

    #[test]
    fn missing_rows_cannot_be_replaced_by_new_seed_after_cancel() {
        let plan = frozen_plan();
        let mut book = QueryBook::open(&plan).unwrap();
        let t = QueryTicket {
            id: "t1".into(),
            plan_digest: plan.digest().unwrap(),
            candidate_digest: "cand1".into(),
            dataset_epoch: plan.dataset_epoch.clone(),
            seed: "s1".into(),
        };
        book.reserve(t.clone()).unwrap();
        book.cancel("t1").unwrap();
        assert!(book.reserve(t).is_err());
    }

    #[test]
    fn near_duplicate_and_family_split_are_contamination() {
        let mut reg = PartitionRegistry::default();
        let a =
            TaskIdentity::from_bytes("t1", b"Hello  World", "fam", DataUse::Development).unwrap();
        let b =
            TaskIdentity::from_bytes("t2", b"hello world", "fam", DataUse::Development).unwrap();
        assert_ne!(a.raw_hash, b.raw_hash);
        assert_eq!(a.normalized_digest, b.normalized_digest);
        reg.admit(&a).unwrap();
        assert!(reg.admit(&b).is_err());
        let c = TaskIdentity::from_bytes("t3", b"other", "fam", DataUse::AcceptanceEpoch).unwrap();
        assert!(reg.admit(&c).is_err());
    }

    #[test]
    fn score_micros_bounds() {
        assert_eq!(q_from_micros(0).unwrap(), 0.0);
        assert_eq!(q_from_micros(1_000_000).unwrap(), 1.0);
        assert!(q_from_micros(1_000_001).is_err());
    }

    #[test]
    fn infeasible_estimate_does_not_lower_threshold() {
        let est = estimate_n(1.0, 0.02, 0.05, 5, 10, 10).unwrap();
        assert!(!est.feasible);
        assert!(
            est.reasons
                .iter()
                .any(|r| r.contains("budget") || r.contains("n_cap"))
        );
    }
}
