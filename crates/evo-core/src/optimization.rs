//! Pure G1 skill-optimization contracts. These records never approve or activate a skill.

use crate::contract::{AppliedReceipt, CapabilityLevel};
use crate::evidence::{EvidenceSet, ExecutionAttestation, Purpose, TaskOrigin};
use crate::skill_edit::{EvidenceClosure, EvidenceRef, SkillTextEdit};
use crate::{Error, Result, fingerprint, identifier, text};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_REFLECTION_BATCHES: usize = 2;
pub const MAX_TRACES_PER_BATCH: usize = 8;
pub const MAX_SUGGESTIONS: usize = 8;
pub const MAX_FINAL_EDITS: usize = 4;

fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(format!("{name} must be a lowercase sha256")));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillFailureKind {
    NotSelected,
    NotAttached,
    ContextMismatch,
    ExecutionLapse,
    SkillDefect,
    EnvironmentOrCapability,
    Uncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionEvidence {
    Selected,
    NotSelected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleHypothesisKind {
    Missing,
    Incorrect,
    Ambiguous,
    NotFollowed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleHypothesis {
    pub kind: RuleHypothesisKind,
    pub rule_id: String,
    pub behavior_evidence_digest: String,
    pub support: Vec<EvidenceRef>,
    pub counterexamples: Vec<EvidenceRef>,
}

#[derive(Debug, Clone)]
pub struct ApplicationObservation<'a> {
    pub skill_id: &'a str,
    pub expected_bundle_digest: &'a str,
    pub expected_request_digest: &'a str,
    pub selection: SelectionEvidence,
    pub receipt: Option<&'a AppliedReceipt>,
    pub request_attestation: Option<ExecutionAttestation>,
    pub context_matches: bool,
    pub environment_or_capability_failure: bool,
    pub hypothesis: Option<&'a RuleHypothesis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillFailureDiagnosis {
    pub kind: SkillFailureKind,
    pub skill_id: String,
    pub bundle_digest: String,
    pub request_digest: String,
    pub rule_id: Option<String>,
    pub support: Vec<EvidenceRef>,
    pub counterexamples: Vec<EvidenceRef>,
    pub reason: String,
}

pub fn diagnose_application(
    observation: ApplicationObservation<'_>,
) -> Result<SkillFailureDiagnosis> {
    identifier(observation.skill_id)?;
    validate_digest(observation.expected_bundle_digest, "bundle digest")?;
    validate_digest(observation.expected_request_digest, "request digest")?;
    if observation.selection == SelectionEvidence::NotSelected {
        return diagnosis(
            observation,
            SkillFailureKind::NotSelected,
            "skill was not selected",
        );
    }
    let Some(receipt) = observation.receipt else {
        return diagnosis(
            observation,
            SkillFailureKind::Uncertain,
            "trusted application receipt missing",
        );
    };
    receipt.validate()?;
    if receipt.bundle_digest != observation.expected_bundle_digest
        || receipt.request_digest != observation.expected_request_digest
        || !observation.context_matches
        || receipt.truncated
    {
        return diagnosis(
            observation,
            SkillFailureKind::ContextMismatch,
            "request or context does not match",
        );
    }
    if receipt.capability_level == CapabilityLevel::ToolOnly
        || !receipt.attached.iter().any(|id| id == observation.skill_id)
    {
        return diagnosis(
            observation,
            SkillFailureKind::NotAttached,
            "selected skill was not attached",
        );
    }
    if observation.environment_or_capability_failure {
        return diagnosis(
            observation,
            SkillFailureKind::EnvironmentOrCapability,
            "environment or capability failure",
        );
    }
    let Some(hypothesis) = observation.hypothesis else {
        return diagnosis(
            observation,
            SkillFailureKind::Uncertain,
            "no localized rule hypothesis",
        );
    };
    validate_hypothesis(hypothesis)?;
    match hypothesis.kind {
        RuleHypothesisKind::NotFollowed
            if observation.request_attestation == Some(ExecutionAttestation::TrustedHost) =>
        {
            diagnosis(
                observation,
                SkillFailureKind::ExecutionLapse,
                "localized rule was not followed",
            )
        }
        RuleHypothesisKind::NotFollowed => diagnosis(
            observation,
            SkillFailureKind::Uncertain,
            "execution lapse lacks a trusted request attestation",
        ),
        RuleHypothesisKind::Missing
        | RuleHypothesisKind::Incorrect
        | RuleHypothesisKind::Ambiguous => diagnosis(
            observation,
            SkillFailureKind::SkillDefect,
            "localized skill defect hypothesis",
        ),
    }
}

fn diagnosis(
    observation: ApplicationObservation<'_>,
    kind: SkillFailureKind,
    reason: &str,
) -> Result<SkillFailureDiagnosis> {
    let (rule_id, support, counterexamples) = observation
        .hypothesis
        .map(|hypothesis| {
            (
                Some(hypothesis.rule_id.clone()),
                hypothesis.support.clone(),
                hypothesis.counterexamples.clone(),
            )
        })
        .unwrap_or_default();
    Ok(SkillFailureDiagnosis {
        kind,
        skill_id: observation.skill_id.into(),
        bundle_digest: observation.expected_bundle_digest.into(),
        request_digest: observation.expected_request_digest.into(),
        rule_id,
        support,
        counterexamples,
        reason: reason.into(),
    })
}

fn validate_hypothesis(hypothesis: &RuleHypothesis) -> Result<()> {
    identifier(&hypothesis.rule_id)?;
    validate_digest(
        &hypothesis.behavior_evidence_digest,
        "behavior evidence digest",
    )?;
    if hypothesis.support.is_empty() {
        return Err(Error::Invalid(
            "rule hypothesis lacks supporting evidence".into(),
        ));
    }
    validate_evidence_refs(hypothesis.support.iter().chain(&hypothesis.counterexamples))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceOutcome {
    Success,
    TaskFailure,
    EnvironmentFailure,
    Invalid,
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizationTrace {
    pub run_id: String,
    pub parent_family: String,
    pub source_digest: String,
    pub purpose: Purpose,
    pub outcome: TraceOutcome,
    pub diagnosis: Option<SkillFailureDiagnosis>,
    pub excerpt: String,
    pub seed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionBatchKind {
    Failure,
    Success,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionBatch {
    pub id: String,
    pub kind: ReflectionBatchKind,
    pub traces: Vec<OptimizationTrace>,
    pub independent_parent_families: BTreeSet<String>,
    pub source_closure: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionBuild {
    pub batches: Vec<ReflectionBatch>,
    pub excluded_run_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TrustedSourceBinding {
    pub source_id: String,
    pub source_digest: String,
    pub parent_family: String,
}

pub fn build_reflection_batches(
    evidence: &EvidenceSet,
    bindings: &[TrustedSourceBinding],
    traces: Vec<OptimizationTrace>,
) -> Result<ReflectionBuild> {
    let members: BTreeMap<&str, _> = evidence
        .members
        .iter()
        .map(|member| (member.source_id.as_str(), member))
        .collect();
    let binding_by_source: BTreeMap<&str, &TrustedSourceBinding> = bindings
        .iter()
        .map(|binding| (binding.source_id.as_str(), binding))
        .collect();
    if binding_by_source.len() != evidence.members.len() {
        return Err(Error::Invalid(
            "trusted source binding set is incomplete".into(),
        ));
    }
    let mut failures = Vec::new();
    let mut successes = Vec::new();
    let mut excluded = Vec::new();
    let mut seen_runs = BTreeSet::new();
    for trace in traces {
        identifier(&trace.run_id)?;
        if !seen_runs.insert(trace.run_id.clone()) {
            return Err(Error::Conflict("duplicate optimization trace run".into()));
        }
        identifier(&trace.parent_family)?;
        validate_digest(&trace.source_digest, "trace source digest")?;
        text(&trace.excerpt, "trace excerpt", 16 * 1024)?;
        if trace.purpose != Purpose::Development {
            return Err(Error::Forbidden);
        }
        let member = members.get(trace.run_id.as_str()).ok_or(Error::Forbidden)?;
        let binding = binding_by_source
            .get(trace.run_id.as_str())
            .ok_or(Error::Forbidden)?;
        if member.content_digest != trace.source_digest
            || member.task_origin != TaskOrigin::TrustedRun
            || member.execution_attestation != ExecutionAttestation::TrustedHost
            || member.purpose != trace.purpose
            || binding.source_id != trace.run_id
            || binding.source_digest != trace.source_digest
            || binding.parent_family != trace.parent_family
            || !evidence
                .independent_clusters
                .contains(&binding.parent_family)
        {
            return Err(Error::Forbidden);
        }
        match trace.outcome {
            TraceOutcome::Success => successes.push(trace),
            TraceOutcome::TaskFailure
                if matches!(
                    trace.diagnosis.as_ref().map(|value| value.kind),
                    Some(SkillFailureKind::ExecutionLapse | SkillFailureKind::SkillDefect)
                ) =>
            {
                failures.push(trace)
            }
            _ => excluded.push(trace.run_id),
        }
    }
    if failures.len() > MAX_TRACES_PER_BATCH || successes.len() > MAX_TRACES_PER_BATCH {
        return Err(Error::Invalid(
            "reflection batch exceeds eight traces".into(),
        ));
    }
    failures.sort_by(|left, right| left.run_id.cmp(&right.run_id));
    successes.sort_by(|left, right| left.run_id.cmp(&right.run_id));
    let mut batches = Vec::new();
    if !failures.is_empty() {
        batches.push(reflection_batch(
            "reflection-failure",
            ReflectionBatchKind::Failure,
            failures,
        ));
    }
    if !successes.is_empty() {
        batches.push(reflection_batch(
            "reflection-success",
            ReflectionBatchKind::Success,
            successes,
        ));
    }
    if batches.len() > MAX_REFLECTION_BATCHES {
        return Err(Error::Internal);
    }
    Ok(ReflectionBuild {
        batches,
        excluded_run_ids: excluded,
    })
}

fn reflection_batch(
    id: &str,
    kind: ReflectionBatchKind,
    traces: Vec<OptimizationTrace>,
) -> ReflectionBatch {
    let independent_parent_families = traces
        .iter()
        .map(|trace| trace.parent_family.clone())
        .collect();
    let source_closure = traces
        .iter()
        .map(|trace| EvidenceRef {
            id: trace.run_id.clone(),
            digest: trace.source_digest.clone(),
        })
        .collect();
    ReflectionBatch {
        id: id.into(),
        kind,
        traces,
        independent_parent_families,
        source_closure,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditSuggestion {
    pub id: String,
    pub hypothesis: String,
    pub batch_ids: Vec<String>,
    pub support: Vec<EvidenceRef>,
    pub counterexamples: Vec<EvidenceRef>,
    pub dependencies: Vec<EvidenceRef>,
    pub edit: SkillTextEdit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeRecord {
    pub input_suggestion_ids: Vec<String>,
    pub selected_suggestion_ids: Vec<String>,
    pub read_dependencies: Vec<EvidenceRef>,
    pub evidence: EvidenceClosure,
    pub edits: Vec<SkillTextEdit>,
}

pub fn merge_suggestions(
    batches: &[ReflectionBatch],
    suggestions: &[EditSuggestion],
    selected_ids: &[String],
) -> Result<MergeRecord> {
    validate_suggestion_pool(batches, suggestions)?;
    let by_id: BTreeMap<&str, &EditSuggestion> = suggestions
        .iter()
        .map(|suggestion| (suggestion.id.as_str(), suggestion))
        .collect();
    let mut support = BTreeSet::new();
    let mut counterexamples = BTreeSet::new();
    let mut dependencies = BTreeSet::new();
    for suggestion in suggestions {
        support.extend(suggestion.support.iter().cloned());
        counterexamples.extend(suggestion.counterexamples.iter().cloned());
        dependencies.extend(suggestion.dependencies.iter().cloned());
    }
    let selected: BTreeSet<&str> = selected_ids.iter().map(String::as_str).collect();
    if selected.len() != selected_ids.len() || !selected.iter().all(|id| by_id.contains_key(id)) {
        return Err(Error::Invalid("invalid selected suggestion ids".into()));
    }
    if selected_ids.is_empty() || selected_ids.len() > MAX_FINAL_EDITS {
        return Err(Error::Invalid(
            "final edit selection must contain one to four items".into(),
        ));
    }
    let edits = selected_ids
        .iter()
        .map(|id| by_id[id.as_str()].edit.clone())
        .collect();
    Ok(MergeRecord {
        input_suggestion_ids: suggestions.iter().map(|item| item.id.clone()).collect(),
        selected_suggestion_ids: selected_ids.to_vec(),
        read_dependencies: dependencies.iter().cloned().collect(),
        evidence: EvidenceClosure {
            support: support.into_iter().collect(),
            counterexamples: counterexamples.into_iter().collect(),
            dependencies: dependencies.into_iter().collect(),
        },
        edits,
    })
}

pub fn validate_suggestion_pool(
    batches: &[ReflectionBatch],
    suggestions: &[EditSuggestion],
) -> Result<()> {
    if suggestions.is_empty() || suggestions.len() > MAX_SUGGESTIONS {
        return Err(Error::Invalid(
            "suggestion pool must contain one to eight items".into(),
        ));
    }
    let batch_ids: BTreeSet<&str> = batches.iter().map(|batch| batch.id.as_str()).collect();
    let allowed: BTreeSet<EvidenceRef> = batches
        .iter()
        .flat_map(|batch| batch.source_closure.iter().cloned())
        .collect();
    let mut by_id = BTreeMap::new();
    for suggestion in suggestions {
        identifier(&suggestion.id)?;
        text(&suggestion.hypothesis, "edit hypothesis", 4096)?;
        if by_id.insert(suggestion.id.as_str(), suggestion).is_some()
            || suggestion.batch_ids.is_empty()
            || !suggestion
                .batch_ids
                .iter()
                .all(|id| batch_ids.contains(id.as_str()))
        {
            return Err(Error::Invalid("invalid suggestion provenance".into()));
        }
        validate_evidence_refs(
            suggestion
                .support
                .iter()
                .chain(&suggestion.counterexamples)
                .chain(&suggestion.dependencies),
        )?;
        if suggestion.support.is_empty()
            || !suggestion
                .support
                .iter()
                .chain(&suggestion.counterexamples)
                .chain(&suggestion.dependencies)
                .all(|reference| allowed.contains(reference))
        {
            return Err(Error::Forbidden);
        }
        let declared: BTreeSet<_> = suggestion.dependencies.iter().collect();
        if !suggestion
            .support
            .iter()
            .chain(&suggestion.counterexamples)
            .all(|reference| declared.contains(reference))
        {
            return Err(Error::Invalid(
                "suggestion omitted a read dependency".into(),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStage {
    ReflectFailure,
    ReflectSuccess,
    Merge,
    Rank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelInputRole {
    System,
    User,
    Evidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInputPart {
    pub role: ModelInputRole,
    pub label: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequestContext {
    pub request_id: String,
    pub namespace: String,
    pub purpose: Purpose,
    pub stage: ModelStage,
    pub episode_id: String,
    pub step: u32,
    pub attempt: u32,
    pub parent_skill_digest: String,
    pub bundle_digest: String,
    pub source_closure: Vec<EvidenceRef>,
    pub model_digest: String,
    pub tools_digest: String,
    pub rules_digest: String,
    pub sampling_digest: String,
    pub revoke_watermark: u64,
    pub max_suggestions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRequest {
    pub request_id: String,
    pub namespace: String,
    pub purpose: Purpose,
    pub stage: ModelStage,
    pub episode_id: String,
    pub step: u32,
    pub attempt: u32,
    pub parent_skill_digest: String,
    pub bundle_digest: String,
    pub source_closure: Vec<EvidenceRef>,
    pub model_digest: String,
    pub tools_digest: String,
    pub rules_digest: String,
    pub sampling_digest: String,
    pub revoke_watermark: u64,
    pub max_suggestions: usize,
    pub input: Vec<ModelInputPart>,
    pub input_digest: String,
    pub cache_key_digest: String,
}

impl ModelRequest {
    pub fn build(context: ModelRequestContext, input: Vec<ModelInputPart>) -> Result<Self> {
        if context.purpose != Purpose::Development || input.is_empty() {
            return Err(Error::Forbidden);
        }
        for value in [
            &context.request_id,
            &context.namespace,
            &context.episode_id,
            &context.parent_skill_digest,
            &context.bundle_digest,
            &context.model_digest,
            &context.tools_digest,
            &context.rules_digest,
            &context.sampling_digest,
        ] {
            if value == &context.request_id
                || value == &context.namespace
                || value == &context.episode_id
            {
                identifier(value)?;
            } else {
                validate_digest(value, "model request digest")?;
            }
        }
        if context.source_closure.is_empty()
            || context.max_suggestions == 0
            || context.max_suggestions > MAX_SUGGESTIONS
        {
            return Err(Error::Invalid("invalid model request bounds".into()));
        }
        validate_evidence_refs(&context.source_closure)?;
        for part in &input {
            identifier(&part.label)?;
            text(&part.content, "model input", 128 * 1024)?;
        }
        let input_digest = fingerprint(&(input.as_slice(), context.source_closure.as_slice()))?;
        let cache_key_digest = fingerprint(&(
            &context.namespace,
            context.purpose,
            context.stage,
            &context.episode_id,
            context.step,
            context.attempt,
            &context.parent_skill_digest,
            &context.bundle_digest,
            &context.source_closure,
            &context.model_digest,
            &context.tools_digest,
            &context.rules_digest,
            &context.sampling_digest,
            context.revoke_watermark,
            &input_digest,
        ))?;
        Ok(Self {
            request_id: context.request_id,
            namespace: context.namespace,
            purpose: context.purpose,
            stage: context.stage,
            episode_id: context.episode_id,
            step: context.step,
            attempt: context.attempt,
            parent_skill_digest: context.parent_skill_digest,
            bundle_digest: context.bundle_digest,
            source_closure: context.source_closure,
            model_digest: context.model_digest,
            tools_digest: context.tools_digest,
            rules_digest: context.rules_digest,
            sampling_digest: context.sampling_digest,
            revoke_watermark: context.revoke_watermark,
            max_suggestions: context.max_suggestions,
            input,
            input_digest,
            cache_key_digest,
        })
    }

    pub fn validate(&self) -> Result<()> {
        let rebuilt = Self::build(
            ModelRequestContext {
                request_id: self.request_id.clone(),
                namespace: self.namespace.clone(),
                purpose: self.purpose,
                stage: self.stage,
                episode_id: self.episode_id.clone(),
                step: self.step,
                attempt: self.attempt,
                parent_skill_digest: self.parent_skill_digest.clone(),
                bundle_digest: self.bundle_digest.clone(),
                source_closure: self.source_closure.clone(),
                model_digest: self.model_digest.clone(),
                tools_digest: self.tools_digest.clone(),
                rules_digest: self.rules_digest.clone(),
                sampling_digest: self.sampling_digest.clone(),
                revoke_watermark: self.revoke_watermark,
                max_suggestions: self.max_suggestions,
            },
            self.input.clone(),
        )?;
        if rebuilt.input_digest != self.input_digest
            || rebuilt.cache_key_digest != self.cache_key_digest
        {
            return Err(Error::Conflict("model request digest mismatch".into()));
        }
        Ok(())
    }
}

fn validate_evidence_refs<'a>(refs: impl IntoIterator<Item = &'a EvidenceRef>) -> Result<()> {
    let mut identities = BTreeMap::new();
    for reference in refs {
        identifier(&reference.id)?;
        validate_digest(&reference.digest, "evidence digest")?;
        if let Some(previous) = identities.insert(reference.id.as_str(), reference.digest.as_str())
            && previous != reference.digest
        {
            return Err(Error::Conflict(
                "evidence id has conflicting digests".into(),
            ));
        }
    }
    Ok(())
}
