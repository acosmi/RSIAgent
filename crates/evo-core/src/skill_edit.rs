//! Deterministic compilation of bounded text edits into the existing `SkillPatch`.
//!
//! `ProtectedTextRange` and `TrustedEditContext` are control-plane inputs. They are
//! deliberately not deserializable from a model proposal.

use crate::contract::{PatchOp, SkillPatch, SkillSnapshot};
use crate::{Error, Result, hash, identifier};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SKILL_EDIT_SCHEMA: &str = "rsia.skill_edit.v1";
pub const SKILL_EDIT_REPORT_SCHEMA: &str = "rsia.skill_edit.apply_report.v1";
pub const SKILL_EDIT_COMPILER_VERSION: &str = "rsia.skill_edit.compiler.v1";
pub const MAX_EDITS_PER_BATCH: usize = 4;
pub const MAX_CHANGED_BYTES: usize = 4096;

const SKILL_EDIT_INPUT_DIGEST_SCHEMA: &str = "rsia.skill_edit.input_digest.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillTextField {
    Content,
    Applicability,
    Counterexample,
}

impl SkillTextField {
    fn get(self, snapshot: &SkillSnapshot) -> &str {
        match self {
            Self::Content => &snapshot.content,
            Self::Applicability => &snapshot.applicability,
            Self::Counterexample => &snapshot.counterexample,
        }
    }

    fn get_mut(self, snapshot: &mut SkillSnapshot) -> &mut String {
        match self {
            Self::Content => &mut snapshot.content,
            Self::Applicability => &mut snapshot.applicability,
            Self::Counterexample => &mut snapshot.counterexample,
        }
    }

    fn set_patch(self, patch: &mut SkillPatch, value: String) {
        let op = PatchOp::Set { value };
        match self {
            Self::Content => patch.content = op,
            Self::Applicability => patch.applicability = op,
            Self::Counterexample => patch.counterexample = op,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub id: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// These three lists are sets on the wire. Accepted input order is normalized in reports.
pub struct EvidenceClosure {
    #[serde(default)]
    pub support: Vec<EvidenceRef>,
    #[serde(default)]
    pub counterexamples: Vec<EvidenceRef>,
    pub dependencies: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactAnchor {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum TextEditOperation {
    Insert { text: String },
    Replace { text: String },
    Delete,
}

impl TextEditOperation {
    fn inserted_text(&self) -> &str {
        match self {
            Self::Insert { text } | Self::Replace { text } => text,
            Self::Delete => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillTextEdit {
    pub field: SkillTextField,
    pub start: usize,
    pub end: usize,
    pub expected_text_digest: String,
    #[serde(default)]
    pub exact_anchor: Option<ExactAnchor>,
    pub operation: TextEditOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillEditBatch {
    pub schema_version: String,
    pub compiler_version: String,
    pub namespace: String,
    pub profile_id: String,
    pub skill_id: String,
    pub skill_version: String,
    pub input_digest: String,
    pub approved_parent_digest: String,
    pub safe_baseline_digest: String,
    pub evidence: EvidenceClosure,
    pub edits: Vec<SkillTextEdit>,
}

/// Scope fixed by the trusted control plane before a model proposal is parsed.
#[derive(Debug, Clone)]
pub struct TrustedEditContext {
    namespace: String,
    profile_id: String,
    skill_id: String,
    skill_version: String,
    input_digest: String,
    approved_parent_digest: String,
    safe_baseline_digest: String,
    allowed_evidence: BTreeSet<EvidenceRef>,
}

impl TrustedEditContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        namespace: impl Into<String>,
        profile_id: impl Into<String>,
        skill_id: impl Into<String>,
        skill_version: impl Into<String>,
        approved_parent_digest: impl Into<String>,
        safe_baseline_digest: impl Into<String>,
        input: &SkillSnapshot,
        allowed_evidence: impl IntoIterator<Item = EvidenceRef>,
    ) -> Result<Self> {
        let context = Self {
            namespace: namespace.into(),
            profile_id: profile_id.into(),
            skill_id: skill_id.into(),
            skill_version: skill_version.into(),
            input_digest: skill_snapshot_digest(input)?,
            approved_parent_digest: approved_parent_digest.into(),
            safe_baseline_digest: safe_baseline_digest.into(),
            allowed_evidence: allowed_evidence.into_iter().collect(),
        };
        context.validate()?;
        Ok(context)
    }

    fn validate(&self) -> Result<()> {
        identifier(&self.namespace)?;
        identifier(&self.profile_id)?;
        identifier(&self.skill_id)?;
        identifier(&self.skill_version)?;
        validate_digest(&self.input_digest, "trusted input digest")?;
        validate_digest(
            &self.approved_parent_digest,
            "trusted approved parent digest",
        )?;
        validate_digest(&self.safe_baseline_digest, "trusted safe baseline digest")?;
        for evidence in &self.allowed_evidence {
            validate_evidence(evidence)?;
        }
        validate_source_identities(self.allowed_evidence.iter(), "trusted_allowed_evidence")?;
        Ok(())
    }
}

/// A range registered by trusted core code. It is not part of `SkillEditBatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedTextRange {
    field: SkillTextField,
    start: usize,
    end: usize,
    expected_text_digest: String,
}

impl ProtectedTextRange {
    pub fn new(
        field: SkillTextField,
        start: usize,
        end: usize,
        expected_text_digest: impl Into<String>,
    ) -> Self {
        Self {
            field,
            start,
            end,
            expected_text_digest: expected_text_digest.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditApplyStatus {
    Applied,
    NoChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedTextEdit {
    pub field: SkillTextField,
    pub start: usize,
    pub end: usize,
    pub removed_text: String,
    pub inserted_text: String,
    pub removed_digest: String,
    pub inserted_digest: String,
    pub removed_bytes: usize,
    pub inserted_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditApplyReport {
    pub schema_version: String,
    pub compiler_version: String,
    pub namespace: String,
    pub profile_id: String,
    pub skill_id: String,
    pub skill_version: String,
    pub status: EditApplyStatus,
    pub input_digest: String,
    pub output_digest: String,
    pub approved_parent_digest: String,
    pub safe_baseline_digest: String,
    pub evidence: EvidenceClosure,
    pub changed_bytes: usize,
    pub edits: Vec<AppliedTextEdit>,
}

#[derive(Debug, Clone)]
pub struct CompiledSkillEdit {
    pub output: SkillSnapshot,
    pub patch: SkillPatch,
    pub report: EditApplyReport,
}

#[derive(Serialize)]
struct SkillEditInputDigest<'a> {
    schema_version: &'static str,
    content_digest: String,
    applicability_digest: String,
    counterexample_digest: String,
    required_capabilities: &'a [String],
    dependencies: &'a [String],
}

/// Hashes the fixed, schema-tagged representation. Text bytes and ordered arrays
/// are preserved exactly; this does not claim full RFC 8785 canonicalization.
pub fn skill_snapshot_digest(snapshot: &SkillSnapshot) -> Result<String> {
    snapshot.validate()?;
    let value = SkillEditInputDigest {
        schema_version: SKILL_EDIT_INPUT_DIGEST_SCHEMA,
        content_digest: hash(snapshot.content.as_bytes()),
        applicability_digest: hash(snapshot.applicability.as_bytes()),
        counterexample_digest: hash(snapshot.counterexample.as_bytes()),
        required_capabilities: &snapshot.required_capabilities,
        dependencies: &snapshot.dependencies,
    };
    let encoded = serde_json::to_vec(&value).map_err(|_| Error::Internal)?;
    Ok(hash(&encoded))
}

/// Parses a proposal without accepting duplicate or unknown object fields.
pub fn parse_skill_edit_batch(input: &[u8]) -> Result<SkillEditBatch> {
    serde_json::from_slice(input)
        .map_err(|error| Error::Invalid(format!("invalid skill edit batch: {error}")))
}

pub fn compile_skill_edit_batch(
    input: &SkillSnapshot,
    context: &TrustedEditContext,
    batch: &SkillEditBatch,
    protected_ranges: &[ProtectedTextRange],
) -> Result<CompiledSkillEdit> {
    input.validate()?;
    context.validate()?;
    validate_batch_scope(input, context, batch)?;
    validate_evidence_closure(&batch.evidence, &context.allowed_evidence)?;

    if batch.edits.len() > MAX_EDITS_PER_BATCH {
        return Err(Error::Invalid(format!(
            "too_many_edits: maximum is {MAX_EDITS_PER_BATCH}"
        )));
    }

    for protected in protected_ranges {
        validate_protected_range(input, protected)?;
    }

    let mut reports = Vec::with_capacity(batch.edits.len());
    let mut changed_bytes = 0usize;
    for edit in &batch.edits {
        let report = validate_edit(input, edit, protected_ranges)?;
        changed_bytes = changed_bytes
            .checked_add(report.removed_bytes)
            .and_then(|value| value.checked_add(report.inserted_bytes))
            .ok_or_else(|| Error::Invalid("changed_bytes_overflow".into()))?;
        if changed_bytes > MAX_CHANGED_BYTES {
            return Err(Error::Invalid(format!(
                "changed_bytes_exceeded: maximum is {MAX_CHANGED_BYTES}"
            )));
        }
        reports.push(report);
    }
    validate_edit_conflicts(&batch.edits)?;

    let mut output = clone_snapshot(input);
    apply_validated_edits(&mut output, &batch.edits);
    output.validate()?;
    let output_digest = skill_snapshot_digest(&output)?;
    let status = if output_digest == context.input_digest {
        EditApplyStatus::NoChange
    } else {
        EditApplyStatus::Applied
    };

    let mut patch = SkillPatch::default();
    if status == EditApplyStatus::Applied {
        for field in [
            SkillTextField::Content,
            SkillTextField::Applicability,
            SkillTextField::Counterexample,
        ] {
            if field.get(input) != field.get(&output) {
                field.set_patch(&mut patch, field.get(&output).to_owned());
            }
        }
    }

    Ok(CompiledSkillEdit {
        output,
        patch,
        report: EditApplyReport {
            schema_version: SKILL_EDIT_REPORT_SCHEMA.into(),
            compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
            namespace: context.namespace.clone(),
            profile_id: context.profile_id.clone(),
            skill_id: context.skill_id.clone(),
            skill_version: context.skill_version.clone(),
            status,
            input_digest: context.input_digest.clone(),
            output_digest,
            approved_parent_digest: context.approved_parent_digest.clone(),
            safe_baseline_digest: context.safe_baseline_digest.clone(),
            evidence: canonical_evidence_closure(&batch.evidence),
            changed_bytes,
            edits: reports,
        },
    })
}

fn clone_snapshot(input: &SkillSnapshot) -> SkillSnapshot {
    SkillSnapshot {
        content: input.content.clone(),
        applicability: input.applicability.clone(),
        counterexample: input.counterexample.clone(),
        required_capabilities: input.required_capabilities.clone(),
        dependencies: input.dependencies.clone(),
    }
}

fn validate_batch_scope(
    input: &SkillSnapshot,
    context: &TrustedEditContext,
    batch: &SkillEditBatch,
) -> Result<()> {
    if batch.schema_version != SKILL_EDIT_SCHEMA {
        return Err(Error::Invalid("unsupported_skill_edit_schema".into()));
    }
    if batch.compiler_version != SKILL_EDIT_COMPILER_VERSION {
        return Err(Error::Conflict("compiler_version_mismatch".into()));
    }
    for (name, value) in [
        ("namespace", batch.namespace.as_str()),
        ("profile_id", batch.profile_id.as_str()),
        ("skill_id", batch.skill_id.as_str()),
        ("skill_version", batch.skill_version.as_str()),
    ] {
        identifier(value).map_err(|_| Error::Invalid(format!("invalid_batch_scope:{name}")))?;
    }
    validate_digest(&batch.input_digest, "batch input digest")?;
    validate_digest(
        &batch.approved_parent_digest,
        "batch approved parent digest",
    )?;
    validate_digest(&batch.safe_baseline_digest, "batch safe baseline digest")?;

    let actual_input_digest = skill_snapshot_digest(input)?;
    for (name, actual, trusted) in [
        (
            "namespace",
            batch.namespace.as_str(),
            context.namespace.as_str(),
        ),
        (
            "profile_id",
            batch.profile_id.as_str(),
            context.profile_id.as_str(),
        ),
        (
            "skill_id",
            batch.skill_id.as_str(),
            context.skill_id.as_str(),
        ),
        (
            "skill_version",
            batch.skill_version.as_str(),
            context.skill_version.as_str(),
        ),
        (
            "input_digest",
            batch.input_digest.as_str(),
            context.input_digest.as_str(),
        ),
        (
            "approved_parent_digest",
            batch.approved_parent_digest.as_str(),
            context.approved_parent_digest.as_str(),
        ),
        (
            "safe_baseline_digest",
            batch.safe_baseline_digest.as_str(),
            context.safe_baseline_digest.as_str(),
        ),
    ] {
        if actual != trusted {
            return Err(Error::Conflict(format!("edit_scope_mismatch:{name}")));
        }
    }
    if actual_input_digest != context.input_digest {
        return Err(Error::Conflict("trusted_input_digest_mismatch".into()));
    }
    Ok(())
}

fn validate_evidence_closure(
    evidence: &EvidenceClosure,
    allowed: &BTreeSet<EvidenceRef>,
) -> Result<()> {
    if evidence.dependencies.is_empty() {
        return Err(Error::Invalid("empty_evidence_closure".into()));
    }
    let support = validate_evidence_list(&evidence.support, "support")?;
    let counterexamples = validate_evidence_list(&evidence.counterexamples, "counterexamples")?;
    let dependencies = validate_evidence_list(&evidence.dependencies, "dependencies")?;
    validate_source_identities(
        evidence
            .support
            .iter()
            .chain(&evidence.counterexamples)
            .chain(&evidence.dependencies),
        "evidence_closure",
    )?;
    if !support.is_subset(&dependencies) || !counterexamples.is_subset(&dependencies) {
        return Err(Error::Conflict("incomplete_evidence_closure".into()));
    }
    if !dependencies.is_subset(allowed) {
        return Err(Error::Forbidden);
    }
    Ok(())
}

fn validate_evidence_list(values: &[EvidenceRef], name: &str) -> Result<BTreeSet<EvidenceRef>> {
    let mut unique = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for value in values {
        validate_evidence(value)?;
        if !unique.insert(value.clone()) {
            return Err(Error::Invalid(format!("duplicate_evidence:{name}")));
        }
        if !ids.insert(value.id.clone()) {
            return Err(Error::Conflict(format!("source_identity_conflict:{name}")));
        }
    }
    Ok(unique)
}

fn validate_source_identities<'a>(
    values: impl IntoIterator<Item = &'a EvidenceRef>,
    name: &str,
) -> Result<()> {
    let mut by_id = BTreeMap::new();
    for value in values {
        match by_id.insert(value.id.as_str(), value.digest.as_str()) {
            Some(existing) if existing != value.digest => {
                return Err(Error::Conflict(format!("source_identity_conflict:{name}")));
            }
            _ => {}
        }
    }
    Ok(())
}

fn canonical_evidence_closure(evidence: &EvidenceClosure) -> EvidenceClosure {
    let mut canonical = evidence.clone();
    canonical.support.sort();
    canonical.counterexamples.sort();
    canonical.dependencies.sort();
    canonical
}

fn validate_evidence(value: &EvidenceRef) -> Result<()> {
    identifier(&value.id)?;
    validate_digest(&value.digest, "evidence digest")
}

fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(Error::Invalid(format!(
            "{name}: expected lowercase sha256 digest"
        )));
    }
    Ok(())
}

fn validate_protected_range(input: &SkillSnapshot, protected: &ProtectedTextRange) -> Result<()> {
    let source = protected.field.get(input);
    validate_range(source, protected.start, protected.end, "protected_range")?;
    validate_digest(&protected.expected_text_digest, "protected text digest")?;
    if hash(&source.as_bytes()[protected.start..protected.end]) != protected.expected_text_digest {
        return Err(Error::Conflict("protected_preimage_mismatch".into()));
    }
    Ok(())
}

fn validate_edit(
    input: &SkillSnapshot,
    edit: &SkillTextEdit,
    protected_ranges: &[ProtectedTextRange],
) -> Result<AppliedTextEdit> {
    let source = edit.field.get(input);
    validate_range(source, edit.start, edit.end, "edit_range")?;
    validate_digest(&edit.expected_text_digest, "expected text digest")?;

    match &edit.operation {
        TextEditOperation::Insert { text } => {
            if edit.start != edit.end {
                return Err(Error::Invalid("insert_requires_empty_range".into()));
            }
            validate_inserted_text(text, "insert")?;
        }
        TextEditOperation::Replace { text } => {
            if edit.start == edit.end {
                return Err(Error::Invalid("replace_requires_nonempty_range".into()));
            }
            validate_inserted_text(text, "replace")?;
        }
        TextEditOperation::Delete => {
            if edit.start == edit.end {
                return Err(Error::Invalid("delete_requires_nonempty_range".into()));
            }
        }
    }

    let removed = &source[edit.start..edit.end];
    if hash(removed.as_bytes()) != edit.expected_text_digest {
        return Err(Error::Conflict("preimage_digest_mismatch".into()));
    }
    if let Some(anchor) = &edit.exact_anchor {
        validate_exact_anchor(source, edit, anchor)?;
    }
    for protected in protected_ranges
        .iter()
        .filter(|protected| protected.field == edit.field)
    {
        if ranges_conflict(edit.start, edit.end, protected.start, protected.end) {
            return Err(Error::Forbidden);
        }
    }

    let inserted = edit.operation.inserted_text();
    Ok(AppliedTextEdit {
        field: edit.field,
        start: edit.start,
        end: edit.end,
        removed_text: removed.into(),
        inserted_text: inserted.into(),
        removed_digest: hash(removed.as_bytes()),
        inserted_digest: hash(inserted.as_bytes()),
        removed_bytes: removed.len(),
        inserted_bytes: inserted.len(),
    })
}

fn validate_inserted_text(value: &str, operation: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::Invalid(format!(
            "{operation}_requires_nonempty_text"
        )));
    }
    if value.contains('\0') {
        return Err(Error::Invalid(format!("{operation}_contains_nul")));
    }
    Ok(())
}

fn validate_range(source: &str, start: usize, end: usize, name: &str) -> Result<()> {
    if start > end || end > source.len() {
        return Err(Error::Invalid(format!("{name}_out_of_bounds")));
    }
    if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
        return Err(Error::Invalid(format!("{name}_not_utf8_boundary")));
    }
    Ok(())
}

fn validate_exact_anchor(source: &str, edit: &SkillTextEdit, anchor: &ExactAnchor) -> Result<()> {
    if anchor.text.is_empty() {
        return Err(Error::Invalid("empty_exact_anchor".into()));
    }
    let positions = exact_match_positions(source.as_bytes(), anchor.text.as_bytes());
    let anchor_start = match positions.as_slice() {
        [] => return Err(Error::Conflict("anchor_missing".into())),
        [position] => *position,
        _ => return Err(Error::Conflict("anchor_not_unique".into())),
    };
    let anchor_end = anchor_start + anchor.text.len();
    if edit.start < anchor_start || edit.end > anchor_end {
        return Err(Error::Conflict("edit_outside_anchor".into()));
    }
    Ok(())
}

fn exact_match_positions(source: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > source.len() {
        return Vec::new();
    }
    source
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, candidate)| (candidate == needle).then_some(index))
        .collect()
}

fn validate_edit_conflicts(edits: &[SkillTextEdit]) -> Result<()> {
    for (index, left) in edits.iter().enumerate() {
        for right in edits.iter().skip(index + 1) {
            if left.field == right.field
                && ranges_conflict(left.start, left.end, right.start, right.end)
            {
                return Err(Error::Conflict("edit_range_conflict".into()));
            }
        }
    }
    Ok(())
}

fn ranges_conflict(
    left_start: usize,
    left_end: usize,
    right_start: usize,
    right_end: usize,
) -> bool {
    let left_insert = left_start == left_end;
    let right_insert = right_start == right_end;
    match (left_insert, right_insert) {
        (true, true) => left_start == right_start,
        (true, false) => right_start <= left_start && left_start <= right_end,
        (false, true) => left_start <= right_start && right_start <= left_end,
        (false, false) => left_start < right_end && right_start < left_end,
    }
}

fn apply_validated_edits(output: &mut SkillSnapshot, edits: &[SkillTextEdit]) {
    for field in [
        SkillTextField::Content,
        SkillTextField::Applicability,
        SkillTextField::Counterexample,
    ] {
        let mut field_edits = edits
            .iter()
            .filter(|edit| edit.field == field)
            .collect::<Vec<_>>();
        field_edits.sort_by_key(|edit| (edit.start, edit.end));
        let target = field.get_mut(output);
        for edit in field_edits.into_iter().rev() {
            target.replace_range(edit.start..edit.end, edit.operation.inserted_text());
        }
    }
}
