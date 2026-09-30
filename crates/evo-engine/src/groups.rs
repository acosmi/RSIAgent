//! E09 skill-group job orchestration (plan v4.2 section 7.7, V095.a program subrange).
//!
//! One job runs one or two isolated skill groups through the existing
//! single-skill optimization step (`run_optimization_step`). Each group keeps
//! its own sources, parent version, episode and stage records; the groups share
//! the model port, the development runner and therefore one root budget. The
//! billing scope and root budget belong to those ports (for example the
//! persistent broker's configuration), not to the step requests, so a job can
//! only ever draw on the one budget of the ports it is given and there is no
//! per-group budget to compare.
//!
//! * Every check that does not need a model or a runner runs before the first
//!   group starts, so an invalid job fails as a whole with zero calls.
//! * Groups run serially in `group_id` order. A group that ends as rejected,
//!   uncertain or failed is reported as such and never stops, rolls back or
//!   cleans up the other group: each group's durable facts are the ordinary
//!   optimization stage facts of its own episode.
//! * When at least one group produced a candidate, the outcome carries a
//!   [`CombinedSkillCandidate`]. That value only *marks* the combination as
//!   still requiring a complete `ResolvedBundle`, a full-combination
//!   development selection and independent formal acceptance. Nothing in this
//!   module approves, publishes or activates anything, and it persists no
//!   object of its own; repeating a job converges through the per-group
//!   journals without new dispatches.
//!
//! Not done here, by design of this increment: compiling the combined
//! `ResolvedBundle`, the development selection of the combination, formal
//! acceptance, and any management entry point.

use crate::model::ModelPort;
use crate::optimization::{
    DevRunner, OptimizationJournal, OptimizationStepOutcome, OptimizationStepRequest,
    run_optimization_step,
};
use evo_core::skill_edit::{skill_snapshot_digest, validate_batch_scope};
use evo_core::strategy::{
    CombinedSkillCandidate, SkillGroupCandidate, build_combined_skill_candidate,
};
use evo_core::{Error, Result, identifier};
use serde::Serialize;

/// The default bound of plan section 7.7, the same as
/// `evo_core::strategy::validate_skill_groups` enforces on the marker.
pub const MAX_SKILL_GROUPS_PER_JOB: usize = 2;

/// One group of a job: the group's name and its complete single-skill request.
/// The request's `edit_context`, `edit_batch_template` and `parent_skill` are
/// what confine the group to its own skill; its source selection, episode and
/// request id are what give it its own records.
pub struct SkillGroupSpec<'a> {
    pub group_id: String,
    pub request: OptimizationStepRequest<'a>,
}

pub struct SkillGroupJobRequest<'a> {
    /// Names the job in its outcome only. Nothing is persisted under it.
    pub job_id: String,
    pub groups: Vec<SkillGroupSpec<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupOutcomeKind {
    /// The group's development selection accepted its candidate.
    Candidate,
    NoChange,
    Rejected,
    /// A dispatch or its usage is unknown. Never retried automatically.
    Uncertain,
    /// `run_optimization_step` itself returned an error for this group, so it
    /// has no durable outcome of its own. Its facts, if any, are left exactly
    /// as they are; whether a dispatch happened is known only from them.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupOutcome {
    pub group_id: String,
    pub skill_id: String,
    pub episode_id: String,
    pub kind: GroupOutcomeKind,
    pub candidate_skill_digest: Option<String>,
    pub candidate_bundle_digest: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillGroupJobOutcome {
    pub job_id: String,
    /// In `group_id` order, whatever the order of the request.
    pub groups: Vec<GroupOutcome>,
    /// Present iff at least one group produced a candidate. It lists only those
    /// groups and still requires a complete combined development rerun and an
    /// independent formal evaluation.
    pub combined: Option<CombinedSkillCandidate>,
}

/// What the job needs to remember about one group whose request passed every
/// check that needs neither a model nor a runner.
struct GroupScope {
    group_id: String,
    skill_id: String,
    skill_version: String,
    namespace: String,
    parent_digest: String,
    approved_parent_digest: String,
    episode_id: String,
    request_id: String,
}

/// Runs the groups of one job and reports each group's own outcome.
///
/// `Err` means the job was refused before any group started: a malformed job
/// (`Invalid`), groups that are not separate enough (`Conflict`) or no journal
/// (`NotFound`). `Ok` means every group was run or replayed from its journal
/// and has an outcome, whatever that outcome is. The ports are only used by a
/// group that still has work to do: a group whose step is already recorded is
/// answered from the journal alone, so `None` ports replay a finished job.
pub async fn run_skill_group_job(
    model: Option<&dyn ModelPort>,
    runner: Option<&dyn DevRunner>,
    journal: Option<&dyn OptimizationJournal>,
    request: SkillGroupJobRequest<'_>,
) -> Result<SkillGroupJobOutcome> {
    let SkillGroupJobRequest { job_id, mut groups } = request;
    identifier(&job_id)?;
    if groups.is_empty() || groups.len() > MAX_SKILL_GROUPS_PER_JOB {
        return Err(Error::Invalid(
            "skill group job must contain one or two groups".into(),
        ));
    }
    for group in &groups {
        identifier(&group.group_id)?;
    }
    groups.sort_by(|left, right| left.group_id.cmp(&right.group_id));
    if groups
        .windows(2)
        .any(|pair| pair[0].group_id == pair[1].group_id)
    {
        return Err(Error::Conflict("duplicate skill group id".into()));
    }
    let scopes = groups.iter().map(group_scope).collect::<Result<Vec<_>>>()?;
    validate_group_separation(&scopes)?;
    let journal = journal.ok_or(Error::NotFound)?;

    let mut outcomes = Vec::with_capacity(groups.len());
    for (group, scope) in groups.into_iter().zip(scopes) {
        let result = run_optimization_step(model, runner, Some(journal), group.request).await;
        outcomes.push(group_outcome(scope, result));
    }
    let candidates: Vec<SkillGroupCandidate> = outcomes
        .iter()
        .filter(|outcome| outcome.kind == GroupOutcomeKind::Candidate)
        .filter_map(|outcome| {
            Some(SkillGroupCandidate {
                group_id: outcome.group_id.clone(),
                candidate_digest: outcome.candidate_bundle_digest.clone()?,
            })
        })
        .collect();
    // Group ids are validated identifiers and candidate digests are bundle
    // digests, so building the marker cannot fail here.
    let combined = if candidates.is_empty() {
        None
    } else {
        Some(build_combined_skill_candidate(candidates)?)
    };
    Ok(SkillGroupJobOutcome {
        job_id,
        groups: outcomes,
        combined,
    })
}

fn group_outcome(scope: GroupScope, result: Result<OptimizationStepOutcome>) -> GroupOutcome {
    let outcome = |kind, reason: Option<String>| GroupOutcome {
        group_id: scope.group_id.clone(),
        skill_id: scope.skill_id.clone(),
        episode_id: scope.episode_id.clone(),
        kind,
        candidate_skill_digest: None,
        candidate_bundle_digest: None,
        reason,
    };
    match result {
        Ok(OptimizationStepOutcome::Candidate { edit, bundle, .. }) => {
            match skill_snapshot_digest(&edit.output) {
                Ok(candidate_skill_digest) => GroupOutcome {
                    candidate_skill_digest: Some(candidate_skill_digest),
                    candidate_bundle_digest: Some(bundle.digest),
                    ..outcome(GroupOutcomeKind::Candidate, None)
                },
                Err(error) => outcome(GroupOutcomeKind::Failed, Some(error.to_string())),
            }
        }
        Ok(OptimizationStepOutcome::NoChange { reason }) => {
            outcome(GroupOutcomeKind::NoChange, Some(reason))
        }
        Ok(OptimizationStepOutcome::Rejected { reason }) => {
            outcome(GroupOutcomeKind::Rejected, Some(reason))
        }
        Ok(OptimizationStepOutcome::Uncertain { reason }) => {
            outcome(GroupOutcomeKind::Uncertain, Some(reason))
        }
        Err(error) => outcome(GroupOutcomeKind::Failed, Some(error.to_string())),
    }
}

/// Checks that belong to one group and need no model, runner or journal.
fn group_scope(group: &SkillGroupSpec<'_>) -> Result<GroupScope> {
    let request = &group.request;
    let context = &request.model_context;
    let development = &request.development_request;
    let template = &request.edit_batch_template;
    for value in [&context.namespace, &context.episode_id, &context.request_id] {
        identifier(value).map_err(|error| in_group(&group.group_id, error))?;
    }
    // The same predicate `run_optimization_step` applies before its first
    // dispatch, evaluated here so that one bad group cannot let the other
    // group spend before the job is refused.
    if context.namespace != development.namespace
        || context.episode_id != development.episode_id
        || context.step != development.step
        || context.attempt != development.attempt
        || context.revoke_watermark != development.revoke_watermark
    {
        return Err(Error::Conflict(format!(
            "skill group {}: step/development scope differs",
            group.group_id
        )));
    }
    verify_trusted_edit_scope(&group.group_id, request)?;
    if template.namespace != context.namespace {
        return Err(Error::Conflict(format!(
            "skill group {}: edit scope namespace differs from the model context",
            group.group_id
        )));
    }
    Ok(GroupScope {
        group_id: group.group_id.clone(),
        skill_id: template.skill_id.clone(),
        skill_version: template.skill_version.clone(),
        namespace: template.namespace.clone(),
        parent_digest: template.input_digest.clone(),
        approved_parent_digest: template.approved_parent_digest.clone(),
        episode_id: context.episode_id.clone(),
        request_id: context.request_id.clone(),
    })
}

/// A group may only edit the skill its `TrustedEditContext` was issued for.
/// The context has no accessors, so the job cannot read its fields; it asks the
/// core's own scope check instead. That is the check `compile_skill_edit_batch`
/// applies to the real batch, only after the model calls: the template's schema
/// and compiler version, every scope field of the template against the context
/// (namespace, profile, skill id and version, input digest, approved parent
/// digest, safe baseline digest), and that the parent skill is the one the
/// context was issued for. Running it here refuses a doomed group before any
/// dispatch. Once it passes, the template's public scope fields are the trusted
/// scope, which is what the comparison across groups reads.
fn verify_trusted_edit_scope(group_id: &str, request: &OptimizationStepRequest<'_>) -> Result<()> {
    validate_batch_scope(
        request.parent_skill,
        request.edit_context,
        &request.edit_batch_template,
    )
    .map_err(|error| in_group(group_id, error))
}

/// Checks across the (at most two) groups of one job.
fn validate_group_separation(scopes: &[GroupScope]) -> Result<()> {
    for (index, left) in scopes.iter().enumerate() {
        for right in &scopes[index + 1..] {
            if left.namespace != right.namespace {
                return Err(Error::Conflict(
                    "skill groups must share one namespace".into(),
                ));
            }
            if left.skill_id == right.skill_id {
                let same_parent = left.skill_version == right.skill_version
                    && left.parent_digest == right.parent_digest
                    && left.approved_parent_digest == right.approved_parent_digest;
                return Err(Error::Conflict(if same_parent {
                    format!(
                        "skill groups {} and {} target the same skill {}",
                        left.group_id, right.group_id, left.skill_id
                    )
                } else {
                    format!(
                        "skill {} appears in groups {} and {} with different parent versions",
                        left.skill_id, left.group_id, right.group_id
                    )
                }));
            }
            if left.episode_id == right.episode_id {
                return Err(Error::Conflict(
                    "skill groups must use distinct episodes".into(),
                ));
            }
            // The persistent broker keys every model call of a billing scope by
            // its request id alone, so a request id shared by two groups would
            // make the second group's first call a reuse of the first group's
            // call id, reported as a spurious "uncertain".
            if left.request_id == right.request_id {
                return Err(Error::Conflict(
                    "skill groups must use distinct request ids".into(),
                ));
            }
        }
    }
    Ok(())
}

fn in_group(group_id: &str, error: Error) -> Error {
    match error {
        Error::Invalid(message) => Error::Invalid(format!("skill group {group_id}: {message}")),
        Error::Conflict(message) => Error::Conflict(format!("skill group {group_id}: {message}")),
        other => other,
    }
}
