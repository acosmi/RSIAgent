//! A pure, bounded ProgramFixture. Byte tokens belong only to this local codec.
//!
//! Caller-frozen controls bind imported, unverified development material; they do
//! not establish storage authorization, fresh revocation, model safety, or approval.
use crate::contract::SkillSnapshot;
use crate::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use crate::skill_edit::{
    EditApplyReport, EvidenceClosure, EvidenceRef, ProtectedTextRange, SKILL_EDIT_COMPILER_VERSION,
    SKILL_EDIT_SCHEMA, SkillEditBatch, SkillTextEdit, SkillTextField, TextEditOperation,
    TrustedEditContext, compile_skill_edit_batch, parse_skill_edit_batch, skill_snapshot_digest,
};
use crate::{Error, Result, hash, identifier};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

pub const PROGRAM_SCHEMA: &str = "rsia.import_fixture.program.v1";
pub const METHOD_SCHEMA: &str = "rsia.import_fixture.method.v1";
pub const REQUEST_SCHEMA: &str = "rsia.import_fixture.request.v1";
pub const BYTE_CODEC: &str = "rsia.fixture.utf8_json_bytes.v1";
pub const FIXTURE_MODEL: &str = "rsia.fixture.closed_integer_program.v1";
pub const MAX_INPUT_BYTES: usize = 131_072;
pub const MAX_OUTPUT_BYTES: usize = 65_536;
pub const MAX_CONTEXT_TOKENS: u64 = 196_608;
pub const MAX_CASES: usize = 32;
pub const MAX_VALUES: usize = 32;
pub const MAX_SOURCES: usize = 202;
pub const MAX_STEPS: usize = 4;
pub const MAX_CONTEXT_TEXT: usize = 16_384;
pub const MIN_VALUE: i64 = -1_000_000;
pub const MAX_VALUE: i64 = 1_000_000;

const MODEL_DECLARATION: &str =
    "Local deterministic closed integer ProgramFixture; no provider dispatch.";
const POLICY: &str = "ImportedHistory/UnverifiedImport/Development; conditions can narrow the fixed domain; no command execution, historical attestation, approval, or learning claim.";
const TOOL_SCHEMA: &str = r#"{"schema_version":"rsia.fixture.tools.v1","type":"array","maxItems":4,"items":{"enum":["abs","deduplicate","sort_ascending"]},"operations":{"abs":"absolute value of each current integer","deduplicate":"keep first occurrence of each current value without sorting","sort_ascending":"sort current integers ascending"},"external_calls":false}"#;
const OUTPUT_SCHEMA: &str = r##"{"schema_version":"rsia.fixture.edit_output_schema.v1","type":"object","additionalProperties":false,"required":["schema_version","compiler_version","namespace","profile_id","skill_id","skill_version","input_digest","approved_parent_digest","safe_baseline_digest","evidence","edits"],"properties":{"schema_version":{"const":"rsia.skill_edit.v1"},"compiler_version":{"const":"rsia.skill_edit.compiler.v1"},"namespace":{"$ref":"#/$defs/id"},"profile_id":{"$ref":"#/$defs/id"},"skill_id":{"$ref":"#/$defs/id"},"skill_version":{"$ref":"#/$defs/id"},"input_digest":{"$ref":"#/$defs/digest"},"approved_parent_digest":{"$ref":"#/$defs/digest"},"safe_baseline_digest":{"$ref":"#/$defs/digest"},"evidence":{"type":"object","additionalProperties":false,"required":["support","counterexamples","dependencies"],"properties":{"support":{"type":"array","maxItems":32,"items":{"$ref":"#/$defs/source"}},"counterexamples":{"type":"array","maxItems":32,"items":{"$ref":"#/$defs/source"}},"dependencies":{"type":"array","minItems":1,"maxItems":202,"items":{"$ref":"#/$defs/source"}}}},"edits":{"type":"array","maxItems":1,"items":{"type":"object","additionalProperties":false,"required":["field","start","end","expected_text_digest","exact_anchor","operation"],"properties":{"field":{"const":"content"},"start":{"const":0},"end":{"type":"integer","minimum":1},"expected_text_digest":{"$ref":"#/$defs/digest"},"exact_anchor":{"type":"null"},"operation":{"type":"object","additionalProperties":false,"required":["op","text"],"properties":{"op":{"const":"replace"},"text":{"type":"string","maxLength":16384}}}}}}},"$defs":{"id":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9_.:-]+$"},"digest":{"type":"string","pattern":"^[0-9a-f]{64}$"},"source":{"type":"object","additionalProperties":false,"required":["id","digest"],"properties":{"id":{"$ref":"#/$defs/id"},"digest":{"$ref":"#/$defs/digest"}}}}}"##;
const ROLE_FRAMING: &str = "system:model+policy+mandatory; evidence:complete-parent+method+all-sources; user:frozen-control; output:complete-SkillEditBatch-JSON";
// A digest is 64 bytes and Content is the shortest field name. This lower bound
// rejects typed vectors that cannot possibly fit before walking or cloning them.
const MIN_PROTECTED_JSON_BYTES: usize =
    r#"{"field":"content","start":0,"end":0,"expected_text_digest":""}"#.len() + 64;

fn invalid(reason: &'static str) -> Error {
    Error::Invalid(reason.into())
}

fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("fixture_invalid_digest"));
    }
    Ok(())
}

fn id(value: &str) -> Result<()> {
    if value.len() > 128 {
        return Err(invalid("fixture_invalid_identifier"));
    }
    identifier(value).map_err(|_| invalid("fixture_invalid_identifier"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureOperation {
    Abs,
    Deduplicate,
    SortAscending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureCondition {
    pub max_length: usize,
    pub min_value: i64,
    pub max_value: i64,
}

impl Default for FixtureCondition {
    fn default() -> Self {
        Self {
            max_length: MAX_VALUES,
            min_value: MIN_VALUE,
            max_value: MAX_VALUE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureProgram {
    pub schema_version: String,
    pub condition: FixtureCondition,
    pub steps: Vec<FixtureOperation>,
}

impl FixtureProgram {
    fn validate(&self) -> Result<()> {
        if self.schema_version != PROGRAM_SCHEMA {
            return Err(invalid("fixture_unknown_program_schema"));
        }
        if self.steps.len() > MAX_STEPS {
            return Err(invalid("fixture_too_many_steps"));
        }
        if self.condition.max_length > MAX_VALUES
            || self.condition.min_value < MIN_VALUE
            || self.condition.max_value > MAX_VALUE
            || self.condition.min_value > self.condition.max_value
        {
            return Err(invalid("fixture_invalid_condition"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureSourceKind {
    Artifact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureSourceCategory {
    SourceSelection,
    ImportSource,
    ImportResult,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureSourceRef {
    pub kind: FixtureSourceKind,
    pub id: String,
    pub category: FixtureSourceCategory,
    pub object_digest: String,
}

impl FixtureSourceRef {
    fn validate(&self) -> Result<()> {
        id(&self.id)?;
        digest(&self.object_digest)
    }
    fn evidence(&self) -> EvidenceRef {
        EvidenceRef {
            id: self.id.clone(),
            digest: self.object_digest.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureCase {
    pub source: FixtureSourceRef,
    pub locator_digest: String,
    pub excerpt_digest: String,
    pub input: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureMethod {
    pub schema_version: String,
    pub program: FixtureProgram,
    pub dependencies: Vec<FixtureSourceRef>,
    pub support: Vec<FixtureCase>,
    pub counterexamples: Vec<FixtureCase>,
}

fn validate_values(values: &[i64]) -> Result<()> {
    if values.len() > MAX_VALUES || values.iter().any(|v| !(MIN_VALUE..=MAX_VALUE).contains(v)) {
        return Err(invalid("fixture_input_out_of_domain"));
    }
    Ok(())
}

fn validate_identities<'a>(sources: impl IntoIterator<Item = &'a FixtureSourceRef>) -> Result<()> {
    let mut seen = BTreeMap::new();
    for source in sources {
        source.validate()?;
        let identity = (source.kind, source.category, source.object_digest.as_str());
        if let Some(previous) = seen.insert(source.id.as_str(), identity)
            && previous != identity
        {
            return Err(invalid("fixture_source_identity_conflict"));
        }
    }
    Ok(())
}

fn normalize_sources(mut sources: Vec<FixtureSourceRef>) -> Vec<FixtureSourceRef> {
    sources.sort();
    sources.dedup();
    sources
}

fn checked_method(method: &FixtureMethod) -> Result<FixtureMethod> {
    if method.schema_version != METHOD_SCHEMA {
        return Err(invalid("fixture_unknown_method_schema"));
    }
    method.program.validate()?;
    if method.dependencies.len() > MAX_SOURCES
        || method
            .support
            .len()
            .checked_add(method.counterexamples.len())
            .is_none_or(|n| n > MAX_CASES)
    {
        return Err(invalid("fixture_material_cap"));
    }
    for case in method.support.iter().chain(&method.counterexamples) {
        validate_values(&case.input)?;
        digest(&case.locator_digest)?;
        digest(&case.excerpt_digest)?;
    }
    validate_identities(
        method.dependencies.iter().chain(
            method
                .support
                .iter()
                .chain(&method.counterexamples)
                .map(|c| &c.source),
        ),
    )?;
    let mut method = method.clone();
    method.dependencies = normalize_sources(method.dependencies);
    if method.dependencies.is_empty()
        || method
            .support
            .iter()
            .chain(&method.counterexamples)
            .any(|c| method.dependencies.binary_search(&c.source).is_err())
    {
        return Err(invalid("fixture_incomplete_material_closure"));
    }
    Ok(method)
}

/// Parses the bounded, closed method only; imported success claims are not accepted.
pub fn parse_fixture_method(bytes: &[u8]) -> Result<FixtureMethod> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(invalid("fixture_input_byte_cap"));
    }
    let method =
        serde_json::from_slice(bytes).map_err(|_| invalid("fixture_invalid_method_json"))?;
    checked_method(&method)
}

fn checked_parent(parent: &SkillSnapshot) -> Result<()> {
    if parent.content.len() > 16_384
        || parent.applicability.len() > 4096
        || parent.counterexample.len() > 4096
        || parent.required_capabilities.len() > 32
        || parent.dependencies.len() > 32
    {
        return Err(invalid("fixture_invalid_parent"));
    }
    for value in parent
        .required_capabilities
        .iter()
        .chain(&parent.dependencies)
    {
        id(value)?;
    }
    parent
        .validate()
        .map_err(|_| invalid("fixture_invalid_parent"))?;
    parse_program(&parent.content)?;
    Ok(())
}

fn parse_program(content: &str) -> Result<FixtureProgram> {
    if content.len() > 16_384 {
        return Err(invalid("fixture_program_byte_cap"));
    }
    let program: FixtureProgram =
        serde_json::from_str(content).map_err(|_| invalid("fixture_invalid_program_json"))?;
    program.validate()?;
    Ok(program)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureBinding {
    pub namespace: String,
    pub profile_id: String,
    pub skill_id: String,
    pub skill_version: String,
    pub approved_parent_digest: String,
    pub safe_baseline_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureLimits {
    pub input_token_limit: u64,
    pub output_token_reserve: u64,
    pub total_context_limit: u64,
}

impl Default for FixtureLimits {
    fn default() -> Self {
        Self {
            input_token_limit: MAX_INPUT_BYTES as u64,
            output_token_reserve: MAX_OUTPUT_BYTES as u64,
            total_context_limit: MAX_CONTEXT_TOKENS,
        }
    }
}

impl FixtureLimits {
    fn validate(self) -> Result<()> {
        self.input_token_limit
            .checked_add(self.output_token_reserve)
            .ok_or_else(|| invalid("fixture_budget_overflow"))?;
        if self.input_token_limit > MAX_INPUT_BYTES as u64
            || self.output_token_reserve > MAX_OUTPUT_BYTES as u64
            || self.total_context_limit > MAX_CONTEXT_TOKENS
        {
            return Err(invalid("fixture_budget_cap"));
        }
        Ok(())
    }
    fn check_input(self, count: u64) -> Result<()> {
        let total = count
            .checked_add(self.output_token_reserve)
            .ok_or_else(|| invalid("fixture_budget_overflow"))?;
        if count > self.input_token_limit || total > self.total_context_limit {
            return Err(invalid("fixture_input_budget_exceeded"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureContext {
    pub mandatory_context: String,
    pub tool_declarations: String,
}

impl Default for FixtureContext {
    fn default() -> Self {
        Self { mandatory_context: "Evaluate the actual Skill program against the independent absolute-value-set oracle. Keep all required material and every counterexample.".into(),
            tool_declarations: "Abs maps each current value to its absolute value; Deduplicate retains the first current value; SortAscending sorts current values. These are the only operations.".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureProtectedRange {
    pub field: SkillTextField,
    pub start: usize,
    pub end: usize,
    pub expected_text_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlEcho {
    binding: FixtureBinding,
    parent_digest: String,
    allowed_sources: Vec<FixtureSourceRef>,
    required_sources: Vec<FixtureSourceRef>,
    limits: FixtureLimits,
    context: FixtureContext,
    protected: Vec<FixtureProtectedRange>,
}

/// Constructed before encoding by the caller; deliberately not Deserialize.
#[derive(Debug, Clone)]
pub struct FixtureControl {
    echo: ControlEcho,
}

impl FixtureControl {
    pub fn new(
        binding: FixtureBinding,
        parent: &SkillSnapshot,
        allowed_sources: Vec<FixtureSourceRef>,
        required_sources: Vec<FixtureSourceRef>,
        limits: FixtureLimits,
        context: FixtureContext,
        protected: Vec<FixtureProtectedRange>,
    ) -> Result<Self> {
        checked_parent(parent)?;
        limits.validate()?;
        for value in [
            &binding.namespace,
            &binding.profile_id,
            &binding.skill_id,
            &binding.skill_version,
        ] {
            id(value)?;
        }
        digest(&binding.approved_parent_digest)?;
        digest(&binding.safe_baseline_digest)?;
        if allowed_sources.len() > MAX_SOURCES
            || required_sources.len() > MAX_SOURCES
            || protected
                .len()
                .checked_mul(MIN_PROTECTED_JSON_BYTES)
                .is_none_or(|n| n > MAX_INPUT_BYTES)
        {
            return Err(invalid("fixture_control_cap"));
        }
        for value in [&context.mandatory_context, &context.tool_declarations] {
            if value.len() > MAX_CONTEXT_TEXT || value.trim().is_empty() || value.contains('\0') {
                return Err(invalid("fixture_context_text_cap"));
            }
        }
        validate_identities(allowed_sources.iter().chain(&required_sources))?;
        let allowed_sources = normalize_sources(allowed_sources);
        let required_sources = normalize_sources(required_sources);
        if required_sources.is_empty()
            || required_sources
                .iter()
                .any(|r| allowed_sources.binary_search(r).is_err())
        {
            return Err(invalid("fixture_required_source_not_allowed"));
        }
        for range in &protected {
            digest(&range.expected_text_digest)?;
            let text = match range.field {
                SkillTextField::Content => &parent.content,
                SkillTextField::Applicability => &parent.applicability,
                SkillTextField::Counterexample => &parent.counterexample,
            };
            if range.start > range.end
                || range.end > text.len()
                || !text.is_char_boundary(range.start)
                || !text.is_char_boundary(range.end)
                || hash(&text.as_bytes()[range.start..range.end]) != range.expected_text_digest
            {
                return Err(invalid("fixture_invalid_protected_preimage"));
            }
        }
        let control = Self {
            echo: ControlEcho {
                binding,
                parent_digest: skill_snapshot_digest(parent)
                    .map_err(|_| invalid("fixture_invalid_parent"))?,
                allowed_sources,
                required_sources,
                limits,
                context,
                protected,
            },
        };
        // Even an already typed control is bounded before any subsequent clone.
        bounded_json(&control.echo, MAX_INPUT_BYTES, "fixture_control_cap")?;
        control.edit_context(parent)?;
        Ok(control)
    }

    fn edit_context(&self, parent: &SkillSnapshot) -> Result<TrustedEditContext> {
        let b = &self.echo.binding;
        TrustedEditContext::new(
            &b.namespace,
            &b.profile_id,
            &b.skill_id,
            &b.skill_version,
            &b.approved_parent_digest,
            &b.safe_baseline_digest,
            parent,
            self.echo
                .allowed_sources
                .iter()
                .map(FixtureSourceRef::evidence),
        )
        .map_err(|_| invalid("fixture_invalid_edit_control"))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureRequest {
    schema_version: String,
    codec: String,
    model: String,
    model_declaration: String,
    policy: String,
    tool_schema: String,
    output_schema: String,
    role_framing: String,
    historical_origin: TaskOrigin,
    historical_attestation: ExecutionAttestation,
    historical_purpose: Purpose,
    control: ControlEcho,
    parent: SkillSnapshot,
    method: FixtureMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncodedFixtureRequest {
    /// Each UTF-8 JSON byte is one token in this fixed local byte vocabulary.
    pub bytes: Vec<u8>,
    pub input_tokens: u64,
    pub digest: String,
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(io::Error::other("fixture_byte_cap"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bounded_json<T: Serialize>(value: &T, limit: usize, reason: &'static str) -> Result<Vec<u8>> {
    // Reserving the cap avoids Vec geometric growth beyond the frozen byte cap.
    let mut writer = BoundedWriter {
        bytes: Vec::with_capacity(limit),
        limit,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| invalid(reason))?;
    Ok(writer.bytes)
}

fn bind_method(control: &FixtureControl, method: &FixtureMethod) -> Result<()> {
    if method.dependencies != control.echo.required_sources {
        return Err(invalid("fixture_required_material_mismatch"));
    }
    Ok(())
}

/// Encodes every consumer input; no truncation or implicit removal of material.
pub fn encode_fixture_request(
    control: &FixtureControl,
    parent: &SkillSnapshot,
    method: &FixtureMethod,
) -> Result<EncodedFixtureRequest> {
    checked_parent(parent)?;
    if skill_snapshot_digest(parent).map_err(|_| invalid("fixture_invalid_parent"))?
        != control.echo.parent_digest
    {
        return Err(invalid("fixture_parent_mismatch"));
    }
    let method = checked_method(method)?;
    bind_method(control, &method)?;
    let request = FixtureRequest {
        schema_version: REQUEST_SCHEMA.into(),
        codec: BYTE_CODEC.into(),
        model: FIXTURE_MODEL.into(),
        model_declaration: MODEL_DECLARATION.into(),
        policy: POLICY.into(),
        tool_schema: TOOL_SCHEMA.into(),
        output_schema: OUTPUT_SCHEMA.into(),
        role_framing: ROLE_FRAMING.into(),
        historical_origin: TaskOrigin::ImportedHistory,
        historical_attestation: ExecutionAttestation::UnverifiedImport,
        historical_purpose: Purpose::Development,
        control: control.echo.clone(),
        parent: parent.clone(),
        method,
    };
    let bytes = bounded_json(&request, MAX_INPUT_BYTES, "fixture_input_byte_cap")?;
    let input_tokens = bytes.len() as u64;
    control.echo.limits.check_input(input_tokens)?;
    Ok(EncodedFixtureRequest {
        digest: hash(&bytes),
        bytes,
        input_tokens,
    })
}

fn decode_request(
    control: &FixtureControl,
    encoded: &EncodedFixtureRequest,
) -> Result<FixtureRequest> {
    if encoded.bytes.len() > MAX_INPUT_BYTES {
        return Err(invalid("fixture_input_byte_cap"));
    }
    if encoded.input_tokens != encoded.bytes.len() as u64 || encoded.digest != hash(&encoded.bytes)
    {
        return Err(invalid("fixture_input_meter_mismatch"));
    }
    control.echo.limits.check_input(encoded.input_tokens)?;
    let mut request: FixtureRequest = serde_json::from_slice(&encoded.bytes)
        .map_err(|_| invalid("fixture_invalid_request_json"))?;
    if request.schema_version != REQUEST_SCHEMA
        || request.codec != BYTE_CODEC
        || request.model != FIXTURE_MODEL
        || request.model_declaration != MODEL_DECLARATION
        || request.policy != POLICY
        || request.tool_schema != TOOL_SCHEMA
        || request.output_schema != OUTPUT_SCHEMA
        || request.role_framing != ROLE_FRAMING
        || request.historical_origin != TaskOrigin::ImportedHistory
        || request.historical_attestation != ExecutionAttestation::UnverifiedImport
        || request.historical_purpose != Purpose::Development
    {
        return Err(invalid("fixture_unknown_protocol"));
    }
    if request.control != control.echo {
        return Err(invalid("fixture_control_mismatch"));
    }
    checked_parent(&request.parent)?;
    if skill_snapshot_digest(&request.parent).map_err(|_| invalid("fixture_invalid_parent"))?
        != control.echo.parent_digest
    {
        return Err(invalid("fixture_parent_mismatch"));
    }
    request.method = checked_method(&request.method)?;
    bind_method(control, &request.method)?;
    if bounded_json(&request, MAX_INPUT_BYTES, "fixture_input_byte_cap")? != encoded.bytes {
        return Err(invalid("fixture_noncanonical_input"));
    }
    Ok(request)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum FixtureAssessment {
    Supported {
        output: Vec<i64>,
        oracle: Vec<i64>,
        matched: bool,
        local_operations: u64,
    },
    Unsupported {
        oracle: Vec<i64>,
    },
}

fn oracle(input: &[i64]) -> Vec<i64> {
    // Independently derives the target from ORIGINAL input, never from program output.
    input
        .iter()
        .map(|&value| if value < 0 { -value } else { value })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Re-reads and strictly parses the actual compiled Skill body on every call.
pub fn execute_program_fixture(skill: &SkillSnapshot, input: &[i64]) -> Result<FixtureAssessment> {
    validate_values(input)?;
    let program = parse_program(&skill.content)?;
    let oracle = oracle(input);
    if input.len() > program.condition.max_length
        || input
            .iter()
            .any(|v| *v < program.condition.min_value || *v > program.condition.max_value)
    {
        return Ok(FixtureAssessment::Unsupported { oracle });
    }
    let mut output = input.to_vec();
    for step in &program.steps {
        match step {
            FixtureOperation::Abs => {
                for value in &mut output {
                    *value = value.abs();
                }
            }
            FixtureOperation::Deduplicate => {
                let mut seen = BTreeSet::new();
                output.retain(|value| seen.insert(*value));
            }
            FixtureOperation::SortAscending => output.sort_unstable(),
        }
    }
    Ok(FixtureAssessment::Supported {
        matched: output == oracle,
        output,
        oracle,
        local_operations: program.steps.len() as u64,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureExecutionKind {
    ProgramFixture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureCaseRole {
    Support,
    Counterexample,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureCaseResult {
    pub role: FixtureCaseRole,
    pub case: FixtureCase,
    pub assessment: FixtureAssessment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureDeclarationDigests {
    pub model: String,
    pub policy: String,
    pub tools: String,
    pub output_schema: String,
    pub role_framing: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FixtureRun {
    pub historical_origin: TaskOrigin,
    pub historical_attestation: ExecutionAttestation,
    pub historical_purpose: Purpose,
    pub execution_kind: FixtureExecutionKind,
    pub execution_purpose: Purpose,
    pub identity_program: bool,
    pub output: SkillSnapshot,
    pub edit_report: EditApplyReport,
    pub dependencies: Vec<FixtureSourceRef>,
    pub cases: Vec<FixtureCaseResult>,
    pub input_tokens: u64,
    pub input_digest: String,
    pub edit_response: Vec<u8>,
    pub output_tokens: u64,
    pub output_digest: String,
    pub declaration_digests: FixtureDeclarationDigests,
    pub local_operations: u64,
    pub real_provider_calls: u64,
}

/// Consumes only the same metered encoded fields, compiles the same metered output,
/// and then executes the compiled Skill body. No broker, ledger, or storage side effects.
pub fn run_program_fixture(
    control: &FixtureControl,
    encoded: &EncodedFixtureRequest,
) -> Result<FixtureRun> {
    let request = decode_request(control, encoded)?;
    let content = bounded_json(&request.method.program, 16_384, "fixture_program_byte_cap")?;
    let content =
        String::from_utf8(content).map_err(|_| invalid("fixture_invalid_program_json"))?;
    let edits = if content == request.parent.content {
        Vec::new()
    } else {
        vec![SkillTextEdit {
            field: SkillTextField::Content,
            start: 0,
            end: request.parent.content.len(),
            expected_text_digest: hash(request.parent.content.as_bytes()),
            exact_anchor: None,
            operation: TextEditOperation::Replace { text: content },
        }]
    };
    let evidence_set = |cases: &[FixtureCase]| {
        cases
            .iter()
            .map(|c| c.source.evidence())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    let b = &control.echo.binding;
    let batch = SkillEditBatch {
        schema_version: SKILL_EDIT_SCHEMA.into(),
        compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
        namespace: b.namespace.clone(),
        profile_id: b.profile_id.clone(),
        skill_id: b.skill_id.clone(),
        skill_version: b.skill_version.clone(),
        input_digest: control.echo.parent_digest.clone(),
        approved_parent_digest: b.approved_parent_digest.clone(),
        safe_baseline_digest: b.safe_baseline_digest.clone(),
        evidence: EvidenceClosure {
            support: evidence_set(&request.method.support),
            counterexamples: evidence_set(&request.method.counterexamples),
            dependencies: request
                .method
                .dependencies
                .iter()
                .map(FixtureSourceRef::evidence)
                .collect(),
        },
        edits,
    };
    let edit_response = bounded_json(&batch, MAX_OUTPUT_BYTES, "fixture_output_byte_cap")?;
    let output_tokens = edit_response.len() as u64;
    if output_tokens > control.echo.limits.output_token_reserve {
        return Err(invalid("fixture_output_budget_exceeded"));
    }
    // Parse and apply exactly these measured bytes, not the pre-encoding batch.
    let parsed = parse_skill_edit_batch(&edit_response)
        .map_err(|_| invalid("fixture_invalid_edit_response"))?;
    let context = control.edit_context(&request.parent)?;
    let protected = control
        .echo
        .protected
        .iter()
        .map(|r| ProtectedTextRange::new(r.field, r.start, r.end, &r.expected_text_digest))
        .collect::<Vec<_>>();
    let compiled = compile_skill_edit_batch(&request.parent, &context, &parsed, &protected)
        .map_err(|_| invalid("fixture_edit_rejected"))?;
    let mut cases =
        Vec::with_capacity(request.method.support.len() + request.method.counterexamples.len());
    let mut local_operations = 0u64;
    for (role, values) in [
        (FixtureCaseRole::Support, &request.method.support),
        (
            FixtureCaseRole::Counterexample,
            &request.method.counterexamples,
        ),
    ] {
        for case in values {
            let assessment = execute_program_fixture(&compiled.output, &case.input)?;
            if let FixtureAssessment::Supported {
                local_operations: count,
                ..
            } = &assessment
            {
                local_operations = local_operations
                    .checked_add(*count)
                    .ok_or_else(|| invalid("fixture_operation_overflow"))?;
            }
            cases.push(FixtureCaseResult {
                role,
                case: case.clone(),
                assessment,
            });
        }
    }
    let declaration_digests = FixtureDeclarationDigests {
        model: hash(request.model_declaration.as_bytes()),
        policy: hash(request.policy.as_bytes()),
        tools: hash(&bounded_json(
            &(
                &request.tool_schema,
                &request.control.context.tool_declarations,
            ),
            MAX_INPUT_BYTES,
            "fixture_input_byte_cap",
        )?),
        output_schema: hash(request.output_schema.as_bytes()),
        role_framing: hash(request.role_framing.as_bytes()),
    };
    Ok(FixtureRun {
        historical_origin: request.historical_origin,
        historical_attestation: request.historical_attestation,
        historical_purpose: request.historical_purpose,
        execution_kind: FixtureExecutionKind::ProgramFixture,
        execution_purpose: Purpose::Development,
        identity_program: request.method.program.steps.is_empty(),
        output: compiled.output,
        edit_report: compiled.report,
        dependencies: request.method.dependencies,
        cases,
        input_tokens: encoded.bytes.len() as u64,
        input_digest: hash(&encoded.bytes),
        output_digest: hash(&edit_response),
        edit_response,
        output_tokens,
        declaration_digests,
        local_operations,
        real_provider_calls: 0,
    })
}
