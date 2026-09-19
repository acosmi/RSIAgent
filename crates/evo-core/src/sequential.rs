//! v4.1 reject-only sequential evaluation contracts (§3.4.1, V085).
//! Approval remains the fixed-sample empirical-Bernstein v2 decision.
use crate::evaluation::{ProfileKind, SCORE_MICROS_MAX, parse_decimal};
use crate::{Error, Result, fingerprint, identifier};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const EARLY_STOP_PLAN_SCHEMA: &str = "rsia.early_stop_plan.v1";
pub const EARLY_STOP_VERSION: &str = "rsia.sequential_reject_only.v1";
pub const CS_VERSION: &str = "rsia.normal_mixture_bounded.v1";
pub const ALPHA_PLAN_SCHEMA: &str = "rsia.research_family_alpha_plan.v1";
const CS_RHO_DECIMAL: &str = "1";
const NUMERIC_OUTWARD_ERROR: f64 = 1e-12;

fn digest(value: &str, name: &str) -> Result<()> {
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

fn canonical_nonnegative_decimal(raw: &str, name: &str) -> Result<u128> {
    if raw.starts_with('+') || raw.starts_with('-') || raw.contains('e') || raw.contains('E') {
        return Err(Error::Invalid(format!("{name} is not a canonical decimal")));
    }
    let (_, units) = parse_decimal(raw)?;
    let canonical = if units == 0 {
        "0".to_string()
    } else {
        let whole = units / 100_000_000;
        let fraction = units % 100_000_000;
        if fraction == 0 {
            whole.to_string()
        } else {
            let fraction = format!("{fraction:08}").trim_end_matches('0').to_string();
            format!("{whole}.{fraction}")
        }
    };
    if raw != canonical {
        return Err(Error::Invalid(format!("{name} is not a canonical decimal")));
    }
    Ok(units)
}

fn decimal_f64(raw: &str, name: &str) -> Result<f64> {
    canonical_nonnegative_decimal(raw, name)?;
    raw.parse::<f64>()
        .map_err(|_| Error::Invalid(format!("{name} is not finite")))
}

fn canonical_signed_decimal(raw: &str, name: &str) -> Result<f64> {
    if raw.starts_with('+') || raw.contains('e') || raw.contains('E') {
        return Err(Error::Invalid(format!("{name} is not a canonical decimal")));
    }
    let magnitude = raw.strip_prefix('-').unwrap_or(raw);
    let mut parts = magnitude.split('.');
    let whole = parts.next().unwrap_or("");
    let fraction = parts.next();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
        || fraction.is_some_and(|value| {
            value.is_empty()
                || value.len() > 17
                || value.ends_with('0')
                || !value.bytes().all(|byte| byte.is_ascii_digit())
        })
        || (raw.starts_with('-') && magnitude == "0")
    {
        return Err(Error::Invalid(format!("{name} is not a canonical decimal")));
    }
    let value = raw
        .parse::<f64>()
        .map_err(|_| Error::Invalid(format!("{name} is not finite")))?;
    if !value.is_finite() {
        return Err(Error::Invalid(format!("{name} is not finite")));
    }
    Ok(value)
}

fn finite_decimal(value: f64) -> Result<String> {
    if !value.is_finite() {
        return Err(Error::Invalid("non-finite decimal result".into()));
    }
    let mut rendered = format!("{value:.17}");
    while rendered.contains('.') && rendered.ends_with('0') {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.pop();
    }
    if rendered == "-0" {
        rendered = "0".into();
    }
    Ok(rendered)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormalClaimKind {
    FixedSampleGain,
    FixedSampleCost,
    SequentialReject,
    OtherFormalMetric,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlphaAllocation {
    pub claim_id: String,
    pub attempt_id: String,
    pub kind: FormalClaimKind,
    /// Canonical finite decimal string; never a JSON float.
    pub alpha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchFamilyAlphaPlan {
    pub schema_version: String,
    pub research_family_id: String,
    /// Canonical finite decimal string; never a JSON float.
    pub alpha_total: String,
    pub allocations: Vec<AlphaAllocation>,
}

impl ResearchFamilyAlphaPlan {
    pub fn new(
        research_family_id: impl Into<String>,
        alpha_total: impl Into<String>,
        allocations: Vec<AlphaAllocation>,
    ) -> Result<Self> {
        let plan = Self {
            schema_version: ALPHA_PLAN_SCHEMA.into(),
            research_family_id: research_family_id.into(),
            alpha_total: alpha_total.into(),
            allocations,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != ALPHA_PLAN_SCHEMA {
            return Err(Error::Invalid(
                "unsupported research-family alpha schema".into(),
            ));
        }
        identifier(&self.research_family_id)?;
        let total = canonical_nonnegative_decimal(&self.alpha_total, "alpha_total")?;
        if total == 0 || total >= 50_000_000 {
            return Err(Error::Invalid("alpha_total must be in (0,0.5)".into()));
        }
        if self.allocations.is_empty() {
            return Err(Error::Invalid("alpha plan requires formal claims".into()));
        }
        let mut claim_ids = BTreeSet::new();
        let mut sum = 0u128;
        for allocation in &self.allocations {
            identifier(&allocation.claim_id)?;
            identifier(&allocation.attempt_id)?;
            if !claim_ids.insert(allocation.claim_id.clone()) {
                return Err(Error::Invalid("duplicate alpha allocation identity".into()));
            }
            let units = canonical_nonnegative_decimal(&allocation.alpha, "claim alpha")?;
            if units == 0 {
                return Err(Error::Invalid("claim alpha must be positive".into()));
            }
            sum = sum
                .checked_add(units)
                .ok_or_else(|| Error::Invalid("alpha allocation overflow".into()))?;
        }
        if sum > total {
            return Err(Error::Invalid(
                "formal alpha allocations exceed family total".into(),
            ));
        }
        Ok(())
    }

    pub fn allocation(
        &self,
        claim_id: &str,
        attempt_id: &str,
        kind: FormalClaimKind,
    ) -> Result<&str> {
        self.validate()?;
        self.allocations
            .iter()
            .find(|allocation| {
                allocation.claim_id == claim_id
                    && allocation.attempt_id == attempt_id
                    && allocation.kind == kind
            })
            .map(|allocation| allocation.alpha.as_str())
            .ok_or_else(|| Error::Invalid("alpha claim is absent or has the wrong scope".into()))
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EarlyStopPlan {
    pub schema_version: String,
    pub early_stop_version: String,
    pub cs_version: String,
    pub v1_plan_snapshot_digest: String,
    pub alpha_plan_digest: String,
    pub alpha_stop_claim_id: String,
    pub attempt_id: String,
    pub rho: String,
    pub alpha_stop_i: String,
    pub min_gain_micros: u32,
    pub max_regression_micros: u32,
    pub noninferiority_margin_micros: u32,
    pub n_planned: u32,
    pub profile: ProfileKind,
    pub ordered_cluster_ids: Vec<String>,
    pub statistical_assumptions_digest: String,
    pub source_sampling_digest: String,
    pub anchor_coverage_digest: String,
}

impl EarlyStopPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        v1_plan_snapshot_digest: impl Into<String>,
        alpha_plan: &ResearchFamilyAlphaPlan,
        alpha_stop_claim_id: impl Into<String>,
        attempt_id: impl Into<String>,
        profile: ProfileKind,
        n_planned: u32,
        ordered_cluster_ids: Vec<String>,
        statistical_assumptions_digest: impl Into<String>,
        source_sampling_digest: impl Into<String>,
        anchor_coverage_digest: impl Into<String>,
        min_gain_micros: u32,
        max_regression_micros: u32,
        noninferiority_margin_micros: u32,
    ) -> Result<Self> {
        let alpha_stop_claim_id = alpha_stop_claim_id.into();
        let attempt_id = attempt_id.into();
        let alpha_stop_i = alpha_plan
            .allocation(
                &alpha_stop_claim_id,
                &attempt_id,
                FormalClaimKind::SequentialReject,
            )?
            .to_string();
        let plan = Self {
            schema_version: EARLY_STOP_PLAN_SCHEMA.into(),
            early_stop_version: EARLY_STOP_VERSION.into(),
            cs_version: CS_VERSION.into(),
            v1_plan_snapshot_digest: v1_plan_snapshot_digest.into(),
            alpha_plan_digest: alpha_plan.digest()?,
            alpha_stop_claim_id,
            attempt_id,
            rho: CS_RHO_DECIMAL.into(),
            alpha_stop_i,
            min_gain_micros,
            max_regression_micros,
            noninferiority_margin_micros,
            n_planned,
            profile,
            ordered_cluster_ids,
            statistical_assumptions_digest: statistical_assumptions_digest.into(),
            source_sampling_digest: source_sampling_digest.into(),
            anchor_coverage_digest: anchor_coverage_digest.into(),
        };
        plan.validate_against(alpha_plan)?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != EARLY_STOP_PLAN_SCHEMA
            || self.early_stop_version != EARLY_STOP_VERSION
            || self.cs_version != CS_VERSION
        {
            return Err(Error::Invalid("unsupported early-stop contract".into()));
        }
        digest(&self.v1_plan_snapshot_digest, "v1_plan_snapshot_digest")?;
        digest(&self.alpha_plan_digest, "alpha_plan_digest")?;
        digest(
            &self.statistical_assumptions_digest,
            "statistical_assumptions_digest",
        )?;
        digest(&self.source_sampling_digest, "source_sampling_digest")?;
        digest(&self.anchor_coverage_digest, "anchor_coverage_digest")?;
        identifier(&self.alpha_stop_claim_id)?;
        identifier(&self.attempt_id)?;
        if self.rho != CS_RHO_DECIMAL {
            return Err(Error::Invalid("v1 confidence sequence fixes rho=1".into()));
        }
        let alpha = canonical_nonnegative_decimal(&self.alpha_stop_i, "alpha_stop_i")?;
        if alpha == 0 || alpha >= 100_000_000 {
            return Err(Error::Invalid("alpha_stop_i must be in (0,1)".into()));
        }
        for (value, name) in [
            (self.min_gain_micros, "min_gain_micros"),
            (self.max_regression_micros, "max_regression_micros"),
            (
                self.noninferiority_margin_micros,
                "noninferiority_margin_micros",
            ),
        ] {
            if value == 0 || value > SCORE_MICROS_MAX {
                return Err(Error::Invalid(format!("{name} must be in (0,1000000]")));
            }
        }
        if self.n_planned < 2
            || self.n_planned > 100_000
            || self.ordered_cluster_ids.len() != self.n_planned as usize
        {
            return Err(Error::Invalid(
                "n_planned must be explicit, supported, and match the frozen order".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        for cluster_id in &self.ordered_cluster_ids {
            identifier(cluster_id)?;
            if !seen.insert(cluster_id) {
                return Err(Error::Invalid(
                    "duplicate cluster in frozen prefix order".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn validate_against(&self, alpha_plan: &ResearchFamilyAlphaPlan) -> Result<()> {
        self.validate()?;
        if self.alpha_plan_digest != alpha_plan.digest()? {
            return Err(Error::Invalid(
                "early-stop plan binds a different alpha plan".into(),
            ));
        }
        let allocated = alpha_plan.allocation(
            &self.alpha_stop_claim_id,
            &self.attempt_id,
            FormalClaimKind::SequentialReject,
        )?;
        if self.alpha_stop_i != allocated {
            return Err(Error::Invalid(
                "alpha_stop_i differs from family allocation".into(),
            ));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletePairedUnit {
    pub cluster_id: String,
    pub baseline_score_micros: u32,
    pub candidate_score_micros: u32,
}

impl CompletePairedUnit {
    fn difference_micros(&self) -> Result<i64> {
        identifier(&self.cluster_id)?;
        if self.baseline_score_micros > SCORE_MICROS_MAX
            || self.candidate_score_micros > SCORE_MICROS_MAX
        {
            return Err(Error::Invalid("score micros out of range".into()));
        }
        Ok(i64::from(self.candidate_score_micros) - i64::from(self.baseline_score_micros))
    }
}

#[derive(Debug, Clone)]
pub struct CsPrefix {
    pub k: u32,
    pub mean: f64,
    pub radius: f64,
    pub lcb: f64,
    pub ucb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CsPrefixWire {
    pub k: u32,
    pub mean: String,
    pub radius: String,
    pub lcb: String,
    pub ucb: String,
}

impl CsPrefix {
    pub fn to_wire(&self) -> Result<CsPrefixWire> {
        Ok(CsPrefixWire {
            k: self.k,
            mean: finite_decimal(self.mean)?,
            radius: finite_decimal(self.radius)?,
            lcb: finite_decimal(self.lcb)?,
            ucb: finite_decimal(self.ucb)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EarlyStopDecision {
    Collecting,
    Continue,
    FinalFixedSample,
    CriticalRegression,
    FutilityQualityGain,
    FutilityNoninferiority,
    RegressedSequential,
}

impl EarlyStopDecision {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Collecting | Self::Continue)
    }

    pub fn is_early_stop(self) -> bool {
        matches!(
            self,
            Self::CriticalRegression
                | Self::FutilityQualityGain
                | Self::FutilityNoninferiority
                | Self::RegressedSequential
        )
    }
}

#[derive(Debug)]
pub struct SequentialRejectOnly {
    plan: EarlyStopPlan,
    pending: BTreeMap<usize, CompletePairedUnit>,
    completed_cluster_ids: BTreeSet<String>,
    prefix: Vec<CompletePairedUnit>,
    terminal: Option<EarlyStopDecision>,
}

impl SequentialRejectOnly {
    pub fn open(plan: EarlyStopPlan, alpha_plan: &ResearchFamilyAlphaPlan) -> Result<Self> {
        plan.validate_against(alpha_plan)?;
        Ok(Self {
            plan,
            pending: BTreeMap::new(),
            completed_cluster_ids: BTreeSet::new(),
            prefix: Vec::new(),
            terminal: None,
        })
    }

    /// Out-of-order pairs wait for their gap. Each newly continuous unit is
    /// tested separately so the first stopping prefix is independent of return order.
    pub fn record_complete_unit(&mut self, unit: CompletePairedUnit) -> Result<EarlyStopDecision> {
        if self.terminal.is_some() {
            return Err(Error::Conflict(
                "sequential evaluation is already terminal".into(),
            ));
        }
        unit.difference_micros()?;
        let position = self
            .plan
            .ordered_cluster_ids
            .iter()
            .position(|id| id == &unit.cluster_id)
            .ok_or_else(|| Error::Invalid("unit is absent from the frozen order".into()))?;
        if !self.completed_cluster_ids.insert(unit.cluster_id.clone()) {
            return Err(Error::Conflict("duplicate completed cluster".into()));
        }
        self.pending.insert(position, unit);
        while let Some(next) = self.pending.remove(&self.prefix.len()) {
            self.prefix.push(next);
            let decision = self.compute_decision()?;
            if decision.is_terminal() {
                self.terminal = Some(decision);
                return Ok(decision);
            }
        }
        self.decision()
    }

    pub fn stop_for_critical_anchor(
        &mut self,
        anchor_gate_evidence_digest: &str,
    ) -> Result<EarlyStopDecision> {
        digest(anchor_gate_evidence_digest, "anchor_gate_evidence_digest")?;
        if self.terminal.is_some() {
            return Err(Error::Conflict(
                "sequential evaluation is already terminal".into(),
            ));
        }
        self.terminal = Some(EarlyStopDecision::CriticalRegression);
        Ok(EarlyStopDecision::CriticalRegression)
    }

    pub fn k(&self) -> u32 {
        self.prefix.len() as u32
    }

    pub fn decision(&self) -> Result<EarlyStopDecision> {
        self.terminal
            .map(Ok)
            .unwrap_or_else(|| self.compute_decision())
    }

    pub fn boundary(&self) -> Result<Option<CsPrefix>> {
        if self.prefix.is_empty() {
            return Ok(None);
        }
        let k = self.prefix.len() as u32;
        let kf = f64::from(k);
        let sum_micros = self.prefix.iter().try_fold(0i64, |sum, unit| {
            sum.checked_add(unit.difference_micros()?)
                .ok_or_else(|| Error::Invalid("prefix micros overflow".into()))
        })?;
        let mean = sum_micros as f64 / (kf * f64::from(SCORE_MICROS_MAX));
        let alpha = decimal_f64(&self.plan.alpha_stop_i, "alpha_stop_i")?;
        let log_term = (kf + 1.0).ln() - 2.0 * alpha.ln();
        if !log_term.is_finite() || log_term < 0.0 {
            return Err(Error::Invalid("non-finite confidence-sequence term".into()));
        }
        let exact_formula_radius = ((kf + 1.0) * log_term).sqrt() / kf;
        if !mean.is_finite() || !exact_formula_radius.is_finite() {
            return Err(Error::Invalid(
                "non-finite confidence-sequence boundary".into(),
            ));
        }
        // Supported k/alpha are bounded above. This fixed envelope is larger
        // than the observed f64-vs-80-digit reference error scanned by the
        // companion Python fixture; widening prevents a rounding-induced stop.
        let radius = exact_formula_radius + NUMERIC_OUTWARD_ERROR;
        Ok(Some(CsPrefix {
            k,
            mean,
            radius,
            lcb: (mean - radius).max(-1.0),
            ucb: (mean + radius).min(1.0),
        }))
    }

    pub fn completed_prefix_digest(&self) -> Result<String> {
        fingerprint(&self.prefix)
    }

    fn compute_decision(&self) -> Result<EarlyStopDecision> {
        let Some(bound) = self.boundary()? else {
            return Ok(EarlyStopDecision::Collecting);
        };
        let threshold = |micros: u32| f64::from(micros) / f64::from(SCORE_MICROS_MAX);
        if bound.ucb < -threshold(self.plan.max_regression_micros) {
            return Ok(EarlyStopDecision::RegressedSequential);
        }
        match self.plan.profile {
            ProfileKind::QualityGain if bound.ucb < threshold(self.plan.min_gain_micros) => {
                return Ok(EarlyStopDecision::FutilityQualityGain);
            }
            ProfileKind::NoninferiorSavings
                if bound.ucb < -threshold(self.plan.noninferiority_margin_micros) =>
            {
                return Ok(EarlyStopDecision::FutilityNoninferiority);
            }
            _ => {}
        }
        if self.k() == self.plan.n_planned {
            Ok(EarlyStopDecision::FinalFixedSample)
        } else {
            Ok(EarlyStopDecision::Continue)
        }
    }
}

/// Data shape for E05's trusted grader. No public constructor converts caller
/// assertions into a trusted formal certificate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EarlyStopCertificate {
    schema_version: String,
    ticket_id: String,
    v1_plan_snapshot_digest: String,
    early_stop_plan_digest: String,
    alpha_plan_digest: String,
    manifest_digest: String,
    slice_id: String,
    candidate_digest: String,
    baseline_digest: String,
    grader_digest: String,
    issuer_receipt_digest: String,
    k: u32,
    lcb: Option<String>,
    ucb: Option<String>,
    completed_prefix_digest: String,
    member_terminal_digest: String,
    stop_reason: EarlyStopDecision,
    stopped_at_unix_ms: i64,
    stop_sequence: u64,
    usage_uncertain: bool,
}

#[derive(Debug, Clone)]
pub struct EarlyStopCertificateParts {
    pub ticket_id: String,
    pub v1_plan_snapshot_digest: String,
    pub early_stop_plan_digest: String,
    pub alpha_plan_digest: String,
    pub manifest_digest: String,
    pub slice_id: String,
    pub candidate_digest: String,
    pub baseline_digest: String,
    pub grader_digest: String,
    pub issuer_receipt_digest: String,
    pub k: u32,
    pub lcb: Option<String>,
    pub ucb: Option<String>,
    pub completed_prefix_digest: String,
    pub member_terminal_digest: String,
    pub stop_reason: EarlyStopDecision,
    pub stopped_at_unix_ms: i64,
    pub stop_sequence: u64,
    pub usage_uncertain: bool,
}

impl EarlyStopCertificate {
    pub const SCHEMA: &'static str = "rsia.early_stop_certificate.v1";

    /// Constructs the immutable certificate shape after the engine has checked
    /// trusted Store evidence. This validates bytes only and grants no formal
    /// authority; E05 must re-check the persisted issuer/ticket/receipt closure.
    pub fn from_verified_parts(parts: EarlyStopCertificateParts) -> Result<Self> {
        let certificate = Self {
            schema_version: Self::SCHEMA.into(),
            ticket_id: parts.ticket_id,
            v1_plan_snapshot_digest: parts.v1_plan_snapshot_digest,
            early_stop_plan_digest: parts.early_stop_plan_digest,
            alpha_plan_digest: parts.alpha_plan_digest,
            manifest_digest: parts.manifest_digest,
            slice_id: parts.slice_id,
            candidate_digest: parts.candidate_digest,
            baseline_digest: parts.baseline_digest,
            grader_digest: parts.grader_digest,
            issuer_receipt_digest: parts.issuer_receipt_digest,
            k: parts.k,
            lcb: parts.lcb,
            ucb: parts.ucb,
            completed_prefix_digest: parts.completed_prefix_digest,
            member_terminal_digest: parts.member_terminal_digest,
            stop_reason: parts.stop_reason,
            stopped_at_unix_ms: parts.stopped_at_unix_ms,
            stop_sequence: parts.stop_sequence,
            usage_uncertain: parts.usage_uncertain,
        };
        certificate.validate_shape()?;
        Ok(certificate)
    }

    pub fn validate_shape(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported early-stop certificate schema".into(),
            ));
        }
        identifier(&self.ticket_id)?;
        identifier(&self.slice_id)?;
        for (value, name) in [
            (&self.v1_plan_snapshot_digest, "v1_plan_snapshot_digest"),
            (&self.early_stop_plan_digest, "early_stop_plan_digest"),
            (&self.alpha_plan_digest, "alpha_plan_digest"),
            (&self.manifest_digest, "manifest_digest"),
            (&self.candidate_digest, "candidate_digest"),
            (&self.baseline_digest, "baseline_digest"),
            (&self.grader_digest, "grader_digest"),
            (&self.issuer_receipt_digest, "issuer_receipt_digest"),
            (&self.completed_prefix_digest, "completed_prefix_digest"),
            (&self.member_terminal_digest, "member_terminal_digest"),
        ] {
            digest(value, name)?;
        }
        if !self.stop_reason.is_early_stop() {
            return Err(Error::Invalid(
                "certificate requires a reject-only stop reason".into(),
            ));
        }
        if self.stopped_at_unix_ms < 0 || self.stop_sequence == 0 {
            return Err(Error::Invalid(
                "certificate requires a valid stop time and sequence".into(),
            ));
        }
        match (self.stop_reason, self.k, &self.lcb, &self.ucb) {
            (EarlyStopDecision::CriticalRegression, 0, None, None) => {}
            (_, k, Some(lcb), Some(ucb)) if k > 0 => {
                let lcb = canonical_signed_decimal(lcb, "certificate lcb")?;
                let ucb = canonical_signed_decimal(ucb, "certificate ucb")?;
                if !lcb.is_finite() || !ucb.is_finite() || lcb > ucb {
                    return Err(Error::Invalid("invalid certificate bounds".into()));
                }
            }
            _ => {
                return Err(Error::Invalid(
                    "certificate prefix and bounds disagree".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn is_promotable(&self) -> bool {
        false
    }

    pub fn ticket_id(&self) -> &str {
        &self.ticket_id
    }

    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    pub fn v1_plan_snapshot_digest(&self) -> &str {
        &self.v1_plan_snapshot_digest
    }

    pub fn alpha_plan_digest(&self) -> &str {
        &self.alpha_plan_digest
    }

    pub fn candidate_digest(&self) -> &str {
        &self.candidate_digest
    }

    pub fn baseline_digest(&self) -> &str {
        &self.baseline_digest
    }

    pub fn grader_digest(&self) -> &str {
        &self.grader_digest
    }

    pub fn slice_id(&self) -> &str {
        &self.slice_id
    }

    pub fn issuer_receipt_digest(&self) -> &str {
        &self.issuer_receipt_digest
    }

    pub fn member_terminal_digest(&self) -> &str {
        &self.member_terminal_digest
    }

    pub fn stop_reason(&self) -> EarlyStopDecision {
        self.stop_reason
    }

    pub fn k(&self) -> u32 {
        self.k
    }

    pub fn usage_uncertain(&self) -> bool {
        self.usage_uncertain
    }

    /// Returns a new certificate view after E05 has reconciled every
    /// dispatched call. The original stop reason, prefix and sequence remain
    /// immutable; this method does not itself prove that reconciliation.
    pub fn with_usage_reconciled(mut self) -> Result<Self> {
        self.usage_uncertain = false;
        self.validate_shape()?;
        Ok(self)
    }
}
