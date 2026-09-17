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
