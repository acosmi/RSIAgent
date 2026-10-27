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
    StepErrorKind, StepTerminalClass, run_optimization_step,
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
    /// How the group's step ended, as the closed class of the optimization step: a
    /// no change, an incumbent kept, a rejection or an uncertain dispatch, and, for a
    /// group that has no durable outcome of its own (`Failed`), the kind of the error
    /// that ended it. `None` for a candidate. In memory only: the job persists nothing,
    /// and no text of an error, a model or a source is part of it.
    pub terminal: Option<StepTerminalClass>,
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
/// (`NotFound`). A clock refusal also stops a started job before any next group.
/// `Ok` means every group was run or replayed from its journal
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
    // Validate every group before the first group's model, runner or claim.
    for group in &groups {
        journal
            .current_compiler_inputs(&group.request.compiler_input_request())
            .await?;
    }

    let mut outcomes = Vec::with_capacity(groups.len());
    for (group, scope) in groups.into_iter().zip(scopes) {
        let result = run_optimization_step(model, runner, Some(journal), group.request).await;
        if matches!(&result, Err(Error::ClockRegressed)) {
            return Err(Error::ClockRegressed);
        }
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
    let outcome = |kind, terminal: Option<StepTerminalClass>| GroupOutcome {
        group_id: scope.group_id.clone(),
        skill_id: scope.skill_id.clone(),
        episode_id: scope.episode_id.clone(),
        kind,
        candidate_skill_digest: None,
        candidate_bundle_digest: None,
        terminal,
    };
    let failed = |error: &Error| {
        outcome(
            GroupOutcomeKind::Failed,
            Some(StepTerminalClass::StepError {
                error: StepErrorKind::from(error),
            }),
        )
    };
    match result {
        Ok(OptimizationStepOutcome::Candidate { edit, bundle, .. }) => {
            match skill_snapshot_digest(&edit.output) {
                Ok(candidate_skill_digest) => GroupOutcome {
                    candidate_skill_digest: Some(candidate_skill_digest),
                    candidate_bundle_digest: Some(bundle.digest),
                    ..outcome(GroupOutcomeKind::Candidate, None)
                },
                Err(error) => failed(&error),
            }
        }
        Ok(OptimizationStepOutcome::CurrentCandidate { edit, bundle, .. }) => {
            match skill_snapshot_digest(&edit.output) {
                Ok(candidate_skill_digest) => GroupOutcome {
                    candidate_skill_digest: Some(candidate_skill_digest),
                    candidate_bundle_digest: Some(bundle.digest),
                    ..outcome(GroupOutcomeKind::Candidate, None)
                },
                Err(error) => failed(&error),
            }
        }
        Ok(OptimizationStepOutcome::NoChange { class }) => {
            outcome(GroupOutcomeKind::NoChange, Some(class))
        }
        Ok(OptimizationStepOutcome::Rejected { class }) => {
            outcome(GroupOutcomeKind::Rejected, Some(class))
        }
        Ok(OptimizationStepOutcome::Uncertain { class }) => {
            outcome(GroupOutcomeKind::Uncertain, Some(class))
        }
        Err(error) => failed(&error),
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

#[cfg(test)]
mod current_candidate_tests {
    use super::*;
    use crate::optimization::{DevelopmentSelection, DevelopmentSelectionDecision};
    use evo_core::skill_edit::{
        CompiledSkillEdit, EditApplyReport, EditApplyStatus, EvidenceClosure,
    };

    // Complete compiled content for projection only: no registered producer,
    // Host, paid execution or approved-source authority is constructed here.
    fn current_bundle() -> evo_core::contract::ResolvedBundleV3 {
        use evo_core::contract::{
            CompilePartsV3, EmptyRootContentV1, FrozenCompilerProfileContentV1, HostCapabilities,
            ImproverPatch, ParentContentV3, PatchOp, Profile, ProfileContentProofV3, SkillPatch,
            SkillSnapshot, SystemSnapshot, compile_bundle_v3, replay_resolved_bundle_v3,
        };
        use evo_core::{Strategy, fingerprint, hash};
        use std::collections::BTreeSet;
        let snapshot = SystemSnapshot {
            schema_version: "rsia.system_snapshot.v1".into(),
            profile_id: "projection-profile".into(),
            host_id: "reference".into(),
            host_version: "0.1.0".into(),
            model_id: "development-model".into(),
            tools: vec![],
            mandatory_context_digest: hash(b"Complete projection context original.\n"),
        };
        let baseline = FrozenCompilerProfileContentV1 {
            id: "projection-profile-registration".into(),
            schema_version: "rsia.frozen_compiler_profile.v1".into(),
            namespace: "projection-data".into(),
            profile_id: snapshot.profile_id.clone(),
            version: 1,
            host_registration_id: "projection-host-registration".into(),
            provider_registration_id: "projection-provider-registration".into(),
            resource_instrumentation_id: None,
            environment_digest: snapshot.digest().unwrap(),
            system_snapshot: snapshot,
            capabilities: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            baseline_skill: SkillSnapshot::empty(),
            baseline_strategy: Strategy::default(),
            dependencies: vec![],
            purpose: evo_core::evidence::Purpose::Development,
            application_order: vec!["mandatory_context".into(), "skill".into(), "task".into()],
            authorized_by: "projection-data-operator".into(),
        };
        let root = EmptyRootContentV1 {
            id: "projection-empty-root".into(),
            schema_version: "rsia.empty_root_authorization.v1".into(),
            namespace: baseline.namespace.clone(),
            profile_registration_id: baseline.id.clone(),
            profile_digest: fingerprint(&baseline).unwrap(),
            host_registration_id: baseline.host_registration_id.clone(),
            skill: SkillSnapshot::empty(),
            strategy: Strategy {
                schema_version: "evo.strategy.v1".into(),
                instruction:
                    "Preserve the unchanged host baseline; no evolution instruction is applied."
                        .into(),
                max_candidates: 1,
                max_rounds: 1,
                require_counterexample: true,
            },
            authorized_by: "projection-data-operator".into(),
        };
        let proof = ProfileContentProofV3::from_content(
            Profile {
                id: baseline.profile_id.clone(),
                evolution_enabled: true,
                parent_digest: fingerprint(&root).unwrap(),
                baseline_digest: fingerprint(&baseline).unwrap(),
            },
            ParentContentV3::EmptyRoot(root),
            baseline,
        )
        .unwrap();
        let patch = SkillPatch {
            content: PatchOp::Set {
                value: "完整 Current 候选内容；保留 typed V3 全像。".into(),
            },
            ..SkillPatch::default()
        };
        let bundle = compile_bundle_v3(CompilePartsV3 {
            content: &proof,
            skill_patch: &patch,
            improver_patch: &ImproverPatch::default(),
            revoked: &BTreeSet::new(),
        })
        .unwrap();
        replay_resolved_bundle_v3(&bundle, &proof, &BTreeSet::new()).unwrap();
        bundle
    }

    fn scope() -> GroupScope {
        GroupScope {
            group_id: "group".into(),
            skill_id: "skill".into(),
            skill_version: "v1".into(),
            namespace: "projection-data".into(),
            parent_digest: "parent".into(),
            approved_parent_digest: "approved-parent".into(),
            episode_id: "episode".into(),
            request_id: "request".into(),
        }
    }

    #[test]
    fn current_candidate_projects_its_actual_skill_and_complete_bundle_digest() {
        let bundle = current_bundle();
        let expected_bundle = bundle.digest.clone();
        let expected_skill = skill_snapshot_digest(&bundle.skill).unwrap();
        let edit = CompiledSkillEdit {
            output: bundle.skill.clone(),
            patch: evo_core::contract::SkillPatch::default(),
            report: EditApplyReport {
                schema_version: evo_core::skill_edit::SKILL_EDIT_SCHEMA.into(),
                compiler_version: evo_core::skill_edit::SKILL_EDIT_COMPILER_VERSION.into(),
                namespace: "projection-data".into(),
                profile_id: "projection-profile".into(),
                skill_id: "skill".into(),
                skill_version: "v1".into(),
                status: EditApplyStatus::Applied,
                input_digest: skill_snapshot_digest(&evo_core::contract::SkillSnapshot::empty())
                    .unwrap(),
                output_digest: expected_skill.clone(),
                approved_parent_digest: bundle.parent_digest.clone(),
                safe_baseline_digest: bundle.baseline_digest.clone(),
                evidence: EvidenceClosure {
                    support: vec![],
                    counterexamples: vec![],
                    dependencies: vec![],
                },
                changed_bytes: 0,
                edits: vec![],
            },
        };
        let selection = DevelopmentSelection {
            request_id: "projection-development".into(),
            manifest_digest: evo_core::hash(b"Complete projection manifest original."),
            parent_bundle_digest: bundle.parent_digest.clone(),
            candidate_bundle_digest: expected_bundle.clone(),
            decision: DevelopmentSelectionDecision::AcceptCandidate,
            parent_total_micros: 1,
            candidate_total_micros: 2,
            reason: "data projection only".into(),
        };
        let outcome = group_outcome(
            scope(),
            Ok(OptimizationStepOutcome::CurrentCandidate {
                edit: Box::new(edit.clone()),
                bundle: Box::new(bundle.clone()),
                selection: selection.clone(),
            }),
        );
        assert_eq!(outcome.kind, GroupOutcomeKind::Candidate);
        assert_eq!(
            outcome.candidate_skill_digest.as_deref(),
            Some(expected_skill.as_str())
        );
        assert_eq!(
            outcome.candidate_bundle_digest.as_deref(),
            Some(expected_bundle.as_str())
        );
        assert!(outcome.terminal.is_none());

        let mut invalid = edit;
        invalid.output.content = "x".repeat(16 * 1024 + 1);
        let refused = group_outcome(
            scope(),
            Ok(OptimizationStepOutcome::CurrentCandidate {
                edit: Box::new(invalid),
                bundle: Box::new(bundle),
                selection,
            }),
        );
        assert_eq!(refused.kind, GroupOutcomeKind::Failed);
        assert!(refused.candidate_skill_digest.is_none());
        assert!(refused.candidate_bundle_digest.is_none());
        assert!(matches!(
            refused.terminal,
            Some(StepTerminalClass::StepError { .. })
        ));
    }

    #[test]
    fn current_projection_keeps_non_candidate_terminal_classes() {
        for (step, kind, class) in [
            (
                OptimizationStepOutcome::NoChange {
                    class: StepTerminalClass::NoEligibleReflectionBatch,
                },
                GroupOutcomeKind::NoChange,
                StepTerminalClass::NoEligibleReflectionBatch,
            ),
            (
                OptimizationStepOutcome::Rejected {
                    class: StepTerminalClass::GrantUnavailable,
                },
                GroupOutcomeKind::Rejected,
                StepTerminalClass::GrantUnavailable,
            ),
            (
                OptimizationStepOutcome::Uncertain {
                    class: StepTerminalClass::CancelledOrUncertain,
                },
                GroupOutcomeKind::Uncertain,
                StepTerminalClass::CancelledOrUncertain,
            ),
        ] {
            let actual = group_outcome(scope(), Ok(step));
            assert_eq!(actual.kind, kind);
            assert_eq!(actual.terminal, Some(class));
            assert!(actual.candidate_skill_digest.is_none());
            assert!(actual.candidate_bundle_digest.is_none());
        }
    }
}
