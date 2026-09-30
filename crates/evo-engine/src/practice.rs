//! E09 same-task comparison practice (v4.2 §8.5; V093, V094.b, V096.a): the
//! program sub-scope over the registered pure-function development runner.
//!
//! A practice set re-runs the authorized development tasks of one Admin-
//! registered control K times, strictly in series, and keeps every attempt:
//!
//! * K=1 is the default. It performs exactly one attempt and is always
//!   `no_contrast`: a single attempt has nothing to contrast, so no comparison
//!   call is added and no registration is needed.
//! * K=3 must be registered by an Admin ([`register_practice`]) and the plan
//!   must cite that persisted registration. A plan covers at most two tasks,
//!   and they must be exactly the tasks of the control, so the set shares the
//!   control's root budget and cannot spend on an unregistered task.
//! * Attempt n uses a request id derived from the set id and n. A new set never
//!   hits an old attempt (V094.b); re-running the same request returns the
//!   stored set and sends nothing (an uncertain attempt is never resent).
//! * The set never counts as an independent sample (V093.c): its
//!   `independent_clusters` is the number of distinct task parent families, K
//!   is never part of it, and `counts_as_independent_samples` is always false.
//! * A contrast is only a development hypothesis (§8.5): it names the better
//!   and the worse attempt of one task and is never evidence or a formal n.
//!
//! # What a practice attempt is not
//!
//! An attempt is **not** a development cycle. No `DevelopmentRequestPrepared`,
//! `DevelopmentObserved` or other stage fact is written for it, so the E12
//! `record_cycle` and the E13 cycle close can neither see nor count it
//! (V093.c). Everything an attempt proves is reloaded from the E03 receipts by
//! the E03 verifier, fed with a transient carrier that is never sealed,
//! committed or stored.
//!
//! # Known limits (disclosed, not hidden)
//!
//! * The registered runner executes a Parent and a Candidate side for every
//!   task. A practice attempt has no such split, so both sides run the same
//!   bundle and only the Candidate side is adopted ("both sides executed, one
//!   side used"). The duplicate side is the price of not touching the runner.
//! * Budget rows stay `DevelopmentExecution`: `BudgetStage::Practice` is not
//!   used because the registered settlement accepts development stages only.
//!   The curriculum's 20 % share is not charged here either.
//! * The registered runner is a deterministic pure function, so K attempts of
//!   one control always agree and the real outcome is `no_contrast`. There is
//!   no model sampling, hence no sampling configuration and no real cost.
//! * A contrast does not feed the bounded edit chain (§6.7/§5.3.1), and there
//!   is no management entry point.
//! * A registration authorizes a plan (cluster, tasks, K) and carries no
//!   failure/instability/coverage basis. Only [`register_practice`] is gated to
//!   Admin: like every engine artifact, a raw storage write that bypasses it
//!   cannot be told apart, because the store has no owner readback.

use crate::development::{
    CONTROL_KIND, DevelopmentControlV1, DevelopmentCostState, DevelopmentEvidenceScope,
    DevelopmentSide, EXECUTION_RECEIPT_KIND, RUN_RECEIPT_KIND, execution_receipt_id,
    load_development_control, load_execution_receipt, load_run_receipt, run_receipt_id, storage_id,
    typed_receipt_closure, verify_observed_report_in_session,
};
use crate::evidence::validate_stored_sources;
use crate::optimization::{
    DevRunner, DevelopmentExecutionProvenance, DevelopmentRunReport, DevelopmentRunRequest,
    OPTIMIZATION_STAGE_FACT_SCHEMA, OptimizationJournalStage, StageFact, StageFactKind,
    select_development, validate_development_request,
};
use evo_core::evidence::Purpose;
use evo_core::strategy::PracticePlan;
use evo_core::{Context, Error, Result, Role, fingerprint, identifier};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PRACTICE_REGISTRATION_SCHEMA: &str = "rsia.practice_registration.v1";
pub const PRACTICE_ATTEMPT_SET_SCHEMA: &str = "rsia.practice_attempt_set.v1";
/// The only expanded K. K=1 is the default and needs no registration.
pub const REGISTERED_PRACTICE_ATTEMPTS: u8 = 3;

const ARTIFACT_KIND: &str = "artifact";
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";
const MAX_SCORE_MICROS: u32 = 1_000_000;

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

fn valid_time(value: i64) -> Result<()> {
    if value < 0 {
        return Err(Error::Invalid("timestamp must be nonnegative".into()));
    }
    Ok(())
}

/// Storage id of an Admin practice registration artifact. The store requires an
/// object's `id` to be its key, and the registration's `id` is the plan's
/// authorization digest, so the two are the same 64-hex string.
pub fn practice_registration_storage_id(registration_id: &str) -> Result<String> {
    digest(registration_id, "practice registration id")?;
    Ok(registration_id.to_string())
}

/// Storage id of a practice attempt set artifact, namespaced and hashed like
/// every other engine artifact id so a set id can never overwrite another
/// object.
pub fn practice_attempt_set_storage_id(set_id: &str) -> Result<String> {
    identifier(set_id)?;
    Ok(format!(
        "practice-set-{}",
        fingerprint(&(PRACTICE_ATTEMPT_SET_SCHEMA, set_id))?
    ))
}

/// Request id of attempt `attempt` (1-based) of a set. It is a pure function of
/// the set id and the attempt number: a new set id yields new request ids, so a
/// new deliberate practice can never hit an old attempt (V094.b), and the same
/// set re-derives the same ids, so a reconnect reaches the original evidence.
pub fn practice_attempt_request_id(set_id: &str, attempt: u32) -> Result<String> {
    identifier(set_id)?;
    if attempt == 0 || attempt > u32::from(REGISTERED_PRACTICE_ATTEMPTS) {
        return Err(Error::Invalid("practice attempt is outside 1..=3".into()));
    }
    Ok(format!(
        "practice-{}",
        fingerprint(&("rsia.practice_attempt_request.v1", set_id, attempt))?
    ))
}

/// Admin registration of a K=3 comparison practice.
///
/// `id` **is** the plan's `authorization_digest`. The alternative, the digest
/// of this record, is circular: the record holds `plan_digest`, which is the
/// fingerprint of a plan that would already contain that digest. `id` is
/// chosen by the Admin before the plan is built, so the plan can cite it, and
/// an id is registered once: the same id with other content is a `Conflict`.
/// The free digest inside a plan is therefore never trusted on its own; it only
/// names a record that must exist and must bind exactly that plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PracticeRegistrationV1 {
    pub schema_version: String,
    pub id: String,
    /// `fingerprint(PracticePlan)` of the authorized plan.
    pub plan_digest: String,
    pub task_ids: Vec<String>,
    pub k: u8,
    pub parent_cluster_id: String,
    pub registered_by: String,
    pub created_at: i64,
}

impl PracticeRegistrationV1 {
    pub const SCHEMA: &'static str = PRACTICE_REGISTRATION_SCHEMA;

    /// Builds the registration of a K=3 plan: its `authorization_digest`
    /// becomes the registration id.
    pub fn for_plan(
        plan: &PracticePlan,
        registered_by: impl Into<String>,
        created_at: i64,
    ) -> Result<Self> {
        plan.validate()?;
        if plan.attempts_per_task != REGISTERED_PRACTICE_ATTEMPTS {
            return Err(Error::Invalid(
                "only a K=3 practice plan is registered; K=1 needs no registration".into(),
            ));
        }
        let id = plan.authorization_digest.clone().ok_or_else(|| {
            Error::Invalid("a K=3 practice plan must carry its authorization digest".into())
        })?;
        let registration = Self {
            schema_version: Self::SCHEMA.into(),
            id,
            plan_digest: fingerprint(plan)?,
            task_ids: plan.task_ids.clone(),
            k: plan.attempts_per_task,
            parent_cluster_id: plan.parent_cluster_id.clone(),
            registered_by: registered_by.into(),
            created_at,
        };
        registration.validate()?;
        Ok(registration)
    }

    /// The plan this record authorizes, rebuilt from its own fields.
    fn registered_plan(&self) -> Result<PracticePlan> {
        if self.k != REGISTERED_PRACTICE_ATTEMPTS {
            return Err(Error::Invalid(
                "a practice registration authorizes K=3 only".into(),
            ));
        }
        PracticePlan::new(
            self.parent_cluster_id.clone(),
            self.task_ids.clone(),
            self.k,
            Some(self.id.clone()),
        )
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported practice registration schema".into(),
            ));
        }
        digest(&self.id, "practice registration id")?;
        digest(&self.plan_digest, "practice registration plan_digest")?;
        identifier(&self.registered_by)?;
        valid_time(self.created_at)?;
        if fingerprint(&self.registered_plan()?)? != self.plan_digest {
            return Err(Error::Conflict(
                "practice registration plan digest differs from its own plan".into(),
            ));
        }
        Ok(())
    }

    /// Whether this record authorizes exactly `plan`.
    pub fn binds_plan(&self, plan: &PracticePlan) -> Result<()> {
        if plan.authorization_digest.as_deref() != Some(self.id.as_str())
            || plan.attempts_per_task != self.k
            || plan.parent_cluster_id != self.parent_cluster_id
            || plan.task_ids != self.task_ids
            || fingerprint(plan)? != self.plan_digest
        {
            return Err(Error::Conflict(
                "practice registration does not authorize this plan".into(),
            ));
        }
        Ok(())
    }
}

/// One practice request. The bundle, environment and grader are frozen by the
/// control; the request adds only the set identity, the plan and the fixed
/// bundle under test.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PracticeRunRequest {
    pub set_id: String,
    pub control_id: String,
    pub plan: PracticePlan,
    /// Dispatch group of every attempt's budget rows. Callers should give a
    /// practice set its own episode id.
    pub episode_id: String,
    pub step: u32,
    /// The one fixed Skill bundle under test, used for both runner sides.
    pub bundle_digest: String,
    pub revoke_watermark: u64,
    pub created_at: i64,
}

impl PracticeRunRequest {
    /// Identity of the request: everything that decides what is executed.
    /// `created_at` is a stamp, not part of the actual request, so a reconnect
    /// that stamps a new time still reaches the original set (V094.b).
    pub fn digest(&self) -> Result<String> {
        fingerprint(&(
            "rsia.practice_run_request.v1",
            &self.set_id,
            &self.control_id,
            &self.plan,
            &self.episode_id,
            self.step,
            &self.bundle_digest,
            self.revoke_watermark,
        ))
    }

    pub fn validate(&self) -> Result<()> {
        for value in [&self.set_id, &self.control_id, &self.episode_id] {
            identifier(value)?;
        }
        digest(&self.bundle_digest, "practice bundle_digest")?;
        valid_time(self.created_at)?;
        self.plan.validate()?;
        if self.plan.attempts_per_task != REGISTERED_PRACTICE_ATTEMPTS
            && self.plan.authorization_digest.is_some()
        {
            return Err(Error::Invalid(
                "a K=1 practice plan carries no authorization".into(),
            ));
        }
        Ok(())
    }
}

/// The exact development request of attempt `attempt` (1-based). Both runner
/// sides get the set's one bundle ("both sides executed, one side used"); the
/// environment, grader, rules, tools and manifest come from the control.
pub fn practice_attempt_request(
    control: &DevelopmentControlV1,
    request: &PracticeRunRequest,
    attempt: u32,
) -> Result<DevelopmentRunRequest> {
    let request_id = practice_attempt_request_id(&request.set_id, attempt)?;
    let development = DevelopmentRunRequest {
        request_id: request_id.clone(),
        namespace: control.namespace.clone(),
        purpose: Purpose::Development,
        episode_id: request.episode_id.clone(),
        step: request.step,
        attempt,
        manifest: control.manifest()?,
        parent_bundle_digest: request.bundle_digest.clone(),
        candidate_bundle_digest: request.bundle_digest.clone(),
        environment_digest: control.environment_digest.clone(),
        grader_digest: control.grader_digest.clone(),
        rules_digest: control.rules_digest.clone(),
        tools_digest: control.tools_digest.clone(),
        revoke_watermark: request.revoke_watermark,
        idempotency_key: format!("{request_id}-idem"),
    };
    validate_development_request(&development)?;
    Ok(development)
}

/// The adopted (Candidate side) result of one task in one attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PracticeTaskResultV1 {
    pub task_id: String,
    pub parent_family: String,
    pub passed: bool,
    pub score_micros: u32,
    pub output_digest: String,
}

impl PracticeTaskResultV1 {
    fn validate(&self) -> Result<()> {
        identifier(&self.task_id)?;
        identifier(&self.parent_family)?;
        digest(&self.output_digest, "practice task output_digest")?;
        if self.score_micros > MAX_SCORE_MICROS {
            return Err(Error::Invalid(
                "practice score micros must be within 0..=1000000".into(),
            ));
        }
        Ok(())
    }
}

/// One attempt. There is no sampling configuration: the registered runner is a
/// deterministic pure function, so there is nothing to sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PracticeAttemptV1 {
    /// 1-based position in the set.
    pub attempt: u32,
    pub request_id: String,
    /// The attempt's E03 run receipt (`DevelopmentRunReport::execution_receipt_id`).
    pub execution_receipt_id: String,
    pub tasks: Vec<PracticeTaskResultV1>,
    /// Cost of every execution of the attempt (both runner sides).
    /// `Uncertain` as soon as any execution's cost is. For a cache hit it
    /// describes the original execution and is not new spend.
    pub cost_state: DevelopmentCostState,
    /// The attempt's run receipt already existed before the runner was called:
    /// the historical evidence is reused and this is neither a new practice nor
    /// a new paid call.
    pub cache_hit: bool,
}

impl PracticeAttemptV1 {
    fn validate(&self, set_id: &str, position: usize, task_ids: &BTreeSet<&str>) -> Result<()> {
        if usize::try_from(self.attempt).ok() != Some(position) {
            return Err(Error::Conflict(
                "practice attempts must be numbered 1.. in order".into(),
            ));
        }
        if self.request_id != practice_attempt_request_id(set_id, self.attempt)?
            || self.execution_receipt_id != run_receipt_id(&self.request_id)?
        {
            return Err(Error::Conflict(
                "practice attempt ids are not derived from its set".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        for task in &self.tasks {
            task.validate()?;
            if !seen.insert(task.task_id.as_str()) {
                return Err(Error::Conflict("practice attempt repeats a task".into()));
            }
        }
        if &seen != task_ids {
            return Err(Error::Conflict(
                "practice attempt tasks differ from the plan".into(),
            ));
        }
        if let DevelopmentCostState::Known {
            micros,
            currency,
            pricing_version,
        } = &self.cost_state
        {
            identifier(currency)?;
            identifier(pricing_version)?;
            if *micros < 0 {
                return Err(Error::Invalid("practice cost must be nonnegative".into()));
            }
        }
        Ok(())
    }
}

/// Terminal state of a set. `Contrast` is a development hypothesis only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PracticeOutcomeV1 {
    /// `task_id` has a better and a worse attempt under the frozen development
    /// goal (higher score, then pass). Never evidence, never a formal n.
    Contrast {
        better_attempt: u32,
        worse_attempt: u32,
        task_id: String,
    },
    /// No valid difference. K=1 is always this; K is never raised to find one.
    NoContrast,
    /// An attempt was uncertain, the budget ran out or an error stopped the
    /// set. Later attempts were not run; finished attempts are kept.
    Incomplete { reason: String },
}

/// A K-attempt practice over one control. Persisted as an immutable artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PracticeAttemptSetV1 {
    pub schema_version: String,
    /// Storage id, `practice_attempt_set_storage_id(set_id)`.
    pub id: String,
    pub set_id: String,
    pub namespace: String,
    pub control_id: String,
    pub episode_id: String,
    pub step: u32,
    pub bundle_digest: String,
    pub environment_digest: String,
    pub revoke_watermark: u64,
    /// `PracticeRunRequest::digest`: the same request is the same set.
    pub request_digest: String,
    pub plan: PracticePlan,
    /// The registration a K=3 plan cited and that was verified; `None` for K=1.
    pub registration_id: Option<String>,
    pub attempts: Vec<PracticeAttemptV1>,
    pub outcome: PracticeOutcomeV1,
    /// Distinct task parent families. K is never part of it (V093.c).
    pub independent_clusters: u32,
    /// Always false: K attempts stay in one original cluster (V093.c).
    pub counts_as_independent_samples: bool,
    pub created_at: i64,
}

impl PracticeAttemptSetV1 {
    pub const SCHEMA: &'static str = PRACTICE_ATTEMPT_SET_SCHEMA;

    /// Attempts that really ran; a cache hit is reused history, not a new
    /// practice.
    pub fn new_practice_attempts(&self) -> usize {
        self.attempts
            .iter()
            .filter(|attempt| !attempt.cache_hit)
            .count()
    }

    /// Structural and semantic checks of a stored or freshly built set,
    /// including that the outcome is exactly what its attempts imply.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported practice attempt set schema".into(),
            ));
        }
        if self.counts_as_independent_samples {
            return Err(Error::Conflict(
                "a practice attempt set never counts as independent samples".into(),
            ));
        }
        for value in [
            &self.id,
            &self.set_id,
            &self.namespace,
            &self.control_id,
            &self.episode_id,
        ] {
            identifier(value)?;
        }
        if self.id != practice_attempt_set_storage_id(&self.set_id)? {
            return Err(Error::Conflict(
                "practice attempt set id is not derived from its set id".into(),
            ));
        }
        for (value, name) in [
            (&self.bundle_digest, "practice bundle_digest"),
            (&self.environment_digest, "practice environment_digest"),
            (&self.request_digest, "practice request_digest"),
        ] {
            digest(value, name)?;
        }
        valid_time(self.created_at)?;
        self.plan.validate()?;
        let k = usize::from(self.plan.attempts_per_task);
        match (&self.registration_id, &self.plan.authorization_digest) {
            (Some(registration), Some(authorization))
                if k == usize::from(REGISTERED_PRACTICE_ATTEMPTS)
                    && registration == authorization => {}
            (None, None) if k == 1 => {}
            _ => {
                return Err(Error::Conflict(
                    "practice set registration differs from its plan".into(),
                ));
            }
        }
        if self.attempts.len() > k {
            return Err(Error::Conflict(
                "practice set holds more attempts than its K".into(),
            ));
        }
        let task_ids: BTreeSet<&str> = self.plan.task_ids.iter().map(String::as_str).collect();
        for (index, attempt) in self.attempts.iter().enumerate() {
            attempt.validate(&self.set_id, index + 1, &task_ids)?;
        }
        let clusters = usize::try_from(self.independent_clusters).map_err(|_| Error::Internal)?;
        let order = |attempt: &PracticeAttemptV1| -> Vec<(String, String)> {
            attempt
                .tasks
                .iter()
                .map(|task| (task.task_id.clone(), task.parent_family.clone()))
                .collect()
        };
        if let Some(first) = self.attempts.first() {
            let expected = order(first);
            if self
                .attempts
                .iter()
                .any(|attempt| order(attempt) != expected)
            {
                return Err(Error::Conflict(
                    "practice attempts disagree on their tasks or families".into(),
                ));
            }
            let families: BTreeSet<&str> = first
                .tasks
                .iter()
                .map(|task| task.parent_family.as_str())
                .collect();
            if clusters != families.len() {
                return Err(Error::Conflict(
                    "practice independent clusters differ from the task families".into(),
                ));
            }
        } else if clusters == 0 || clusters > self.plan.task_ids.len() {
            return Err(Error::Conflict(
                "practice independent clusters are outside the task count".into(),
            ));
        }
        let uncertain = self
            .attempts
            .iter()
            .position(|attempt| attempt.cost_state == DevelopmentCostState::Uncertain);
        if uncertain.is_some_and(|index| index + 1 != self.attempts.len()) {
            return Err(Error::Conflict(
                "a practice set continued after an uncertain attempt".into(),
            ));
        }
        match &self.outcome {
            PracticeOutcomeV1::Incomplete { reason } => {
                identifier(reason)?;
                if self.attempts.len() == k && uncertain.is_none() {
                    return Err(Error::Conflict(
                        "an incomplete practice set ran all of its attempts".into(),
                    ));
                }
            }
            outcome => {
                if self.attempts.len() != k || uncertain.is_some() {
                    return Err(Error::Conflict(
                        "a finished practice set holds all K certain attempts".into(),
                    ));
                }
                if *outcome != select_contrast(&self.attempts) {
                    return Err(Error::Conflict(
                        "practice outcome differs from its attempts".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Picks the contrast of a finished set under the frozen development goal.
///
/// For each task, in the control's task order, the attempts are ranked by
/// (score, passed): the higher score is better, equal scores compare `passed`,
/// and still equal means no difference. The first task whose best and worst
/// attempt differ yields the contrast; ties keep the earliest attempt. No
/// differing task, or a single attempt, is `NoContrast`. Cache-hit attempts
/// take part: a cache hit is historical evidence that may be reused.
pub fn select_contrast(attempts: &[PracticeAttemptV1]) -> PracticeOutcomeV1 {
    let Some(first) = attempts.first() else {
        return PracticeOutcomeV1::NoContrast;
    };
    for task in &first.tasks {
        let mut better: Option<(u32, (u32, bool))> = None;
        let mut worse: Option<(u32, (u32, bool))> = None;
        for attempt in attempts {
            let Some(result) = attempt
                .tasks
                .iter()
                .find(|result| result.task_id == task.task_id)
            else {
                continue;
            };
            let key = (result.score_micros, result.passed);
            if better.is_none_or(|(_, best)| key > best) {
                better = Some((attempt.attempt, key));
            }
            if worse.is_none_or(|(_, worst)| key < worst) {
                worse = Some((attempt.attempt, key));
            }
        }
        if let (Some((better_attempt, best)), Some((worse_attempt, worst))) = (better, worse)
            && best > worst
        {
            return PracticeOutcomeV1::Contrast {
                better_attempt,
                worse_attempt,
                task_id: task.task_id.clone(),
            };
        }
    }
    PracticeOutcomeV1::NoContrast
}

/// Total cost of all executions of one attempt. Anything uncertain, a mixed
/// pricing identity, an overflow or no evidence at all is `Uncertain`.
fn aggregate_cost(states: &[DevelopmentCostState]) -> DevelopmentCostState {
    let mut total = 0i64;
    let mut identity: Option<(&str, &str)> = None;
    for state in states {
        let DevelopmentCostState::Known {
            micros,
            currency,
            pricing_version,
        } = state
        else {
            return DevelopmentCostState::Uncertain;
        };
        match identity {
            None => identity = Some((currency, pricing_version)),
            Some((known_currency, known_pricing))
                if known_currency == currency && known_pricing == pricing_version => {}
            Some(_) => return DevelopmentCostState::Uncertain,
        }
        let Some(next) = total.checked_add(*micros) else {
            return DevelopmentCostState::Uncertain;
        };
        total = next;
    }
    match identity {
        Some((currency, pricing_version)) => DevelopmentCostState::Known {
            micros: total,
            currency: currency.into(),
            pricing_version: pricing_version.into(),
        },
        None => DevelopmentCostState::Uncertain,
    }
}

fn error_class(error: &Error) -> &'static str {
    match error {
        Error::Invalid(_) => "invalid",
        Error::Forbidden => "forbidden",
        Error::NotFound => "not_found",
        Error::Conflict(_) => "conflict",
        Error::Budget => "budget",
        Error::Cancelled => "cancelled",
        Error::Internal => "internal",
    }
}

/// Why a runner error stops the set. The E03 orchestrator treats every runner
/// error as an unknown outcome and forbids an automatic retry; a set does the
/// same: it stops, keeps what finished and is never resent.
fn runner_stop_reason(attempt: u32, error: &Error) -> String {
    match error {
        Error::Budget => format!("budget_exhausted:{attempt}"),
        // The execution was cancelled or its lease was lost: whether the call
        // reached its target is unknown.
        Error::Cancelled => format!("uncertain_attempt:{attempt}"),
        other => format!("attempt_error:{attempt}:{}", error_class(other)),
    }
}

/// Tombstones fail closed as Forbidden before the watermark comparison, as in
/// the E03 gate, so a revoked source is a permission failure rather than drift.
async fn require_live_sources(
    session: &mut Session,
    ctx: &Context,
    source_ids: &[String],
    watermark: u64,
) -> Result<()> {
    require_no_tombstone(session, ctx, source_ids).await?;
    validate_stored_sources(session, ctx, source_ids, watermark).await
}

async fn require_no_tombstone(
    session: &mut Session,
    ctx: &Context,
    source_ids: &[String],
) -> Result<()> {
    for id in source_ids {
        if session
            .get::<serde_json::Value>(ctx, "tombstone", id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}

fn parse_registration(id: &str, value: serde_json::Value) -> Result<PracticeRegistrationV1> {
    match value
        .get("schema_version")
        .and_then(|schema| schema.as_str())
    {
        Some(PRACTICE_REGISTRATION_SCHEMA) => {}
        Some(REDACTED_SCHEMA) => {
            return Err(Error::Conflict("practice registration was redacted".into()));
        }
        _ => {
            return Err(Error::Conflict(
                "stored object is not a practice registration".into(),
            ));
        }
    }
    let registration: PracticeRegistrationV1 = serde_json::from_value(value)
        .map_err(|_| Error::Conflict("practice registration is not strict".into()))?;
    registration.validate()?;
    if registration.id != id {
        return Err(Error::Conflict(
            "practice registration identity mismatch".into(),
        ));
    }
    Ok(registration)
}

fn parse_set(ctx: &Context, id: &str, value: serde_json::Value) -> Result<PracticeAttemptSetV1> {
    match value
        .get("schema_version")
        .and_then(|schema| schema.as_str())
    {
        Some(PRACTICE_ATTEMPT_SET_SCHEMA) => {}
        Some(REDACTED_SCHEMA) => {
            return Err(Error::Conflict(
                "practice attempt set was redacted after source revocation".into(),
            ));
        }
        _ => {
            return Err(Error::Conflict(
                "stored object is not a practice attempt set".into(),
            ));
        }
    }
    let set: PracticeAttemptSetV1 = serde_json::from_value(value)
        .map_err(|_| Error::Conflict("practice attempt set is not strict".into()))?;
    set.validate()?;
    if set.set_id != id || set.namespace != ctx.namespace() {
        return Err(Error::Conflict(
            "practice attempt set identity mismatch".into(),
        ));
    }
    Ok(set)
}

/// Registers a K=3 practice. Admin only; the registering actor is the
/// Admin's own (`registered_by` cannot name someone else). Registration is
/// idempotent for identical content only.
pub async fn register_practice(
    ctx: &Context,
    store: &Store,
    registration: PracticeRegistrationV1,
) -> Result<String> {
    ctx.require(&[Role::Admin])?;
    registration.validate()?;
    if registration.registered_by != ctx.actor() {
        return Err(Error::Forbidden);
    }
    let storage = practice_registration_storage_id(&registration.id)?;
    let mut session = store.session().await?;
    if let Some(value) = session
        .get::<serde_json::Value>(ctx, ARTIFACT_KIND, &storage)
        .await?
    {
        let existing = parse_registration(&registration.id, value)?;
        if fingerprint(&existing)? != fingerprint(&registration)? {
            return Err(Error::Conflict(
                "practice registration already registered with different content".into(),
            ));
        }
        session.commit().await?;
        return Ok(registration.id);
    }
    session
        .put(ctx, ARTIFACT_KIND, &storage, ctx.actor(), &registration)
        .await?;
    session
        .audit(ctx, "practice.registration.register", &registration.id)
        .await?;
    session.commit().await?;
    Ok(registration.id)
}

pub async fn load_practice_registration(
    store: &Store,
    ctx: &Context,
    registration_id: &str,
) -> Result<PracticeRegistrationV1> {
    let storage = practice_registration_storage_id(registration_id)?;
    let mut session = store.session().await?;
    let value = session
        .get::<serde_json::Value>(ctx, ARTIFACT_KIND, &storage)
        .await?;
    session.commit().await?;
    parse_registration(registration_id, value.ok_or(Error::NotFound)?)
}

/// Reads a persisted set. A set redacted by a source revocation is reported as
/// a `Conflict`, never as a storage failure.
pub async fn load_practice_attempt_set(
    store: &Store,
    ctx: &Context,
    set_id: &str,
) -> Result<PracticeAttemptSetV1> {
    load_existing_set(store, ctx, set_id)
        .await?
        .ok_or(Error::NotFound)
}

async fn load_existing_set(
    store: &Store,
    ctx: &Context,
    set_id: &str,
) -> Result<Option<PracticeAttemptSetV1>> {
    let storage = practice_attempt_set_storage_id(set_id)?;
    let mut session = store.session().await?;
    let value = session
        .get::<serde_json::Value>(ctx, ARTIFACT_KIND, &storage)
        .await?;
    session.commit().await?;
    value.map(|value| parse_set(ctx, set_id, value)).transpose()
}

/// A K=3 plan must cite a persisted registration that authorizes exactly it.
/// A missing registration is a permission failure, never a silent K=1.
async fn verify_registration(
    store: &Store,
    ctx: &Context,
    plan: &PracticePlan,
) -> Result<PracticeRegistrationV1> {
    let id = plan
        .authorization_digest
        .as_deref()
        .ok_or(Error::Forbidden)?;
    let registration = match load_practice_registration(store, ctx, id).await {
        Ok(registration) => registration,
        Err(Error::NotFound) => return Err(Error::Forbidden),
        Err(error) => return Err(error),
    };
    registration.binds_plan(plan)?;
    Ok(registration)
}

/// The plan's tasks must be exactly the control's tasks: a practice set cannot
/// spend the shared root budget on a task nobody registered, and a control with
/// more than two tasks cannot be practised under a two-task plan.
fn ensure_plan_matches_control(plan: &PracticePlan, control: &DevelopmentControlV1) -> Result<()> {
    let planned: BTreeSet<&str> = plan.task_ids.iter().map(String::as_str).collect();
    let registered: BTreeSet<&str> = control
        .tasks
        .iter()
        .map(|task| task.task_id.as_str())
        .collect();
    if planned != registered {
        return Err(Error::Conflict(
            "practice plan tasks differ from the registered development control".into(),
        ));
    }
    Ok(())
}

/// Carrier for the E03 verifier. It cross-checks a report against the typed
/// receipt closure a `DevelopmentObserved` fact declares; this holds exactly
/// that closure so the same verifier is reused rather than copied. It is never
/// sealed, committed or stored: a practice attempt has no stage fact (V093.c).
fn verification_carrier(
    request: &DevelopmentRunRequest,
    report: &DevelopmentRunReport,
) -> Result<StageFact> {
    Ok(StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: request.namespace.clone(),
        episode_id: request.episode_id.clone(),
        step: request.step,
        attempt: request.attempt,
        stage: OptimizationJournalStage::Development,
        kind: StageFactKind::DevelopmentObserved,
        request_id: request.request_id.clone(),
        input_digest: request.manifest.digest.clone(),
        output_digest: None,
        dependencies: typed_receipt_closure(report)?,
        payload: serde_json::Value::Null,
    })
}

/// Turns a runner report into an attempt. Nothing the report claims is
/// trusted: its receipts, budget rows, scores and live sources are reloaded by
/// the E03 verifier, and the output digests and costs come from the stored
/// receipts.
async fn record_attempt(
    ctx: &Context,
    store: &Store,
    control_id: &str,
    request: &DevelopmentRunRequest,
    report: &DevelopmentRunReport,
    cache_hit: bool,
) -> Result<PracticeAttemptV1> {
    if report.provenance == DevelopmentExecutionProvenance::Fixture {
        return Err(Error::Forbidden);
    }
    if report.execution_receipt_id != run_receipt_id(&request.request_id)? {
        return Err(Error::Conflict(
            "practice attempt run receipt is not derived from its request".into(),
        ));
    }
    select_development(request, report)?;
    let carrier = verification_carrier(request, report)?;
    let outcomes = {
        let mut session = store.session().await?;
        let outcomes =
            verify_observed_report_in_session(ctx, &mut session, &carrier, request, report).await?;
        session.commit().await?;
        outcomes
    };
    let mut tasks = Vec::with_capacity(outcomes.len());
    let mut costs = Vec::with_capacity(outcomes.len() * 2);
    for outcome in &outcomes {
        let parent = load_execution_receipt(store, ctx, &outcome.parent_execution_id).await?;
        let candidate = load_execution_receipt(store, ctx, &outcome.candidate_execution_id).await?;
        // The set names one control and so one root budget: executions that a
        // runner bound to another control paid for are not this set's evidence.
        if parent.control_id != control_id || candidate.control_id != control_id {
            return Err(Error::Conflict(
                "practice attempt receipts belong to another control".into(),
            ));
        }
        costs.push(parent.cost_state);
        costs.push(candidate.cost_state);
        tasks.push(PracticeTaskResultV1 {
            task_id: outcome.task_id.clone(),
            parent_family: outcome.parent_family.clone(),
            passed: outcome.candidate_passed,
            score_micros: outcome.candidate_score_micros,
            output_digest: candidate.output_digest,
        });
    }
    Ok(PracticeAttemptV1 {
        attempt: request.attempt,
        request_id: request.request_id.clone(),
        execution_receipt_id: report.execution_receipt_id.clone(),
        tasks,
        cost_state: aggregate_cost(&costs),
        cache_hit,
    })
}

/// Whether the attempt's run receipt already exists: the runner will only hand
/// back stored evidence (V094.b, "reconnect retrieves the original result").
async fn attempt_already_executed(store: &Store, ctx: &Context, request_id: &str) -> Result<bool> {
    match load_run_receipt(store, ctx, &run_receipt_id(request_id)?).await {
        Ok(_) => Ok(true),
        Err(Error::NotFound) => Ok(false),
        Err(error) => Err(error),
    }
}

/// An attempt whose cost is uncertain has an unknown outcome: the set stops
/// there and the attempt is never resent.
fn uncertain_stop(attempt: &PracticeAttemptV1) -> Option<String> {
    (attempt.cost_state == DevelopmentCostState::Uncertain)
        .then(|| format!("uncertain_attempt:{}", attempt.attempt))
}

/// Runs attempts 1..=k strictly in series and stops at the first runner
/// error, unverifiable report or uncertain attempt. The returned attempts are
/// the ones that finished, in order; the stop reason says why the rest did not
/// run. Errors of the surrounding store are propagated, not turned into a stop.
async fn run_attempts(
    ctx: &Context,
    store: &Store,
    runner: &dyn DevRunner,
    control: &DevelopmentControlV1,
    request: &PracticeRunRequest,
    k: u32,
) -> Result<(Vec<PracticeAttemptV1>, Option<String>)> {
    let mut attempts = Vec::new();
    for number in 1..=k {
        let development = practice_attempt_request(control, request, number)?;
        let cache_hit = attempt_already_executed(store, ctx, &development.request_id).await?;
        let report = match runner.run(development.clone()).await {
            Ok(report) => report,
            Err(error) => return Ok((attempts, Some(runner_stop_reason(number, &error)))),
        };
        match record_attempt(ctx, store, &control.id, &development, &report, cache_hit).await {
            Ok(attempt) => {
                let stop = uncertain_stop(&attempt);
                attempts.push(attempt);
                if stop.is_some() {
                    return Ok((attempts, stop));
                }
            }
            Err(error) => {
                let reason = format!("attempt_unverified:{number}:{}", error_class(&error));
                return Ok((attempts, Some(reason)));
            }
        }
    }
    Ok((attempts, None))
}

/// Runs (or returns) one practice set.
///
/// `ctx` is a Worker or Admin of the control's namespace; it reads the
/// control, receipts and registration and writes the set. The runner executes
/// with its own contexts. Everything that can be refused is refused before the
/// first runner call: role, request shape, control, task set, registration and
/// live sources. Then attempts run one at a time and in order; an uncertain
/// attempt, an exhausted budget or any runner error stops the set as
/// `Incomplete` (finished attempts are kept, nothing is resent; trying again
/// means a new set id, a new deliberate practice that never hits the stopped
/// attempts). The set is written as an immutable artifact with dependency edges
/// to the control's source runs, the control, every attempt's receipts and the
/// registration, so a source revocation reaches it. No stage fact of any kind
/// is written.
pub async fn run_practice_set(
    ctx: &Context,
    store: &Store,
    runner: &dyn DevRunner,
    request: PracticeRunRequest,
) -> Result<PracticeAttemptSetV1> {
    ctx.require(&[Role::Worker, Role::Admin])?;
    request.validate()?;
    let request_digest = request.digest()?;
    let control = load_development_control(store, ctx, &request.control_id).await?;
    if let Some(existing) = load_existing_set(store, ctx, &request.set_id).await? {
        if existing.request_digest != request_digest {
            return Err(Error::Conflict(
                "practice set id is already recorded for a different request".into(),
            ));
        }
        let mut session = store.session().await?;
        require_no_tombstone(&mut session, ctx, &control.source_ids).await?;
        session.commit().await?;
        return Ok(existing);
    }
    if control.evidence_scope != DevelopmentEvidenceScope::RegisteredPureFunctionExecution {
        return Err(Error::Forbidden);
    }
    ensure_plan_matches_control(&request.plan, &control)?;
    let registration_id = if request.plan.attempts_per_task == REGISTERED_PRACTICE_ATTEMPTS {
        Some(verify_registration(store, ctx, &request.plan).await?.id)
    } else {
        None
    };
    {
        let mut session = store.session().await?;
        require_live_sources(
            &mut session,
            ctx,
            &control.source_ids,
            request.revoke_watermark,
        )
        .await?;
        session.commit().await?;
    }

    let k = u32::from(request.plan.attempts_per_task);
    let (attempts, stop) = run_attempts(ctx, store, runner, &control, &request, k).await?;
    let outcome = match stop {
        Some(reason) => PracticeOutcomeV1::Incomplete { reason },
        // A single attempt has nothing to contrast with; K is never raised.
        None if k == 1 => PracticeOutcomeV1::NoContrast,
        None => select_contrast(&attempts),
    };
    let families: BTreeSet<&str> = control
        .tasks
        .iter()
        .map(|task| task.parent_family.as_str())
        .collect();
    let set = PracticeAttemptSetV1 {
        schema_version: PRACTICE_ATTEMPT_SET_SCHEMA.into(),
        id: practice_attempt_set_storage_id(&request.set_id)?,
        set_id: request.set_id.clone(),
        namespace: ctx.namespace().into(),
        control_id: control.id.clone(),
        episode_id: request.episode_id.clone(),
        step: request.step,
        bundle_digest: request.bundle_digest.clone(),
        environment_digest: control.environment_digest.clone(),
        revoke_watermark: request.revoke_watermark,
        request_digest,
        plan: request.plan.clone(),
        registration_id,
        attempts,
        outcome,
        independent_clusters: u32::try_from(families.len()).map_err(|_| Error::Internal)?,
        counts_as_independent_samples: false,
        created_at: request.created_at,
    };
    set.validate()?;
    persist_set(ctx, store, &control, set).await
}

/// Writes the set once. The live-source gate is re-checked inside the writing
/// transaction, so a set is never stored after a revocation that could already
/// have passed its cleanup. A concurrent writer of the same request wins: the
/// stored set is returned; another request under the same id is a `Conflict`.
async fn persist_set(
    ctx: &Context,
    store: &Store,
    control: &DevelopmentControlV1,
    set: PracticeAttemptSetV1,
) -> Result<PracticeAttemptSetV1> {
    let storage = set.id.clone();
    let mut session = store.session().await?;
    require_live_sources(&mut session, ctx, &control.source_ids, set.revoke_watermark).await?;
    if let Some(value) = session
        .get::<serde_json::Value>(ctx, ARTIFACT_KIND, &storage)
        .await?
    {
        let existing = parse_set(ctx, &set.set_id, value)?;
        if existing.request_digest != set.request_digest {
            return Err(Error::Conflict(
                "practice set id is already recorded for a different request".into(),
            ));
        }
        session.commit().await?;
        return Ok(existing);
    }
    session
        .put(ctx, ARTIFACT_KIND, &storage, ctx.actor(), &set)
        .await?;
    for source in &control.source_ids {
        session
            .put_edge(ctx, ARTIFACT_KIND, &storage, "run", source)
            .await?;
    }
    session
        .put_edge(
            ctx,
            ARTIFACT_KIND,
            &storage,
            ARTIFACT_KIND,
            &storage_id(CONTROL_KIND, &control.id)?,
        )
        .await?;
    for attempt in &set.attempts {
        session
            .put_edge(
                ctx,
                ARTIFACT_KIND,
                &storage,
                ARTIFACT_KIND,
                &storage_id(RUN_RECEIPT_KIND, &attempt.execution_receipt_id)?,
            )
            .await?;
        for task in &attempt.tasks {
            let candidate = execution_receipt_id(
                &attempt.request_id,
                &task.task_id,
                DevelopmentSide::Candidate,
            )?;
            session
                .put_edge(
                    ctx,
                    ARTIFACT_KIND,
                    &storage,
                    ARTIFACT_KIND,
                    &storage_id(EXECUTION_RECEIPT_KIND, &candidate)?,
                )
                .await?;
        }
    }
    if let Some(registration) = &set.registration_id {
        session
            .put_edge(
                ctx,
                ARTIFACT_KIND,
                &storage,
                ARTIFACT_KIND,
                &practice_registration_storage_id(registration)?,
            )
            .await?;
    }
    session
        .audit(ctx, "practice.attempt_set.record", &set.set_id)
        .await?;
    session.commit().await?;
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(label: &str) -> String {
        evo_core::hash(label.as_bytes())
    }

    fn result(task: &str, family: &str, score: u32, passed: bool) -> PracticeTaskResultV1 {
        PracticeTaskResultV1 {
            task_id: task.into(),
            parent_family: family.into(),
            passed,
            score_micros: score,
            output_digest: d(&format!("{task}-{score}-{passed}")),
        }
    }

    fn attempt(number: u32, tasks: Vec<PracticeTaskResultV1>) -> PracticeAttemptV1 {
        let request_id = practice_attempt_request_id("set", number).unwrap();
        PracticeAttemptV1 {
            attempt: number,
            execution_receipt_id: run_receipt_id(&request_id).unwrap(),
            request_id,
            tasks,
            cost_state: known(0),
            cache_hit: false,
        }
    }

    fn known(micros: i64) -> DevelopmentCostState {
        DevelopmentCostState::Known {
            micros,
            currency: "USD".into(),
            pricing_version: "pricing-v1".into(),
        }
    }

    #[test]
    fn higher_score_is_better_and_ties_keep_the_earliest_attempt() {
        let attempts = vec![
            attempt(1, vec![result("a", "fa", 0, false)]),
            attempt(2, vec![result("a", "fa", 1_000_000, true)]),
            attempt(3, vec![result("a", "fa", 0, false)]),
        ];
        assert_eq!(
            select_contrast(&attempts),
            PracticeOutcomeV1::Contrast {
                better_attempt: 2,
                worse_attempt: 1,
                task_id: "a".into(),
            }
        );
        let attempts = vec![
            attempt(1, vec![result("a", "fa", 500_000, false)]),
            attempt(2, vec![result("a", "fa", 900_000, false)]),
            attempt(3, vec![result("a", "fa", 900_000, false)]),
        ];
        assert_eq!(
            select_contrast(&attempts),
            PracticeOutcomeV1::Contrast {
                better_attempt: 2,
                worse_attempt: 1,
                task_id: "a".into(),
            }
        );
    }

    #[test]
    fn equal_scores_compare_passed_and_equal_both_is_no_contrast() {
        let attempts = vec![
            attempt(1, vec![result("a", "fa", 700_000, false)]),
            attempt(2, vec![result("a", "fa", 700_000, true)]),
            attempt(3, vec![result("a", "fa", 700_000, false)]),
        ];
        assert_eq!(
            select_contrast(&attempts),
            PracticeOutcomeV1::Contrast {
                better_attempt: 2,
                worse_attempt: 1,
                task_id: "a".into(),
            }
        );
        let same = vec![
            attempt(1, vec![result("a", "fa", 700_000, true)]),
            attempt(2, vec![result("a", "fa", 700_000, true)]),
            attempt(3, vec![result("a", "fa", 700_000, true)]),
        ];
        assert_eq!(select_contrast(&same), PracticeOutcomeV1::NoContrast);
        assert_eq!(select_contrast(&same[..1]), PracticeOutcomeV1::NoContrast);
        assert_eq!(select_contrast(&[]), PracticeOutcomeV1::NoContrast);
    }

    #[test]
    fn the_first_differing_task_in_control_order_is_named() {
        let attempts = vec![
            attempt(
                1,
                vec![
                    result("a", "fa", 1_000_000, true),
                    result("b", "fb", 0, false),
                ],
            ),
            attempt(
                2,
                vec![
                    result("a", "fa", 1_000_000, true),
                    result("b", "fb", 1_000_000, true),
                ],
            ),
            attempt(
                3,
                vec![
                    result("a", "fa", 1_000_000, true),
                    result("b", "fb", 0, false),
                ],
            ),
        ];
        assert_eq!(
            select_contrast(&attempts),
            PracticeOutcomeV1::Contrast {
                better_attempt: 2,
                worse_attempt: 1,
                task_id: "b".into(),
            }
        );
    }

    #[test]
    fn attempt_cost_is_the_sum_of_known_costs_and_uncertain_is_contagious() {
        assert_eq!(aggregate_cost(&[known(0), known(0)]), known(0));
        assert_eq!(aggregate_cost(&[known(3), known(4)]), known(7));
        assert_eq!(
            aggregate_cost(&[known(3), DevelopmentCostState::Uncertain, known(4)]),
            DevelopmentCostState::Uncertain
        );
        assert_eq!(aggregate_cost(&[]), DevelopmentCostState::Uncertain);
        assert_eq!(
            aggregate_cost(&[known(i64::MAX), known(1)]),
            DevelopmentCostState::Uncertain
        );
        let other_pricing = DevelopmentCostState::Known {
            micros: 1,
            currency: "USD".into(),
            pricing_version: "pricing-v2".into(),
        };
        assert_eq!(
            aggregate_cost(&[known(1), other_pricing]),
            DevelopmentCostState::Uncertain
        );
    }

    #[test]
    fn runner_errors_stop_a_set_with_a_stable_reason() {
        assert_eq!(runner_stop_reason(2, &Error::Budget), "budget_exhausted:2");
        assert_eq!(
            runner_stop_reason(2, &Error::Cancelled),
            "uncertain_attempt:2"
        );
        assert_eq!(
            runner_stop_reason(1, &Error::Conflict("x".into())),
            "attempt_error:1:conflict"
        );
        assert_eq!(
            runner_stop_reason(3, &Error::Internal),
            "attempt_error:3:internal"
        );
        for error in [
            Error::Invalid("x".into()),
            Error::Forbidden,
            Error::NotFound,
            Error::Conflict("x".into()),
            Error::Budget,
            Error::Cancelled,
            Error::Internal,
        ] {
            identifier(&runner_stop_reason(1, &error)).unwrap();
        }
    }

    fn two_task_attempt(number: u32, cost: DevelopmentCostState) -> PracticeAttemptV1 {
        let mut attempt = attempt(
            number,
            vec![
                result("a", "fa", 1_000_000, true),
                result("b", "fb", 1_000_000, true),
            ],
        );
        attempt.cost_state = cost;
        attempt
    }

    fn set_with(
        attempts: Vec<PracticeAttemptV1>,
        outcome: PracticeOutcomeV1,
    ) -> PracticeAttemptSetV1 {
        PracticeAttemptSetV1 {
            schema_version: PRACTICE_ATTEMPT_SET_SCHEMA.into(),
            id: practice_attempt_set_storage_id("set").unwrap(),
            set_id: "set".into(),
            namespace: "n".into(),
            control_id: "control".into(),
            episode_id: "episode".into(),
            step: 1,
            bundle_digest: d("bundle"),
            environment_digest: d("environment"),
            revoke_watermark: 1,
            request_digest: d("request"),
            plan: PracticePlan::new(
                "cluster",
                vec!["a".into(), "b".into()],
                3,
                Some(d("authorization")),
            )
            .unwrap(),
            registration_id: Some(d("authorization")),
            attempts,
            outcome,
            independent_clusters: 2,
            counts_as_independent_samples: false,
            created_at: 1,
        }
    }

    #[test]
    fn an_uncertain_attempt_ends_a_set_and_cannot_be_followed() {
        let certain = |number| two_task_attempt(number, known(0));
        let uncertain = |number| two_task_attempt(number, DevelopmentCostState::Uncertain);
        assert_eq!(uncertain_stop(&certain(1)), None);
        assert_eq!(
            uncertain_stop(&uncertain(2)),
            Some("uncertain_attempt:2".into())
        );
        let incomplete = |reason: &str| PracticeOutcomeV1::Incomplete {
            reason: reason.into(),
        };
        // Attempt 2 was uncertain: recorded, last, and the set is incomplete.
        set_with(
            vec![certain(1), uncertain(2)],
            incomplete("uncertain_attempt:2"),
        )
        .validate()
        .unwrap();
        // The last of K attempts may be the uncertain one.
        set_with(
            vec![certain(1), certain(2), uncertain(3)],
            incomplete("uncertain_attempt:3"),
        )
        .validate()
        .unwrap();
        // It cannot be followed by another attempt, nor hidden by a finished
        // outcome, and all-certain attempts cannot be called incomplete.
        assert!(
            set_with(
                vec![certain(1), uncertain(2), certain(3)],
                incomplete("uncertain_attempt:2")
            )
            .validate()
            .is_err()
        );
        assert!(
            set_with(
                vec![certain(1), certain(2), uncertain(3)],
                PracticeOutcomeV1::NoContrast
            )
            .validate()
            .is_err()
        );
        assert!(
            set_with(
                vec![certain(1), certain(2), certain(3)],
                incomplete("uncertain_attempt:3")
            )
            .validate()
            .is_err()
        );
        set_with(
            vec![certain(1), certain(2), certain(3)],
            PracticeOutcomeV1::NoContrast,
        )
        .validate()
        .unwrap();
    }

    #[test]
    fn attempt_request_ids_are_derived_from_the_set_and_the_attempt_only() {
        let one = practice_attempt_request_id("set", 1).unwrap();
        assert_eq!(one, practice_attempt_request_id("set", 1).unwrap());
        assert_ne!(one, practice_attempt_request_id("set", 2).unwrap());
        assert_ne!(one, practice_attempt_request_id("other-set", 1).unwrap());
        assert!(practice_attempt_request_id("set", 0).is_err());
        assert!(practice_attempt_request_id("set", 4).is_err());
        identifier(&one).unwrap();
    }
}
