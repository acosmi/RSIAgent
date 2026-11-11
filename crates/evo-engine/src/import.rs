//! Versioned history readers. Import is not a trusted execution identity.
//! Strict implementation of v4.1 §6.3, §5.6, §13.1 E16.1.
use evo_core::evidence::{
    AggregateSummary, EvidenceLocator, EvidenceMember, EvidenceSet, ExecutionAttestation,
    IMPORT_NORMALIZATION_V2, IMPORT_READER_SET_V2, IMPORT_RESULT_SCHEMA_V2, IMPORT_RESULT_VERSIONS,
    ImportArtifactRef, ImportArtifactSchema, ImportMaterialCursor, ImportMaterialDispatchPolicy,
    ImportMaterialPrivacyStatus, ImportMaterialReadRequest, ImportProjectionCoverage,
    ImportedDevelopmentMaterial, ImportedMaterialExcerpt, ImportedSourceBinding,
    LEGACY_IMPORT_RESULT_SCHEMA_V1, MAX_DISCOVERED_FILES, MAX_EVENT_BYTES, MAX_EXCERPTS,
    MAX_HEADER_PROBE_BYTES, MAX_TOTAL_EXCERPT_BYTES, MAX_TOTAL_READ_BYTES, Purpose, SourceCoverage,
    SourceSelection, TaskOrigin, TraceCoverage, assert_authorized_path, import_result_id_v2,
    legacy_import_result_id_v1,
};
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier, now};
use evo_storage::{ManagementImportStatusCounts, Store, VerifiedBlobInput};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    RsiaTraceV1,
    RsihPiFixture,
    ClaudeFixture,
}

impl SourceFormat {
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::RsiaTraceV1 => "rsia.trace.v1",
            Self::RsihPiFixture => "rsih.pi.fixture",
            Self::ClaudeFixture => "claude.fixture",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportForensicLimits {
    pub max_files: usize,
    pub max_header_probe_bytes: usize,
    pub max_total_read_bytes: usize,
    pub max_event_bytes: usize,
    pub max_excerpts: usize,
    pub max_total_excerpt_bytes: usize,
}

impl Default for ImportForensicLimits {
    fn default() -> Self {
        Self {
            max_files: MAX_DISCOVERED_FILES,
            max_header_probe_bytes: MAX_HEADER_PROBE_BYTES,
            max_total_read_bytes: MAX_TOTAL_READ_BYTES,
            max_event_bytes: MAX_EVENT_BYTES,
            max_excerpts: MAX_EXCERPTS,
            max_total_excerpt_bytes: MAX_TOTAL_EXCERPT_BYTES,
        }
    }
}

impl ImportForensicLimits {
    fn validate(&self) -> Result<()> {
        if self.max_files == 0
            || self.max_files > MAX_DISCOVERED_FILES
            || self.max_header_probe_bytes == 0
            || self.max_header_probe_bytes > MAX_HEADER_PROBE_BYTES
            || self.max_total_read_bytes == 0
            || self.max_total_read_bytes > MAX_TOTAL_READ_BYTES
            || self.max_event_bytes == 0
            || self.max_event_bytes > MAX_EVENT_BYTES
            || self.max_excerpts > MAX_EXCERPTS
            || self.max_total_excerpt_bytes > MAX_TOTAL_EXCERPT_BYTES
        {
            return Err(Error::Invalid("invalid import forensic limits".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedEvent {
    #[serde(default)]
    pub source_id: String,
    pub origin_name: String,
    pub role: String,
    pub format: SourceFormat,
    pub task_origin: TaskOrigin,
    pub attestation: ExecutionAttestation,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub event_index: usize,
    #[serde(default)]
    pub byte_start: usize,
    #[serde(default)]
    pub byte_end: usize,
    #[serde(default)]
    pub raw_fragment_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTraceExporter {
    pub name: String,
    pub version: String,
    pub commit: String,
}

/// The exporter's redaction claim. Recorded as an unverified declaration; it
/// proves no privacy scan or human review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTraceRedactionReport {
    pub schema_version: String,
    pub scan_status: String,
    pub review_status: String,
    pub removed_items: u64,
    pub redacted_items: u64,
}

/// The `rsia.trace.v1` header every record repeats in full.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTraceHeader {
    pub exporter: NativeTraceExporter,
    pub run_id: String,
    pub redaction_report: NativeTraceRedactionReport,
    /// Self-reported; kept apart from this import's own statistics and only able
    /// to lower a result to partial.
    pub coverage: TraceCoverage,
}

/// One Native source of a v2 result: its header and the original sequence of
/// each persisted event (`event_index` remains the locator index).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTraceSource {
    pub source_id: String,
    pub header: NativeTraceHeader,
    pub event_sequences: Vec<(usize, u64)>,
}

pub fn detect_format(label: &str) -> Result<SourceFormat> {
    match label {
        "rsia.trace.v1" => Ok(SourceFormat::RsiaTraceV1),
        "rsih.pi.fixture" => Ok(SourceFormat::RsihPiFixture),
        "claude.fixture" => Ok(SourceFormat::ClaudeFixture),
        "codex" => Err(Error::Invalid("unsupported_format:codex".into())),
        _ => Err(Error::Invalid("unsupported_format".into())),
    }
}

#[derive(Debug)]
enum JsonNode {
    Object {
        start: usize,
        end: usize,
        fields: Vec<(String, JsonNode)>,
    },
    Array {
        start: usize,
        end: usize,
        elements: Vec<JsonNode>,
    },
    Scalar {
        start: usize,
        end: usize,
        value: Value,
    },
}

impl JsonNode {
    fn span(&self) -> (usize, usize) {
        match self {
            Self::Object { start, end, .. }
            | Self::Array { start, end, .. }
            | Self::Scalar { start, end, .. } => (*start, *end),
        }
    }

    fn field(&self, key: &str) -> Option<&JsonNode> {
        match self {
            Self::Object { fields, .. } => fields
                .iter()
                .find(|(candidate, _)| candidate == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Scalar { value, .. } => value.as_str(),
            _ => None,
        }
    }

    fn array(&self) -> Option<&[JsonNode]> {
        match self {
            Self::Array { elements, .. } => Some(elements),
            _ => None,
        }
    }
}

struct StrictJsonParser<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> StrictJsonParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn document(mut self) -> Result<JsonNode> {
        self.skip_ws();
        let node = self.value()?;
        self.skip_ws();
        if self.offset != self.bytes.len() {
            return Err(Error::Invalid("parse_error".into()));
        }
        Ok(node)
    }

    fn value(&mut self) -> Result<JsonNode> {
        self.skip_ws();
        match self.bytes.get(self.offset).copied() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => {
                let start = self.offset;
                self.string_token()?;
                self.scalar(start)
            }
            Some(b'-' | b'0'..=b'9' | b't' | b'f' | b'n') => {
                let start = self.offset;
                while self.bytes.get(self.offset).is_some_and(|byte| {
                    !matches!(byte, b',' | b']' | b'}' | b' ' | b'\n' | b'\r' | b'\t')
                }) {
                    self.offset += 1;
                }
                self.scalar(start)
            }
            _ => Err(Error::Invalid("parse_error".into())),
        }
    }

    fn scalar(&self, start: usize) -> Result<JsonNode> {
        let value = serde_json::from_slice(&self.bytes[start..self.offset])
            .map_err(|_| Error::Invalid("parse_error".into()))?;
        Ok(JsonNode::Scalar {
            start,
            end: self.offset,
            value,
        })
    }

    fn object(&mut self) -> Result<JsonNode> {
        let start = self.offset;
        self.offset += 1;
        self.skip_ws();
        let mut fields = Vec::new();
        let mut keys = BTreeSet::new();
        if self.consume(b'}') {
            return Ok(JsonNode::Object {
                start,
                end: self.offset,
                fields,
            });
        }
        loop {
            self.skip_ws();
            let key_start = self.offset;
            self.string_token()?;
            let key: String = serde_json::from_slice(&self.bytes[key_start..self.offset])
                .map_err(|_| Error::Invalid("parse_error".into()))?;
            if !keys.insert(key.clone()) {
                return Err(Error::Invalid("duplicate_json_key".into()));
            }
            self.skip_ws();
            if !self.consume(b':') {
                return Err(Error::Invalid("parse_error".into()));
            }
            fields.push((key, self.value()?));
            self.skip_ws();
            if self.consume(b'}') {
                break;
            }
            if !self.consume(b',') {
                return Err(Error::Invalid("parse_error".into()));
            }
        }
        Ok(JsonNode::Object {
            start,
            end: self.offset,
            fields,
        })
    }

    fn array(&mut self) -> Result<JsonNode> {
        let start = self.offset;
        self.offset += 1;
        self.skip_ws();
        let mut elements = Vec::new();
        if self.consume(b']') {
            return Ok(JsonNode::Array {
                start,
                end: self.offset,
                elements,
            });
        }
        loop {
            elements.push(self.value()?);
            self.skip_ws();
            if self.consume(b']') {
                break;
            }
            if !self.consume(b',') {
                return Err(Error::Invalid("parse_error".into()));
            }
        }
        Ok(JsonNode::Array {
            start,
            end: self.offset,
            elements,
        })
    }

    fn string_token(&mut self) -> Result<()> {
        if !self.consume(b'"') {
            return Err(Error::Invalid("parse_error".into()));
        }
        let mut escaped = false;
        while let Some(byte) = self.bytes.get(self.offset).copied() {
            self.offset += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return Ok(());
            } else if byte < 0x20 {
                return Err(Error::Invalid("parse_error".into()));
            }
        }
        Err(Error::Invalid("parse_error".into()))
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.bytes.get(self.offset) == Some(&expected) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        while self
            .bytes
            .get(self.offset)
            .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.offset += 1;
        }
    }
}

fn strict_json_document(bytes: &[u8]) -> Result<JsonNode> {
    std::str::from_utf8(bytes).map_err(|_| Error::Invalid("invalid_utf8".into()))?;
    StrictJsonParser::new(bytes).document()
}

pub fn find_json_array_element_spans(text: &str, array_key: &str) -> Vec<(usize, usize)> {
    strict_json_document(text.as_bytes())
        .ok()
        .and_then(|root| {
            root.field(array_key)?
                .array()
                .map(|items| items.iter().map(JsonNode::span).collect())
        })
        .unwrap_or_default()
}

pub fn find_top_level_json_array_spans(text: &str) -> Vec<(usize, usize)> {
    strict_json_document(text.as_bytes())
        .ok()
        .and_then(|root| {
            root.array()
                .map(|items| items.iter().map(JsonNode::span).collect())
        })
        .unwrap_or_default()
}

pub fn detect_format_from_bytes(bytes: &[u8]) -> Result<SourceFormat> {
    detect_format_from_bytes_with_limit(bytes, MAX_HEADER_PROBE_BYTES)
}

fn detect_format_from_bytes_with_limit(
    bytes: &[u8],
    max_probe_bytes: usize,
) -> Result<SourceFormat> {
    if max_probe_bytes == 0 || max_probe_bytes > MAX_HEADER_PROBE_BYTES {
        return Err(Error::Invalid("invalid header probe bound".into()));
    }
    let probe = &bytes[..bytes.len().min(max_probe_bytes)];
    if let Err(error) = std::str::from_utf8(probe)
        && (error.error_len().is_some() || bytes.len() <= max_probe_bytes)
    {
        return Err(Error::Invalid("invalid_utf8".into()));
    }
    if bytes.len() <= max_probe_bytes {
        if let Ok(root) = strict_json_document(probe) {
            return explicit_format(&root);
        }
        if let Some(line) = probe
            .split(|byte| *byte == b'\n')
            .find(|line| !line.iter().all(u8::is_ascii_whitespace))
        {
            return explicit_format(&strict_json_document(line)?);
        }
        return Err(Error::Invalid("unsupported_format".into()));
    }
    let discriminator = probe_object_discriminator(probe)?
        .ok_or_else(|| Error::Invalid("unsupported_format".into()))?;
    format_from_discriminator(&discriminator)
}

fn explicit_format(root: &JsonNode) -> Result<SourceFormat> {
    let schema = root.field("schema_version").and_then(JsonNode::as_str);
    let format = root.field("format").and_then(JsonNode::as_str);
    match (schema, format) {
        (Some(left), Some(right))
            if format_from_discriminator(left)? != format_from_discriminator(right)? =>
        {
            Err(Error::Conflict("format discriminator mismatch".into()))
        }
        (Some(value), _) | (_, Some(value)) => format_from_discriminator(value),
        _ => Err(Error::Invalid("unsupported_format".into())),
    }
}

/// Closed exchange-format table. The internal authority schema
/// `rsia.optimization.source.v1` is not an exchange format and stays unsupported.
fn format_from_discriminator(value: &str) -> Result<SourceFormat> {
    match value {
        "rsia.trace.v1" => Ok(SourceFormat::RsiaTraceV1),
        "rsih.pi.fixture" => Ok(SourceFormat::RsihPiFixture),
        "claude.fixture" => Ok(SourceFormat::ClaudeFixture),
        value if value.contains("codex") => Err(Error::Invalid("unsupported_format:codex".into())),
        value => Err(Error::Invalid(format!("unsupported_format:{value}"))),
    }
}

fn probe_object_discriminator(bytes: &[u8]) -> Result<Option<String>> {
    let mut offset = 0;
    skip_ws(bytes, &mut offset);
    if bytes.get(offset) != Some(&b'{') {
        return Ok(None);
    }
    offset += 1;
    let mut keys = BTreeSet::new();
    let mut discriminator: Option<String> = None;
    loop {
        skip_ws(bytes, &mut offset);
        if offset >= bytes.len() || bytes[offset] == b'}' {
            return Ok(discriminator);
        }
        let key_end = scan_string_end(bytes, offset)?;
        let key: String = serde_json::from_slice(&bytes[offset..key_end])
            .map_err(|_| Error::Invalid("parse_error".into()))?;
        if !keys.insert(key.clone()) {
            return Err(Error::Invalid("duplicate_json_key".into()));
        }
        offset = key_end;
        skip_ws(bytes, &mut offset);
        if bytes.get(offset) != Some(&b':') {
            return Err(Error::Invalid("parse_error".into()));
        }
        offset += 1;
        skip_ws(bytes, &mut offset);
        if key == "schema_version" || key == "format" {
            let value_end = scan_string_end(bytes, offset)?;
            let value: String = serde_json::from_slice(&bytes[offset..value_end])
                .map_err(|_| Error::Invalid("parse_error".into()))?;
            if let Some(existing) = &discriminator
                && format_from_discriminator(existing)? != format_from_discriminator(&value)?
            {
                return Err(Error::Conflict("format discriminator mismatch".into()));
            }
            discriminator = Some(value);
            offset = value_end;
        } else if let Some(end) = scan_value_end(bytes, offset)? {
            offset = end;
        } else {
            return Ok(discriminator);
        }
        skip_ws(bytes, &mut offset);
        match bytes.get(offset) {
            Some(b',') => offset += 1,
            Some(b'}') => return Ok(discriminator),
            None => return Ok(discriminator),
            _ => return Err(Error::Invalid("parse_error".into())),
        }
    }
}

fn skip_ws(bytes: &[u8], offset: &mut usize) {
    while bytes
        .get(*offset)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        *offset += 1;
    }
}

fn scan_string_end(bytes: &[u8], start: usize) -> Result<usize> {
    if bytes.get(start) != Some(&b'"') {
        return Err(Error::Invalid("parse_error".into()));
    }
    let mut offset = start + 1;
    let mut escaped = false;
    while let Some(byte) = bytes.get(offset).copied() {
        offset += 1;
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return Ok(offset);
        } else if byte < 0x20 {
            return Err(Error::Invalid("parse_error".into()));
        }
    }
    Err(Error::Invalid("probe_truncated_string".into()))
}

fn scan_value_end(bytes: &[u8], start: usize) -> Result<Option<usize>> {
    match bytes.get(start).copied() {
        Some(b'"') => match scan_string_end(bytes, start) {
            Ok(end) => Ok(Some(end)),
            Err(Error::Invalid(message)) if message == "probe_truncated_string" => Ok(None),
            Err(error) => Err(error),
        },
        Some(b'{' | b'[') => {
            let mut stack = vec![bytes[start]];
            let mut offset = start + 1;
            let mut in_string = false;
            let mut escaped = false;
            while let Some(byte) = bytes.get(offset).copied() {
                offset += 1;
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                    continue;
                }
                match byte {
                    b'"' => in_string = true,
                    b'{' | b'[' => stack.push(byte),
                    b'}' if stack.pop() != Some(b'{') => {
                        return Err(Error::Invalid("parse_error".into()));
                    }
                    b']' if stack.pop() != Some(b'[') => {
                        return Err(Error::Invalid("parse_error".into()));
                    }
                    b'}' | b']' if stack.is_empty() => return Ok(Some(offset)),
                    _ => {}
                }
            }
            Ok(None)
        }
        Some(_) => {
            let mut offset = start;
            while bytes.get(offset).is_some_and(|byte| {
                !matches!(byte, b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t')
            }) {
                offset += 1;
            }
            if offset == bytes.len() {
                Ok(None)
            } else {
                serde_json::from_slice::<Value>(&bytes[start..offset])
                    .map_err(|_| Error::Invalid("parse_error".into()))?;
                Ok(Some(offset))
            }
        }
        None => Ok(None),
    }
}

/// Role labels are compared trimmed and ASCII case-insensitively, so `"User"`
/// or `" user"` are the user role.
fn is_user_role(role: &str) -> bool {
    role.trim().eq_ignore_ascii_case("user")
}

/// Tool output (`tool_result`, `command_echo`, `summary`) is never a human
/// preference. The role label (normalised via `is_user_role`) no longer gates the
/// rule: a spelling variant such as `"User"` or a non-user role such as
/// `"assistant"` cannot re-open preference eligibility for tool output.
pub fn tool_result_is_not_preference(role: &str, kind: &str) -> bool {
    let _is_user = is_user_role(role);
    !matches!(
        kind.trim().to_ascii_lowercase().as_str(),
        "tool_result" | "command_echo" | "summary"
    )
}

/// Strips retry/fork suffixes (`:try1`, `.try2`, `.try1:fork2`, ...) to a fixed
/// point, so nested separators still collapse to the incident id.
pub fn extract_cluster_id(source_id: &str) -> String {
    let mut current = source_id;
    loop {
        let next = match current.split_once(':') {
            Some((prefix, _)) => prefix,
            None => match current.split_once('.') {
                Some((prefix, _)) => prefix,
                None => current,
            },
        };
        if next == current {
            return current.to_string();
        }
        current = next;
    }
}

/// Independent clusters are the transitive closure (union-find) of two relations
/// over the ingested sources: same incident id (`extract_cluster_id` of the cluster
/// hint) and identical content digest. A component that stays within one incident
/// keeps that incident id as its label; a component that merged several incidents
/// through identical bytes is labelled `content_<digest>` after the smallest shared
/// digest, so byte copies under different family names are still one cluster.
fn independent_clusters(cluster_inputs: &[(String, String)]) -> BTreeSet<String> {
    fn find(parents: &mut [usize], index: usize) -> usize {
        let mut root = index;
        while parents[root] != root {
            root = parents[root];
        }
        let mut current = index;
        while parents[current] != root {
            let next = parents[current];
            parents[current] = root;
            current = next;
        }
        root
    }
    fn union(parents: &mut [usize], a: usize, b: usize) {
        let (a, b) = (find(parents, a), find(parents, b));
        if a != b {
            parents[a.max(b)] = a.min(b);
        }
    }

    let incident_ids: Vec<String> = cluster_inputs
        .iter()
        .map(|(hint, _)| {
            if hint.contains('.') || hint.contains(':') {
                extract_cluster_id(hint)
            } else {
                "unverified_import_unknown".into()
            }
        })
        .collect();
    let mut parents: Vec<usize> = (0..cluster_inputs.len()).collect();
    let mut by_incident: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_digest: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, (incident, (_, digest))) in incident_ids.iter().zip(cluster_inputs).enumerate() {
        if let Some(first) = by_incident.insert(incident.as_str(), index) {
            union(&mut parents, first, index);
        }
        if let Some(first) = by_digest.insert(digest.as_str(), index) {
            union(&mut parents, first, index);
        }
    }

    let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for index in 0..cluster_inputs.len() {
        let root = find(&mut parents, index);
        components.entry(root).or_default().push(index);
    }
    components
        .values()
        .map(|members| {
            let incidents: BTreeSet<&str> =
                members.iter().map(|&i| incident_ids[i].as_str()).collect();
            if incidents.len() == 1 {
                return incident_ids[members[0]].clone();
            }
            let mut digest_counts: BTreeMap<&str, usize> = BTreeMap::new();
            for &member in members {
                *digest_counts
                    .entry(cluster_inputs[member].1.as_str())
                    .or_default() += 1;
            }
            let shared = digest_counts
                .iter()
                .find(|(_, count)| **count > 1)
                .map(|(digest, _)| *digest)
                .unwrap_or(cluster_inputs[members[0]].1.as_str());
            format!("content_{}", &shared[..16])
        })
        .collect()
}

pub fn parse_fixture(format: SourceFormat, body: &str) -> Result<Vec<ImportedEvent>> {
    parse_fixture_with_limits(format, body, &ImportForensicLimits::default())
}

pub fn parse_fixture_with_limits(
    format: SourceFormat,
    body: &str,
    limits: &ImportForensicLimits,
) -> Result<Vec<ImportedEvent>> {
    read_fixture(format, body, limits)
        .map(|read| read.events)
        .map_err(ReadFailure::into_error)
}

/// Private failure class of one source, so coverage never reads error text: a
/// version outside the closed format table is unsupported; a recognized record
/// that breaks its reader contract is invalid.
enum ReadFailure {
    Unsupported(Error),
    Invalid(Error),
}

impl ReadFailure {
    fn into_error(self) -> Error {
        match self {
            Self::Unsupported(error) | Self::Invalid(error) => error,
        }
    }
}

impl From<Error> for ReadFailure {
    fn from(error: Error) -> Self {
        Self::Invalid(error)
    }
}

/// One wholly parsed source. A Native trace also yields its validated header and
/// the original sequence of each event, in event order.
struct ReadSource {
    events: Vec<ImportedEvent>,
    native: Option<(NativeTraceHeader, Vec<u64>)>,
}

/// The one reader entry of both the detected and the pinned path.
fn read_fixture(
    format: SourceFormat,
    body: &str,
    limits: &ImportForensicLimits,
) -> std::result::Result<ReadSource, ReadFailure> {
    if body.trim().is_empty() {
        return Err(Error::Invalid("zero records is not empty history success".into()).into());
    }
    if body.contains('\0') {
        return Err(Error::Invalid("parse_error".into()).into());
    }
    validate_strict_json_input(format, body)?;
    validate_reader_shape(format, body)?;

    Ok(match format {
        SourceFormat::RsihPiFixture => ReadSource {
            events: parse_rsih_pi(body, limits)?,
            native: None,
        },
        SourceFormat::RsiaTraceV1 => {
            let (events, header, sequences) = parse_rsia_trace(body)?;
            ReadSource {
                events,
                native: Some((header, sequences)),
            }
        }
        SourceFormat::ClaudeFixture => ReadSource {
            events: parse_claude_fixture(body, limits)?,
            native: None,
        },
    })
}

fn validate_reader_shape(format: SourceFormat, body: &str) -> std::result::Result<(), ReadFailure> {
    let full = serde_json::from_str::<Value>(body).ok();
    match format {
        SourceFormat::RsihPiFixture => {
            let root = full.ok_or_else(|| Error::Invalid("parse_error".into()))?;
            validate_value_discriminator(&root, format)?;
            validate_optional_string(&root, "schema_version")?;
            validate_optional_string(&root, "format")?;
            if let Some(prompts) = root.get("prompts") {
                let prompts = prompts
                    .as_array()
                    .ok_or_else(|| Error::Invalid("prompts must be an array".into()))?;
                for prompt in prompts {
                    validate_event_shape(prompt, &["role", "text", "content"])?;
                }
            } else if let Some(items) = root.as_array() {
                for item in items {
                    validate_event_shape(item, &["role", "text", "content"])?;
                }
            } else {
                validate_event_shape(&root, &["role", "text", "content"])?;
            }
        }
        SourceFormat::RsiaTraceV1 => {
            if let Some(root) = full {
                validate_value_discriminator(&root, format)?;
                validate_optional_string(&root, "schema_version")?;
                validate_optional_string(&root, "format")?;
                if let Some(events) = root.get("events") {
                    let events = events
                        .as_array()
                        .ok_or_else(|| Error::Invalid("events must be an array".into()))?;
                    for event in events {
                        validate_event_shape(event, &["role", "kind", "content"])?;
                    }
                } else {
                    validate_event_shape(&root, &["role", "kind", "content"])?;
                }
            } else {
                validate_jsonl_shapes(
                    body,
                    &["schema_version", "format", "role", "kind", "content"],
                    format,
                )?;
            }
        }
        SourceFormat::ClaudeFixture => {
            validate_jsonl_shapes(
                body,
                &["schema_version", "format", "role", "type", "content"],
                format,
            )?;
        }
    }
    Ok(())
}

fn validate_jsonl_shapes(
    body: &str,
    fields: &[&str],
    format: SourceFormat,
) -> std::result::Result<(), ReadFailure> {
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value =
            serde_json::from_str(line.trim()).map_err(|_| Error::Invalid("parse_error".into()))?;
        validate_value_discriminator(&value, format)?;
        validate_event_shape(&value, fields)?;
    }
    Ok(())
}

fn validate_value_discriminator(
    value: &Value,
    expected: SourceFormat,
) -> std::result::Result<(), ReadFailure> {
    for field in ["schema_version", "format"] {
        if let Some(discriminator) = validate_optional_string(value, field)? {
            match format_from_discriminator(discriminator) {
                Err(error) => return Err(ReadFailure::Unsupported(error)),
                Ok(found) if found != expected => {
                    return Err(Error::Conflict("format discriminator mismatch".into()).into());
                }
                Ok(_) => {}
            }
        }
    }
    Ok(())
}

fn validate_event_shape(value: &Value, string_fields: &[&str]) -> Result<()> {
    if !value.is_object() {
        return Err(Error::Invalid("event must be an object".into()));
    }
    for field in string_fields {
        validate_optional_string(value, field)?;
    }
    Ok(())
}

fn validate_optional_string<'a>(value: &'a Value, field: &str) -> Result<Option<&'a str>> {
    match value.get(field) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(Error::Invalid(format!("{field} must be a string"))),
    }
}

fn validate_strict_json_input(format: SourceFormat, body: &str) -> Result<()> {
    if strict_json_document(body.as_bytes()).is_ok() {
        return Ok(());
    }
    if format == SourceFormat::RsihPiFixture {
        return Err(Error::Invalid("parse_error".into()));
    }
    let mut records = 0usize;
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        strict_json_document(line.trim().as_bytes())?;
        records += 1;
    }
    if records == 0 {
        return Err(Error::Invalid("parse_error".into()));
    }
    Ok(())
}

fn parse_rsih_pi(body: &str, _limits: &ImportForensicLimits) -> Result<Vec<ImportedEvent>> {
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|_| Error::Invalid("parse_error".into()))?;

    if let Some(schema_ver) = parsed
        .get("schema_version")
        .and_then(|v| v.as_str())
        .filter(|&ver| ver != "rsih.pi.fixture")
    {
        return Err(Error::Invalid(format!(
            "unknown schema_version: {}",
            schema_ver
        )));
    }
    if let Some(fmt) = parsed
        .get("format")
        .and_then(|v| v.as_str())
        .filter(|&f| f != "rsih.pi.fixture")
    {
        return Err(Error::Invalid(format!("unknown format: {}", fmt)));
    }

    let mut events = Vec::new();
    if let Some(arr) = parsed.get("prompts").and_then(|v| v.as_array()) {
        let spans = find_json_array_element_spans(body, "prompts");
        for (idx, item) in arr.iter().enumerate() {
            let (byte_start, byte_end) = spans
                .get(idx)
                .copied()
                .ok_or_else(|| Error::Invalid("event_locator_mismatch".into()))?;
            let role = item
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let content_raw = item
                .get("text")
                .or_else(|| item.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            let kind = if content_raw.contains("tool_result") {
                "tool_result".to_string()
            } else {
                "chat".to_string()
            };

            let content = content_raw.to_string();

            events.push(ImportedEvent {
                source_id: String::new(),
                origin_name: "rsih_pi".into(),
                role,
                format: SourceFormat::RsihPiFixture,
                task_origin: TaskOrigin::ImportedHistory,
                attestation: ExecutionAttestation::UnverifiedImport,
                kind,
                content,
                event_index: idx,
                byte_start,
                byte_end,
                raw_fragment_digest: String::new(),
            });
        }
    } else if let Some(arr) = parsed.as_array() {
        let spans = find_top_level_json_array_spans(body);
        for (idx, item) in arr.iter().enumerate() {
            let (byte_start, byte_end) = spans
                .get(idx)
                .copied()
                .ok_or_else(|| Error::Invalid("event_locator_mismatch".into()))?;
            let role = item
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let content_raw = item
                .get("text")
                .or_else(|| item.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            let kind = if content_raw.contains("tool_result") {
                "tool_result".to_string()
            } else {
                "chat".to_string()
            };

            let content = content_raw.to_string();

            events.push(ImportedEvent {
                source_id: String::new(),
                origin_name: "rsih_pi".into(),
                role,
                format: SourceFormat::RsihPiFixture,
                task_origin: TaskOrigin::ImportedHistory,
                attestation: ExecutionAttestation::UnverifiedImport,
                kind,
                content,
                event_index: idx,
                byte_start,
                byte_end,
                raw_fragment_digest: String::new(),
            });
        }
    } else {
        let role = parsed
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("user")
            .to_string();
        let content_raw = parsed
            .get("text")
            .or_else(|| parsed.get("content"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        let kind = if content_raw.contains("tool_result") {
            "tool_result".to_string()
        } else {
            "chat".to_string()
        };

        events.push(ImportedEvent {
            source_id: String::new(),
            origin_name: "rsih_pi".into(),
            role,
            format: SourceFormat::RsihPiFixture,
            task_origin: TaskOrigin::ImportedHistory,
            attestation: ExecutionAttestation::UnverifiedImport,
            kind,
            content: content_raw.to_string(),
            event_index: 0,
            byte_start: 0,
            byte_end: body.len(),
            raw_fragment_digest: String::new(),
        });
    }

    if events.is_empty() {
        return Err(Error::Invalid(
            "zero records is not empty history success".into(),
        ));
    }
    Ok(events)
}

/// Strict `rsia.trace.v1` reader (§11.2.1, V117.b), the same for the detected and
/// the pinned path. Every record carries the complete header itself (nothing is
/// inherited from another record) and every event an explicit sequence, role,
/// kind and content; nothing defaults to `user`. Unknown non-authority fields
/// are ignored. The header stays the exporter's unverified declaration.
fn parse_rsia_trace(body: &str) -> Result<(Vec<ImportedEvent>, NativeTraceHeader, Vec<u64>)> {
    let mut read = NativeTraceRead::default();
    if let Ok(root) = serde_json::from_str::<Value>(body) {
        read.header(&root)?;
        if let Some(items) = root.get("events").and_then(Value::as_array) {
            let spans = find_json_array_element_spans(body, "events");
            for (index, item) in items.iter().enumerate() {
                let (byte_start, byte_end) = spans
                    .get(index)
                    .copied()
                    .ok_or_else(|| Error::Invalid("event_locator_mismatch".into()))?;
                read.event(item, index, byte_start, byte_end)?;
            }
        } else {
            read.event(&root, 0, 0, body.len())?;
        }
        return read.finish();
    }

    // Line-delimited JSONL: each nonempty line is one complete record.
    let mut offset = 0;
    for (index, line) in body.split_inclusive('\n').enumerate() {
        let trimmed_end = line.trim_end_matches(&['\r', '\n'][..]);
        let ws_offset = trimmed_end.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        let trimmed = trimmed_end.trim();
        if trimmed.is_empty() {
            offset += line.len();
            continue;
        }
        let byte_start = offset + ws_offset;
        let byte_end = byte_start + trimmed.len();
        offset += line.len();
        let record: Value =
            serde_json::from_str(trimmed).map_err(|_| Error::Invalid("parse_error".into()))?;
        read.header(&record)?;
        read.event(&record, index, byte_start, byte_end)?;
    }
    read.finish()
}

#[derive(Default)]
struct NativeTraceRead {
    header: Option<NativeTraceHeader>,
    events: Vec<ImportedEvent>,
    sequences: Vec<u64>,
}

impl NativeTraceRead {
    fn header(&mut self, record: &Value) -> Result<()> {
        if record.get("schema_version").is_none() {
            return Err(Error::Invalid(
                "missing schema_version for rsia.trace".into(),
            ));
        }
        let header = NativeTraceHeader {
            exporter: native_field(record, "exporter")?,
            run_id: native_field(record, "run_id")?,
            redaction_report: native_field(record, "redaction_report")?,
            coverage: native_field(record, "coverage")?,
        };
        let exporter = &header.exporter;
        if exporter.name != "rsia"
            || identifier(&exporter.version).is_err()
            || exporter.commit.len() != 40
            || !exporter
                .commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::Invalid("invalid exporter for rsia.trace".into()));
        }
        if identifier(&header.run_id).is_err() {
            return Err(Error::Invalid("invalid run_id for rsia.trace".into()));
        }
        let report = &header.redaction_report;
        if report.schema_version != "rsia.trace.redaction.v1"
            || report.scan_status != "passed"
            || report.review_status != "reviewed"
        {
            return Err(Error::Invalid(
                "invalid redaction_report for rsia.trace".into(),
            ));
        }
        match &self.header {
            Some(first) if *first != header => {
                Err(Error::Invalid("header mismatch for rsia.trace".into()))
            }
            Some(_) => Ok(()),
            None => {
                self.header = Some(header);
                Ok(())
            }
        }
    }

    // Field types were checked by the shape validator: an absent string field
    // is missing, never a default.
    fn event(
        &mut self,
        item: &Value,
        event_index: usize,
        byte_start: usize,
        byte_end: usize,
    ) -> Result<()> {
        let sequence = item
            .get("sequence")
            .ok_or_else(|| Error::Invalid("missing sequence for rsia.trace".into()))?
            .as_u64()
            .ok_or_else(|| Error::Invalid("sequence must be an unsigned integer".into()))?;
        if self.sequences.last().is_some_and(|last| *last >= sequence) {
            return Err(Error::Invalid(
                "sequence must strictly increase for rsia.trace".into(),
            ));
        }
        let role = native_label(item, "role")?;
        let kind = native_label(item, "kind")?;
        let content = item
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Invalid("missing content for rsia.trace".into()))?
            .to_string();
        self.sequences.push(sequence);
        self.events.push(ImportedEvent {
            source_id: String::new(),
            origin_name: "rsia_trace".into(),
            role,
            format: SourceFormat::RsiaTraceV1,
            task_origin: TaskOrigin::ImportedHistory,
            attestation: ExecutionAttestation::UnverifiedImport,
            kind,
            content,
            event_index,
            byte_start,
            byte_end,
            raw_fragment_digest: String::new(),
        });
        Ok(())
    }

    fn finish(self) -> Result<(Vec<ImportedEvent>, NativeTraceHeader, Vec<u64>)> {
        let header = self
            .header
            .filter(|_| !self.events.is_empty())
            .ok_or_else(|| Error::Invalid("zero records is not empty history success".into()))?;
        header
            .coverage
            .validate_declared(self.events.len() as u64)?;
        Ok((self.events, header, self.sequences))
    }
}

fn native_field<T: DeserializeOwned>(record: &Value, field: &str) -> Result<T> {
    let value = record
        .get(field)
        .ok_or_else(|| Error::Invalid(format!("missing {field} for rsia.trace")))?;
    serde_json::from_value(value.clone())
        .map_err(|_| Error::Invalid(format!("invalid {field} for rsia.trace")))
}

/// Role and kind keep their original text and case; they are never empty and
/// never carry NUL.
fn native_label(item: &Value, field: &str) -> Result<String> {
    match item.get(field).and_then(Value::as_str) {
        None => Err(Error::Invalid(format!("missing {field} for rsia.trace"))),
        Some(label) if label.is_empty() || label.contains('\0') => {
            Err(Error::Invalid(format!("invalid {field} for rsia.trace")))
        }
        Some(label) => Ok(label.to_string()),
    }
}

fn parse_claude_fixture(body: &str, _limits: &ImportForensicLimits) -> Result<Vec<ImportedEvent>> {
    let mut events = Vec::new();
    let mut offset = 0;

    for (idx, line) in body.split_inclusive('\n').enumerate() {
        let trimmed_end = line.trim_end_matches(&['\r', '\n'][..]);
        let ws_offset = trimmed_end.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        let trimmed = trimmed_end.trim();
        if trimmed.is_empty() {
            offset += line.len();
            continue;
        }
        let byte_start = offset + ws_offset;
        let byte_end = byte_start + trimmed.len();
        offset += line.len();

        let item: serde_json::Value =
            serde_json::from_str(trimmed).map_err(|_| Error::Invalid("parse_error".into()))?;

        if let Some(schema_ver) = item.get("schema_version").and_then(|v| v.as_str())
            && schema_ver != "claude.fixture"
        {
            return Err(Error::Invalid(format!(
                "unknown schema_version: {}",
                schema_ver
            )));
        }
        if let Some(fmt) = item.get("format").and_then(|v| v.as_str())
            && fmt != "claude.fixture"
        {
            return Err(Error::Invalid(format!("unknown format: {}", fmt)));
        }

        let role = item
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("user")
            .to_string();

        let typ = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let kind = if typ == "tool_result" {
            "tool_result".to_string()
        } else if typ == "command_echo" {
            "command_echo".to_string()
        } else if is_user_role(&role)
            && item
                .get("content")
                .and_then(|v| v.as_str())
                .is_some_and(|c| c.contains("tool_result"))
        {
            "tool_result".to_string()
        } else {
            "chat".to_string()
        };

        // A record without a top-level string `content` (for example the nested
        // `message` shape of live Claude Code JSONL) is not a pinned claude.fixture
        // record; it fails closed instead of becoming an empty user chat event.
        let content = match item.get("content") {
            Some(Value::String(c)) => c.clone(),
            _ => return Err(Error::Invalid("content must be a string".into())),
        };

        events.push(ImportedEvent {
            source_id: String::new(),
            origin_name: "claude".into(),
            role,
            format: SourceFormat::ClaudeFixture,
            task_origin: TaskOrigin::ImportedHistory,
            attestation: ExecutionAttestation::UnverifiedImport,
            kind,
            content,
            event_index: idx,
            byte_start,
            byte_end,
            raw_fragment_digest: String::new(),
        });
    }

    if events.is_empty() {
        return Err(Error::Invalid(
            "zero records is not empty history success".into(),
        ));
    }
    Ok(events)
}

#[derive(Debug, Clone)]
pub struct IngestResult {
    pub evidence_set: EvidenceSet,
    pub aggregate_summary: AggregateSummary,
    pub events: Vec<ImportedEvent>,
    pub locators: Vec<EvidenceLocator>,
}

struct ImportAttempt {
    evidence_set: Option<EvidenceSet>,
    aggregate_summary: AggregateSummary,
    events: Vec<ImportedEvent>,
    locators: Vec<EvidenceLocator>,
    native_traces: Vec<NativeTraceSource>,
}

impl ImportAttempt {
    fn require_evidence(self) -> Result<IngestResult> {
        Ok(IngestResult {
            evidence_set: self
                .evidence_set
                .ok_or_else(|| Error::Invalid("no valid imported sources ingested".into()))?,
            aggregate_summary: self.aggregate_summary,
            events: self.events,
            locators: self.locators,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PinnedSourceBytes<'a> {
    pub source_name: &'a str,
    pub raw_bytes: &'a [u8],
    pub reader: SourceFormat,
    pub cluster_hint: Option<&'a str>,
}

/// Ingests authorized external history sources under §6.3 resource bounds and security policies.
pub fn ingest_imported_sources(
    selection: &SourceSelection,
    sources: &[(&str, &[u8])],
    limits: Option<ImportForensicLimits>,
) -> Result<IngestResult> {
    let selected: Vec<_> = sources
        .iter()
        .map(|(source_name, raw_bytes)| PinnedSourceBytes {
            source_name,
            raw_bytes,
            reader: SourceFormat::RsiaTraceV1,
            cluster_hint: Some(source_name),
        })
        .collect();
    ingest_selected_sources(selection, &selected, limits, false)?.require_evidence()
}

pub fn ingest_pinned_imported_sources(
    selection: &SourceSelection,
    sources: &[PinnedSourceBytes<'_>],
    limits: Option<ImportForensicLimits>,
) -> Result<IngestResult> {
    ingest_selected_sources(selection, sources, limits, true)?.require_evidence()
}

fn ingest_selected_sources(
    selection: &SourceSelection,
    sources: &[PinnedSourceBytes<'_>],
    limits: Option<ImportForensicLimits>,
    readers_are_pinned: bool,
) -> Result<ImportAttempt> {
    selection.validate()?;
    let limits = limits.unwrap_or_default();
    limits.validate()?;

    if sources.is_empty() {
        return Err(Error::Invalid("source selection is empty".into()));
    }

    // Path authorization. In-memory labels (valid identifiers, never opened on disk)
    // are accepted as-is; anything else is a path and must be an absolute, normalised
    // path strictly inside one of the validated roots (never "/" or a home directory,
    // which `selection.validate()` already refused as roots).
    for source in sources {
        if identifier(source.source_name).is_err() {
            assert_authorized_path(source.source_name, &selection.roots)?;
        }
    }

    let mut coverage = SourceCoverage {
        discovery_exhausted: true,
        files_known: true,
        ..SourceCoverage::default()
    };

    let count_to_process = if sources.len() > limits.max_files {
        coverage.discovery_exhausted = false;
        coverage.files_known = false;
        limits.max_files
    } else {
        sources.len()
    };

    let mut members = Vec::new();
    let mut all_events = Vec::new();
    let mut all_locators = Vec::new();
    let mut cluster_inputs = Vec::new();
    let mut counter_examples = Vec::new();
    let mut tool_calls_count = 0;
    let mut failure_count = 0;
    let mut seen_sources = BTreeSet::new();
    let mut excerpt_bytes_used = 0usize;
    let mut native_traces = Vec::new();

    for source in &sources[..count_to_process] {
        let source_name = source.source_name;
        let raw_bytes = source.raw_bytes;
        let member_id = if identifier(source_name).is_ok() {
            source_name.to_string()
        } else {
            // Path with slashes: already passed assert_authorized_path above; derive stable identifier
            format!("imp_{}", &evo_core::hash(source_name.as_bytes())[..16])
        };

        if !seen_sources.insert(member_id.clone()) {
            return Err(Error::Conflict("duplicate source record".into()));
        }

        let remaining_read = limits
            .max_total_read_bytes
            .saturating_sub(usize::try_from(coverage.bytes_read).unwrap_or(usize::MAX));
        if raw_bytes.len() > remaining_read {
            coverage.bytes_read += remaining_read as u64;
            coverage.event_truncated += 1;
            coverage.outbound_truncated += 1;
            break;
        }
        coverage.bytes_read += raw_bytes.len() as u64;

        if raw_bytes.is_empty() || raw_bytes.iter().all(|b| b.is_ascii_whitespace()) {
            coverage.zero_records += 1;
            continue;
        }

        let format = match if readers_are_pinned {
            Ok(source.reader)
        } else {
            detect_format_from_bytes_with_limit(raw_bytes, limits.max_header_probe_bytes)
        } {
            Ok(fmt) => fmt,
            Err(error) => {
                if error.to_string().contains("parse")
                    || error.to_string().contains("duplicate_json_key")
                    || error.to_string().contains("invalid_utf8")
                {
                    coverage.parsed_fail += 1;
                } else {
                    coverage.unsupported_format += 1;
                }
                continue;
            }
        };

        let body = match std::str::from_utf8(raw_bytes) {
            Ok(b) => b,
            Err(_) => {
                coverage.parsed_fail += 1;
                continue;
            }
        };

        // A source contributes whole or not at all. Pi/Claude keep their original
        // parse-failure count; a Native version outside the table is unsupported.
        let read = match read_fixture(format, body, &limits) {
            Ok(read) => read,
            Err(ReadFailure::Unsupported(_)) if format == SourceFormat::RsiaTraceV1 => {
                coverage.unsupported_format += 1;
                continue;
            }
            Err(_) => {
                coverage.parsed_fail += 1;
                continue;
            }
        };
        let parsed_events = read.events;

        if parsed_events.is_empty() {
            coverage.zero_records += 1;
            continue;
        }

        coverage.parsed_ok += 1;
        if let Some((header, sequences)) = read.native {
            native_traces.push(NativeTraceSource {
                source_id: member_id.clone(),
                header,
                event_sequences: parsed_events
                    .iter()
                    .map(|event| event.event_index)
                    .zip(sequences)
                    .collect(),
            });
        }

        let content_digest = evo_core::hash(raw_bytes);

        for mut ev in parsed_events {
            // Unconditionally separate 3 trust axes (V005)
            ev.source_id = member_id.clone();
            ev.task_origin = TaskOrigin::ImportedHistory;
            ev.attestation = ExecutionAttestation::UnverifiedImport;
            let raw_fragment = raw_bytes
                .get(ev.byte_start..ev.byte_end)
                .ok_or_else(|| Error::Invalid("event_locator_mismatch".into()))?;
            ev.raw_fragment_digest = hash(raw_fragment);

            if ev.content.len() > limits.max_event_bytes
                || ev.byte_end.saturating_sub(ev.byte_start) > limits.max_event_bytes
            {
                coverage.char_truncated += 1;
                coverage.event_truncated += 1;
                let mut trunc_len = ev.content.len().min(limits.max_event_bytes);
                while !ev.content.is_char_boundary(trunc_len) {
                    trunc_len -= 1;
                }
                ev.content.truncate(trunc_len);
            }

            if ev.kind == "tool_result"
                || ev.content.contains("tool_result")
                || ev.content.contains("command_echo")
            {
                tool_calls_count += 1;
            }

            if ev.content.contains("fail")
                || ev.content.contains("error")
                || ev.content.contains("Error")
            {
                failure_count += 1;
            }

            if ev.content.contains("counter_example")
                || ev.content.contains("regression")
                || ev.content.contains("violation")
            {
                counter_examples.push(format!("{}:{}", member_id, ev.event_index));
            }

            if selection.allow_model_excerpts
                && all_locators.len() < limits.max_excerpts
                && ev.byte_end <= raw_bytes.len()
                && ev.byte_start < ev.byte_end
            {
                let remaining_excerpt = limits
                    .max_total_excerpt_bytes
                    .saturating_sub(excerpt_bytes_used);
                let max_len = (ev.byte_end - ev.byte_start).min(remaining_excerpt);
                if max_len == 0 {
                    coverage.excerpt_truncated += 1;
                    all_events.push(ev);
                    continue;
                }
                let excerpt_start = ev.byte_start;
                let mut excerpt_end = ev.byte_start + max_len;
                while excerpt_end > excerpt_start && !body.is_char_boundary(excerpt_end) {
                    excerpt_end -= 1;
                }
                let excerpt = &raw_bytes[excerpt_start..excerpt_end];
                let excerpt_digest = evo_core::hash(excerpt);
                let loc = EvidenceLocator::build(
                    &member_id,
                    &content_digest,
                    excerpt_start,
                    excerpt_end,
                    ev.event_index,
                    &excerpt_digest,
                )?;
                evo_core::evidence::verify_evidence_locator_content_v3(&loc, raw_bytes)?;
                all_locators.push(loc);
                excerpt_bytes_used += excerpt.len();
                if excerpt_end < ev.byte_end {
                    coverage.excerpt_truncated += 1;
                }
            } else if selection.allow_model_excerpts
                && ev.byte_end <= raw_bytes.len()
                && ev.byte_start < ev.byte_end
            {
                coverage.excerpt_truncated += 1;
            }

            all_events.push(ev);
        }

        cluster_inputs.push((
            source.cluster_hint.unwrap_or(source_name).to_string(),
            content_digest.clone(),
        ));

        let member = EvidenceMember {
            source_id: member_id,
            content_digest,
            task_origin: TaskOrigin::ImportedHistory,
            execution_attestation: ExecutionAttestation::UnverifiedImport,
            purpose: selection.purpose,
        };
        evo_core::evidence::verify_evidence_member_content_v3(&member, raw_bytes)?;
        members.push(member);
    }

    let clusters = independent_clusters(&cluster_inputs);
    let summary = AggregateSummary {
        total_sources: members.len(),
        total_events: all_events.len(),
        tool_calls_count,
        failure_count,
        unique_clusters: clusters.len(),
        counter_examples,
        coverage: coverage.clone(),
    };

    let evidence_set = if members.is_empty() {
        None
    } else {
        let raw_set_id = format!(
            "es_imp_{}",
            &evo_core::hash(format!("{:?}", selection).as_bytes())[..12]
        );
        Some(EvidenceSet::build(raw_set_id, members, coverage, clusters)?)
    };

    Ok(ImportAttempt {
        evidence_set,
        aggregate_summary: summary,
        events: all_events,
        locators: all_locators,
        native_traces,
    })
}

pub const IMPORT_REGISTRATION_SCHEMA: &str = "rsia.e16.import_registration.v1";
pub const SOURCE_SELECTION_SCHEMA: &str = "rsia.e16.source_selection.v1";
pub const IMPORT_SOURCE_SCHEMA: &str = "rsia.e16.import_source.v1";
/// The current result schema; v1 results stay readable as legacy history.
pub const IMPORT_RESULT_SCHEMA: &str = IMPORT_RESULT_SCHEMA_V2;
const IMPORT_ARTIFACT_KIND: &str = "artifact";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportRetentionScope {
    LocalPrivate,
    LocalWithAuthorizedExcerpts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSourceSpec {
    pub source_id: String,
    pub path: String,
    pub reader: SourceFormat,
    pub expected_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRegistrationRequest {
    pub schema_version: String,
    pub request_key: String,
    pub roots: Vec<String>,
    pub purpose: evo_core::evidence::Purpose,
    pub allow_model_excerpts: bool,
    pub outbound_authorized: bool,
    pub retention_scope: ImportRetentionScope,
    pub sources: Vec<ImportSourceSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportTypedRef {
    pub kind: String,
    pub id: String,
    pub content_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportEnvelope<T> {
    pub schema_version: String,
    pub id: String,
    pub namespace: String,
    pub owner_actor: String,
    pub request_key: String,
    pub input_digest: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub source_refs: Vec<ImportTypedRef>,
    pub revoke_watermark: u64,
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSelectionPayload {
    pub purpose: evo_core::evidence::Purpose,
    pub allow_model_excerpts: bool,
    pub outbound_authorized: bool,
    pub retention_scope: ImportRetentionScope,
    pub roots: Vec<String>,
    pub import_source_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSourceReadStatus {
    Prepared,
    Ready,
    Missing,
    PermissionDenied,
    SourceChanged,
    ZeroRecords,
    InvalidFile,
    TooLarge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSourcePayload {
    pub selection_id: String,
    pub logical_source_id: String,
    pub authorized_path: String,
    pub reader: SourceFormat,
    pub reader_version: String,
    pub raw_digest: String,
    pub observed_digest: Option<String>,
    pub raw_blob_digest: String,
    pub blob_published: bool,
    pub byte_len: u64,
    pub status: ImportSourceReadStatus,
    pub task_origin: TaskOrigin,
    pub execution_attestation: ExecutionAttestation,
    pub purpose: evo_core::evidence::Purpose,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedImportEvent {
    pub source_id: String,
    pub event_index: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    pub raw_fragment_digest: String,
    pub normalized_content_digest: String,
    pub role: String,
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportResultState {
    Complete,
    Partial,
    Failed,
}

/// Current (v2) result: the v1 fields plus the reader and normalization
/// versions it was derived with, this import's own statistics in the complete
/// coverage carrier, and each Native source's declared header and original
/// event sequences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportResultPayload {
    pub selection_id: String,
    pub reader_version: String,
    pub normalization_version: String,
    pub state: ImportResultState,
    pub evidence_set: Option<EvidenceSet>,
    pub aggregate_summary: AggregateSummary,
    pub import_coverage: TraceCoverage,
    pub locators: Vec<EvidenceLocator>,
    pub event_records: Vec<PersistedImportEvent>,
    pub native_traces: Vec<NativeTraceSource>,
    pub generation_status: String,
}

/// Historical v1 result, field for field as PR44 wrote it. Statically readable
/// and cleanable; never re-derived and never an input of a new derivation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyImportResultPayloadV1 {
    pub selection_id: String,
    pub state: ImportResultState,
    pub evidence_set: Option<EvidenceSet>,
    pub aggregate_summary: AggregateSummary,
    pub locators: Vec<EvidenceLocator>,
    pub event_records: Vec<PersistedImportEvent>,
    pub generation_status: String,
}

pub type SourceSelectionRecord = ImportEnvelope<SourceSelectionPayload>;
pub type ImportSourceRecord = ImportEnvelope<ImportSourcePayload>;
pub type ImportResultRecord = ImportEnvelope<ImportResultPayload>;
pub type LegacyImportResultRecordV1 = ImportEnvelope<LegacyImportResultPayloadV1>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportResultReadState {
    CurrentVerified,
    Legacy,
}

/// Version-aware read projection of one stored result (§5.1.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportResultReadView {
    pub schema_version: String,
    pub state: ImportResultReadState,
    pub selection_id: String,
    /// Both lineage identities derive from the one selection.
    pub legacy_v1_id: String,
    pub current_v2_id: String,
    /// The stored record text, byte for byte.
    pub record: String,
}

/// A stored result after its static checks, by version. Static checks alone
/// never make a record current. A short-lived value of one read, so the larger
/// current variant is not boxed.
#[allow(clippy::large_enum_variant)]
pub(crate) enum StoredImportResult {
    V2(ImportResultRecord),
    LegacyV1(LegacyImportResultRecordV1),
}

/// What both stored result versions keep alike; every static read checks this.
pub(crate) struct ResultCore<'a> {
    pub(crate) id: &'a str,
    pub(crate) request_key: &'a str,
    pub(crate) input_digest: &'a str,
    pub(crate) source_refs: &'a [ImportTypedRef],
    pub(crate) revoke_watermark: u64,
    pub(crate) selection_id: &'a str,
    pub(crate) state: ImportResultState,
    pub(crate) evidence_set: Option<&'a EvidenceSet>,
    pub(crate) total_events: usize,
    pub(crate) locators: &'a [EvidenceLocator],
    pub(crate) event_records: &'a [PersistedImportEvent],
    pub(crate) generation_status: &'a str,
}

impl StoredImportResult {
    /// Static version table dispatch (§5.1.1): a schema outside the table is
    /// unsupported, never legacy or current; a known version that does not
    /// decode is invalid.
    fn decode(body: &str) -> Result<Self> {
        let invalid = || Error::Conflict("import_result_invalid".into());
        let value: Value = serde_json::from_str(body).map_err(|_| invalid())?;
        let schema = value.get("schema_version").and_then(Value::as_str);
        let version = IMPORT_RESULT_VERSIONS
            .iter()
            .find(|row| Some(row.schema_version) == schema)
            .ok_or_else(|| Error::Invalid("import_result_version_unsupported".into()))?;
        if version.retains_judge {
            serde_json::from_value(value).map(Self::V2)
        } else {
            serde_json::from_value(value).map(Self::LegacyV1)
        }
        .map_err(|_| invalid())
    }

    pub(crate) fn core(&self) -> ResultCore<'_> {
        match self {
            Self::V2(record) => ResultCore {
                id: &record.id,
                request_key: &record.request_key,
                input_digest: &record.input_digest,
                source_refs: &record.source_refs,
                revoke_watermark: record.revoke_watermark,
                selection_id: &record.payload.selection_id,
                state: record.payload.state,
                evidence_set: record.payload.evidence_set.as_ref(),
                total_events: record.payload.aggregate_summary.total_events,
                locators: &record.payload.locators,
                event_records: &record.payload.event_records,
                generation_status: &record.payload.generation_status,
            },
            Self::LegacyV1(record) => ResultCore {
                id: &record.id,
                request_key: &record.request_key,
                input_digest: &record.input_digest,
                source_refs: &record.source_refs,
                revoke_watermark: record.revoke_watermark,
                selection_id: &record.payload.selection_id,
                state: record.payload.state,
                evidence_set: record.payload.evidence_set.as_ref(),
                total_events: record.payload.aggregate_summary.total_events,
                locators: &record.payload.locators,
                event_records: &record.payload.event_records,
                generation_status: &record.payload.generation_status,
            },
        }
    }
}

/// One statically checked result with the stored text and live closure it was
/// checked against.
struct StaticImportResult {
    stored: StoredImportResult,
    body: String,
    selection: SourceSelectionRecord,
    sources: Vec<ImportSourceRecord>,
}

#[derive(Clone)]
pub struct PersistentImportService {
    store: Store,
    operation_lock: Arc<Mutex<()>>,
}

/// One server-derived management blob source: the strict management wire
/// supplies only the logical id and reader; the digest selects the
/// namespace-scoped registered blob whose exact bytes were read outside the
/// job transaction.
struct ManagementBlobSource {
    source_id: String,
    reader: SourceFormat,
    input: VerifiedBlobInput,
}

/// A validated registration plan built outside the job transaction. The
/// internal v1 request's roots and paths are the server's own blob location,
/// never request data.
pub(crate) struct PreparedManagementRegistration {
    request: ImportRegistrationRequest,
    sources: Vec<ManagementBlobSource>,
}

/// A validated execution plan: the live selection, its exact sources and the
/// v2 result derived from the pinned bytes (or the already stored result).
pub(crate) struct PreparedManagementExecution {
    selection: SourceSelectionRecord,
    sources: Vec<ImportSourceRecord>,
    result: ImportResultRecord,
}

impl PersistentImportService {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            operation_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn register(
        &self,
        ctx: &Context,
        request: ImportRegistrationRequest,
    ) -> Result<SourceSelectionRecord> {
        ctx.require(&[Role::Admin])?;
        validate_registration_request(&request)?;
        let _guard = self.operation_lock.lock().await;
        let selection_id = format!("e16sel-{}", &fingerprint(&request)?[..24]);
        let mut session = self.store.session().await?;
        if let Some(cached) = session
            .cached::<SourceSelectionRecord, _>(
                ctx,
                "e16.import.register",
                &request.request_key,
                &request,
            )
            .await?
        {
            validate_selection_record(ctx, &cached)?;
            session.commit().await?;
            let (live, _) = self.load_live_selection(ctx, &cached.id).await?;
            return Ok(live);
        }
        let live_watermark = current_watermark(&mut session, ctx).await?;
        let request_digest = fingerprint(&request)?;
        let existing: Option<SourceSelectionRecord> = session
            .get(ctx, IMPORT_ARTIFACT_KIND, &selection_id)
            .await?;
        let selection = if let Some(existing) = existing {
            if existing.input_digest != request_digest
                || existing.owner_actor != ctx.actor()
                || existing.revoke_watermark != live_watermark
            {
                return Err(Error::Conflict(
                    "prepared selection differs or is stale".into(),
                ));
            }
            existing
        } else {
            let timestamp = now()?;
            let mut prepared_sources = Vec::with_capacity(request.sources.len());
            for source in &request.sources {
                let id = import_source_id(&selection_id, &source.source_id);
                prepared_sources.push(ImportEnvelope {
                    schema_version: IMPORT_SOURCE_SCHEMA.into(),
                    id,
                    namespace: ctx.namespace().into(),
                    owner_actor: ctx.actor().into(),
                    request_key: request.request_key.clone(),
                    input_digest: fingerprint(&(source, source.expected_digest.as_str()))?,
                    created_at: timestamp,
                    updated_at: timestamp,
                    source_refs: vec![ImportTypedRef {
                        kind: "blob".into(),
                        id: source.expected_digest.clone(),
                        content_digest: source.expected_digest.clone(),
                    }],
                    revoke_watermark: live_watermark,
                    payload: ImportSourcePayload {
                        selection_id: selection_id.clone(),
                        logical_source_id: source.source_id.clone(),
                        authorized_path: source.path.clone(),
                        reader: source.reader,
                        reader_version: source.reader.as_label().into(),
                        raw_digest: source.expected_digest.clone(),
                        observed_digest: None,
                        raw_blob_digest: source.expected_digest.clone(),
                        blob_published: false,
                        byte_len: 0,
                        status: ImportSourceReadStatus::Prepared,
                        task_origin: TaskOrigin::ImportedHistory,
                        execution_attestation: ExecutionAttestation::UnverifiedImport,
                        purpose: request.purpose,
                    },
                });
            }
            let source_refs = source_record_refs(&prepared_sources)?;
            let selection = ImportEnvelope {
                schema_version: SOURCE_SELECTION_SCHEMA.into(),
                id: selection_id.clone(),
                namespace: ctx.namespace().into(),
                owner_actor: ctx.actor().into(),
                request_key: request.request_key.clone(),
                input_digest: request_digest.clone(),
                created_at: timestamp,
                updated_at: timestamp,
                source_refs,
                revoke_watermark: live_watermark,
                payload: SourceSelectionPayload {
                    purpose: request.purpose,
                    allow_model_excerpts: request.allow_model_excerpts,
                    outbound_authorized: request.outbound_authorized,
                    retention_scope: request.retention_scope,
                    roots: request.roots.clone(),
                    import_source_ids: prepared_sources
                        .iter()
                        .map(|record| record.id.clone())
                        .collect(),
                },
            };
            // Every id is checked before the first row is written, so a refusal
            // leaves nothing behind (not even a half-prepared selection).
            session.ensure_revocation_namespace_live(ctx).await?;
            for record in &prepared_sources {
                ensure_import_source_id_is_free(&mut session, ctx, &record.id).await?;
            }
            for record in &prepared_sources {
                put_new_artifact(&mut session, ctx, record).await?;
                session
                    .put_edge(
                        ctx,
                        IMPORT_ARTIFACT_KIND,
                        &record.id,
                        "blob",
                        &record.payload.raw_blob_digest,
                    )
                    .await?;
            }
            put_new_artifact(&mut session, ctx, &selection).await?;
            for record in &prepared_sources {
                session
                    .put_edge(
                        ctx,
                        IMPORT_ARTIFACT_KIND,
                        &selection.id,
                        IMPORT_ARTIFACT_KIND,
                        &record.id,
                    )
                    .await?;
            }
            session
                .audit(ctx, "e16.import.prepare", &selection.id)
                .await?;
            selection
        };
        session.commit().await?;

        let source_records = self
            .finalize_prepared_sources(ctx, &selection, &request)
            .await?;
        let mut finalized_selection = selection;
        finalized_selection.source_refs = source_record_refs(&source_records)?;
        finalized_selection.updated_at = now()?;
        let mut session = self.store.session().await?;
        if let Some(cached) = session
            .cached::<SourceSelectionRecord, _>(
                ctx,
                "e16.import.register",
                &request.request_key,
                &request,
            )
            .await?
        {
            session.commit().await?;
            let (live, _) = self.load_live_selection(ctx, &cached.id).await?;
            return Ok(live);
        }
        if current_watermark(&mut session, ctx).await? != finalized_selection.revoke_watermark {
            return Err(Error::Conflict("source revoke watermark changed".into()));
        }
        for source in &source_records {
            if source.payload.status == ImportSourceReadStatus::Prepared {
                return Err(Error::Conflict("import source remained Prepared".into()));
            }
        }
        session
            .put(
                ctx,
                IMPORT_ARTIFACT_KIND,
                &finalized_selection.id,
                ctx.actor(),
                &finalized_selection,
            )
            .await?;
        session
            .cache(
                ctx,
                "e16.import.register",
                &request.request_key,
                &request,
                &finalized_selection.id,
                &finalized_selection,
            )
            .await?;
        session
            .audit(ctx, "e16.import.register", &finalized_selection.id)
            .await?;
        session.commit().await?;
        Ok(finalized_selection)
    }

    async fn finalize_prepared_sources(
        &self,
        ctx: &Context,
        selection: &SourceSelectionRecord,
        request: &ImportRegistrationRequest,
    ) -> Result<Vec<ImportSourceRecord>> {
        let mut records = Vec::with_capacity(selection.payload.import_source_ids.len());
        let mut consumed = 0usize;
        for source_id in &selection.payload.import_source_ids {
            let mut session = self.store.session().await?;
            let record: ImportSourceRecord =
                session.need(ctx, IMPORT_ARTIFACT_KIND, source_id).await?;
            validate_source_record(ctx, &record)?;
            session.commit().await?;
            if record.payload.status != ImportSourceReadStatus::Prepared {
                if matches!(
                    record.payload.status,
                    ImportSourceReadStatus::Ready | ImportSourceReadStatus::SourceChanged
                ) {
                    consumed = consumed
                        .checked_add(record.payload.byte_len as usize)
                        .ok_or_else(|| Error::Invalid("import byte total overflow".into()))?;
                }
                records.push(record);
                continue;
            }
            let spec = request
                .sources
                .iter()
                .find(|source| source.source_id == record.payload.logical_source_id)
                .ok_or_else(|| Error::Conflict("prepared source is absent from request".into()))?;
            let remaining = MAX_TOTAL_READ_BYTES.saturating_sub(consumed);
            // Re-check right before the filesystem is touched: only an absolute,
            // normalised path strictly inside a validated root is ever opened.
            assert_authorized_path(&spec.path, &request.roots)?;
            let metadata = match tokio::fs::symlink_metadata(&spec.path).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    records.push(
                        self.finish_prepared_source(
                            ctx,
                            &record,
                            ImportSourceReadStatus::Missing,
                            None,
                            0,
                            false,
                        )
                        .await?,
                    );
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    records.push(
                        self.finish_prepared_source(
                            ctx,
                            &record,
                            ImportSourceReadStatus::PermissionDenied,
                            None,
                            0,
                            false,
                        )
                        .await?,
                    );
                    continue;
                }
                Err(_) => return Err(Error::Internal),
            };
            if !metadata.file_type().is_file() {
                records.push(
                    self.finish_prepared_source(
                        ctx,
                        &record,
                        ImportSourceReadStatus::InvalidFile,
                        None,
                        metadata.len(),
                        false,
                    )
                    .await?,
                );
                continue;
            }
            if remaining == 0 || metadata.len() > remaining as u64 {
                records.push(
                    self.finish_prepared_source(
                        ctx,
                        &record,
                        ImportSourceReadStatus::TooLarge,
                        None,
                        metadata.len(),
                        false,
                    )
                    .await?,
                );
                continue;
            }
            let bytes = match read_file_bounded(&spec.path, remaining).await {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    records.push(
                        self.finish_prepared_source(
                            ctx,
                            &record,
                            ImportSourceReadStatus::Missing,
                            None,
                            0,
                            false,
                        )
                        .await?,
                    );
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    records.push(
                        self.finish_prepared_source(
                            ctx,
                            &record,
                            ImportSourceReadStatus::PermissionDenied,
                            None,
                            0,
                            false,
                        )
                        .await?,
                    );
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                    records.push(
                        self.finish_prepared_source(
                            ctx,
                            &record,
                            ImportSourceReadStatus::TooLarge,
                            None,
                            metadata.len(),
                            false,
                        )
                        .await?,
                    );
                    continue;
                }
                Err(_) => return Err(Error::Internal),
            };
            consumed = consumed
                .checked_add(bytes.len())
                .ok_or_else(|| Error::Invalid("import byte total overflow".into()))?;
            let observed_digest = hash(&bytes);
            if bytes.is_empty() {
                records.push(
                    self.finish_prepared_source(
                        ctx,
                        &record,
                        ImportSourceReadStatus::ZeroRecords,
                        Some(observed_digest),
                        0,
                        false,
                    )
                    .await?,
                );
                continue;
            }
            if bytes.len() as u64 != metadata.len() || observed_digest != spec.expected_digest {
                records.push(
                    self.finish_prepared_source(
                        ctx,
                        &record,
                        ImportSourceReadStatus::SourceChanged,
                        Some(observed_digest),
                        bytes.len() as u64,
                        false,
                    )
                    .await?,
                );
                continue;
            }
            let published = self
                .store
                .publish_registered_blob(
                    ctx,
                    &record.id,
                    IMPORT_SOURCE_SCHEMA,
                    "raw_blob_digest",
                    &bytes,
                    remaining,
                )
                .await;
            let published = match published {
                Ok(digest) => digest,
                Err(error) => {
                    let mut session = self.store.session().await?;
                    let converged: ImportSourceRecord =
                        session.need(ctx, IMPORT_ARTIFACT_KIND, &record.id).await?;
                    session.commit().await?;
                    if converged.input_digest == record.input_digest
                        && converged.payload.status == ImportSourceReadStatus::Ready
                        && converged.payload.raw_blob_digest == record.payload.raw_blob_digest
                        && converged.payload.blob_published
                    {
                        records.push(converged);
                        continue;
                    }
                    return Err(error);
                }
            };
            if published != record.payload.raw_blob_digest {
                return Err(Error::Conflict(
                    "published import blob digest mismatch".into(),
                ));
            }
            records.push(
                self.finish_prepared_source(
                    ctx,
                    &record,
                    ImportSourceReadStatus::Ready,
                    Some(observed_digest),
                    bytes.len() as u64,
                    true,
                )
                .await?,
            );
        }
        Ok(records)
    }

    async fn finish_prepared_source(
        &self,
        ctx: &Context,
        expected: &ImportSourceRecord,
        status: ImportSourceReadStatus,
        observed_digest: Option<String>,
        byte_len: u64,
        blob_published: bool,
    ) -> Result<ImportSourceRecord> {
        let mut session = self.store.session().await?;
        let mut current: ImportSourceRecord = session
            .need(ctx, IMPORT_ARTIFACT_KIND, &expected.id)
            .await?;
        if current.input_digest != expected.input_digest
            || current.revoke_watermark != current_watermark(&mut session, ctx).await?
            || crate::revocation_gate::judge_upstream(
                ctx,
                &mut session,
                &[
                    ("artifact".to_string(), current.id.clone()),
                    ("artifact".to_string(), current.payload.selection_id.clone()),
                ],
            )
            .await?
            .is_some()
            || session
                .get::<Value>(ctx, IMPORT_ARTIFACT_KIND, &current.payload.selection_id)
                .await?
                .is_none()
        {
            return Err(Error::Conflict(
                "prepared import source changed or was revoked".into(),
            ));
        }
        if current.payload.status != ImportSourceReadStatus::Prepared {
            if current.payload.status == status
                && current.payload.observed_digest == observed_digest
                && current.payload.byte_len == byte_len
                && current.payload.blob_published == blob_published
            {
                session.commit().await?;
                return Ok(current);
            }
            return Err(Error::Conflict(
                "import source finalized differently".into(),
            ));
        }
        current.payload.status = status;
        current.payload.observed_digest = observed_digest;
        current.payload.byte_len = byte_len;
        current.payload.blob_published = blob_published;
        current.updated_at = now()?;
        validate_source_record(ctx, &current)?;
        session
            .put(
                ctx,
                IMPORT_ARTIFACT_KIND,
                &current.id,
                ctx.actor(),
                &current,
            )
            .await?;
        session
            .audit(ctx, "e16.import.source.finalize", &current.id)
            .await?;
        session.commit().await?;
        Ok(current)
    }

    pub async fn execute(&self, ctx: &Context, selection_id: &str) -> Result<ImportResultRecord> {
        ctx.require(&[Role::Admin])?;
        identifier(selection_id)?;
        let _guard = self.operation_lock.lock().await;
        let (selection, source_records) = self.load_live_selection(ctx, selection_id).await?;
        // A v1 result of this selection keeps its own id and stays legacy.
        let result_id = import_result_id_v2(&selection.id, &selection.input_digest)?;
        let mut session = self.store.session().await?;
        let result_exists = session
            .get::<Value>(ctx, IMPORT_ARTIFACT_KIND, &result_id)
            .await?
            .is_some();
        session.commit().await?;
        if result_exists {
            let existing = self.load_live_result(ctx, &result_id).await?;
            if existing.payload.selection_id == selection_id {
                return Ok(existing);
            }
            return Err(Error::Conflict("import result identity mismatch".into()));
        }

        let result = self
            .derive_import_result(ctx, &selection, &source_records)
            .await?;
        let mut session = self.store.session().await?;
        validate_live_import_sources(&mut session, ctx, None, &selection, &source_records).await?;
        if let Some(existing) = session
            .get::<ImportResultRecord>(ctx, IMPORT_ARTIFACT_KIND, &result.id)
            .await?
        {
            if fingerprint(&existing)? != fingerprint(&result)? {
                return Err(Error::Conflict("immutable import result differs".into()));
            }
            session.commit().await?;
            return Ok(existing);
        }
        session
            .put_new_import_result_below_event_capacity(
                ctx,
                &result.id,
                ctx.actor(),
                &result,
                result.payload.aggregate_summary.total_events as u64,
            )
            .await?;
        session
            .put_edge(
                ctx,
                IMPORT_ARTIFACT_KIND,
                &result.id,
                IMPORT_ARTIFACT_KIND,
                &selection.id,
            )
            .await?;
        for source in &source_records {
            session
                .put_edge(
                    ctx,
                    IMPORT_ARTIFACT_KIND,
                    &result.id,
                    IMPORT_ARTIFACT_KIND,
                    &source.id,
                )
                .await?;
        }
        session
            .audit(ctx, "e16.import.complete", &result.id)
            .await?;
        session.commit().await?;
        Ok(result)
    }

    /// The fixed v2 derivation of one live selection from its pinned blob
    /// bytes. It reads every Ready blob outside any caller transaction and
    /// never writes; `execute` and the management preflight share it.
    async fn derive_import_result(
        &self,
        ctx: &Context,
        selection: &SourceSelectionRecord,
        source_records: &[ImportSourceRecord],
    ) -> Result<ImportResultRecord> {
        let result_id = import_result_id_v2(&selection.id, &selection.input_digest)?;
        let mut remaining = MAX_TOTAL_READ_BYTES;
        let mut ready_sources = Vec::new();
        let mut owned_bytes = Vec::new();
        let mut unavailable_coverage = SourceCoverage {
            discovery_exhausted: true,
            files_known: true,
            ..SourceCoverage::default()
        };
        for source in source_records {
            match source.payload.status {
                ImportSourceReadStatus::Prepared => {
                    return Err(Error::Conflict("import source is still Prepared".into()));
                }
                ImportSourceReadStatus::Ready => {}
                ImportSourceReadStatus::Missing => {
                    unavailable_coverage.missing += 1;
                    continue;
                }
                ImportSourceReadStatus::PermissionDenied => {
                    unavailable_coverage.permission_denied += 1;
                    continue;
                }
                ImportSourceReadStatus::ZeroRecords => {
                    unavailable_coverage.zero_records += 1;
                    continue;
                }
                ImportSourceReadStatus::TooLarge => {
                    unavailable_coverage.event_truncated += 1;
                    unavailable_coverage.outbound_truncated += 1;
                    continue;
                }
                ImportSourceReadStatus::SourceChanged | ImportSourceReadStatus::InvalidFile => {
                    unavailable_coverage.parsed_fail += 1;
                    unavailable_coverage.bytes_read = unavailable_coverage
                        .bytes_read
                        .saturating_add(source.payload.byte_len);
                    continue;
                }
            }
            let byte_len = usize::try_from(source.payload.byte_len)
                .map_err(|_| Error::Invalid("invalid import source length".into()))?;
            if byte_len == 0 || byte_len > remaining {
                return Err(Error::Invalid("import total exceeds read budget".into()));
            }
            let bytes = self
                .store
                .read_blob(ctx, &source.payload.raw_blob_digest, remaining)
                .await?;
            if bytes.len() != byte_len || hash(&bytes) != source.payload.raw_digest {
                return Err(Error::Conflict("source_changed".into()));
            }
            remaining -= bytes.len();
            ready_sources.push(source);
            owned_bytes.push(bytes);
        }
        let pinned: Vec<_> = ready_sources
            .iter()
            .zip(&owned_bytes)
            .map(|(source, bytes)| PinnedSourceBytes {
                source_name: &source.id,
                raw_bytes: bytes,
                reader: source.payload.reader,
                cluster_hint: Some(&source.payload.logical_source_id),
            })
            .collect();
        let logical_selection = SourceSelection {
            roots: selection.payload.roots.clone(),
            run_ids: ready_sources
                .iter()
                .map(|source| source.id.clone())
                .collect(),
            purpose: selection.payload.purpose,
            allow_model_excerpts: selection.payload.allow_model_excerpts
                && selection.payload.outbound_authorized
                && selection.payload.retention_scope
                    == ImportRetentionScope::LocalWithAuthorizedExcerpts,
        };
        let mut ingest = if pinned.is_empty() {
            ImportAttempt {
                evidence_set: None,
                aggregate_summary: AggregateSummary {
                    coverage: SourceCoverage {
                        discovery_exhausted: true,
                        files_known: true,
                        ..SourceCoverage::default()
                    },
                    ..AggregateSummary::default()
                },
                events: Vec::new(),
                locators: Vec::new(),
                native_traces: Vec::new(),
            }
        } else {
            ingest_selected_sources(&logical_selection, &pinned, None, true)?
        };
        merge_coverage(
            &mut ingest.aggregate_summary.coverage,
            &unavailable_coverage,
        );
        if let Some(set) = ingest.evidence_set.take() {
            ingest.evidence_set = Some(EvidenceSet::build(
                set.id,
                set.members,
                ingest.aggregate_summary.coverage.clone(),
                set.independent_clusters,
            )?);
        }
        let payload = import_result_payload_v2(&selection.id, &ingest);
        if payload.aggregate_summary.total_events != payload.event_records.len() {
            return Err(Error::Conflict("import event count mismatch".into()));
        }
        let mut source_refs = vec![ImportTypedRef {
            kind: "source_selection".into(),
            id: selection.id.clone(),
            content_digest: fingerprint(selection)?,
        }];
        for record in source_records {
            source_refs.push(ImportTypedRef {
                kind: "import_source".into(),
                id: record.id.clone(),
                content_digest: fingerprint(record)?,
            });
        }
        let timestamp = selection.created_at;
        let result = ImportEnvelope {
            schema_version: IMPORT_RESULT_SCHEMA.into(),
            id: result_id,
            namespace: ctx.namespace().into(),
            owner_actor: ctx.actor().into(),
            request_key: selection.request_key.clone(),
            input_digest: fingerprint(&source_refs)?,
            created_at: timestamp,
            updated_at: timestamp,
            source_refs,
            revoke_watermark: selection.revoke_watermark,
            payload,
        };

        validate_import_evidence_sources(
            result.payload.evidence_set.as_ref(),
            &result.payload.locators,
            selection,
            source_records,
        )?;
        Ok(result)
    }

    /// Build the server-derived registration plan from strict management
    /// inputs. Every blob is read and verified outside any job transaction;
    /// the internal v1 request's roots and paths are the storage's own
    /// namespace location, never request data.
    pub(crate) async fn prepare_management_registration(
        &self,
        ctx: &Context,
        request_key: &str,
        sources: &[(String, String, SourceFormat)],
    ) -> Result<PreparedManagementRegistration> {
        ctx.require(&[Role::Admin])?;
        identifier(request_key)?;
        if sources.is_empty() || sources.len() > MAX_DISCOVERED_FILES {
            return Err(Error::Invalid("import requires 1..=200 sources".into()));
        }
        let mut seen = BTreeSet::new();
        let mut remaining = MAX_TOTAL_READ_BYTES;
        let mut specs = Vec::with_capacity(sources.len());
        let mut pinned = Vec::with_capacity(sources.len());
        let mut root: Option<String> = None;
        for (source_id, digest, reader) in sources {
            identifier(source_id)?;
            if !seen.insert(source_id.as_str()) {
                return Err(Error::Conflict("duplicate import source id".into()));
            }
            validate_digest(digest)?;
            if remaining == 0 {
                return Err(Error::Invalid("import total exceeds read budget".into()));
            }
            let input = self
                .store
                .verified_blob_input(ctx, digest, remaining)
                .await?;
            match &root {
                Some(first) if first != &input.root => {
                    return Err(Error::Conflict(
                        "management import blobs span multiple namespace roots".into(),
                    ));
                }
                None => root = Some(input.root.clone()),
                _ => {}
            }
            remaining = remaining
                .checked_sub(input.bytes.len())
                .ok_or_else(|| Error::Invalid("import total exceeds read budget".into()))?;
            specs.push(ImportSourceSpec {
                source_id: source_id.clone(),
                path: input.path.clone(),
                reader: *reader,
                expected_digest: digest.clone(),
            });
            pinned.push(ManagementBlobSource {
                source_id: source_id.clone(),
                reader: *reader,
                input,
            });
        }
        let request = ImportRegistrationRequest {
            schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
            request_key: request_key.into(),
            roots: vec![root.ok_or(Error::Internal)?],
            purpose: Purpose::Inspection,
            allow_model_excerpts: false,
            outbound_authorized: false,
            retention_scope: ImportRetentionScope::LocalPrivate,
            sources: specs,
        };
        validate_registration_request(&request)?;
        Ok(PreparedManagementRegistration {
            request,
            sources: pinned,
        })
    }

    /// One transaction end to end: re-verify every pinned blob under the held
    /// publication lock, converge a cached/existing selection, then write the
    /// selection, its finalized sources, blob edges, idempotency row and audit
    /// record. The caller commits this session together with the job's phase,
    /// terminal state and result.
    pub(crate) async fn register_in_session(
        &self,
        ctx: &Context,
        session: &mut evo_storage::Session,
        prepared: &PreparedManagementRegistration,
    ) -> Result<(SourceSelectionRecord, ManagementImportStatusCounts)> {
        ctx.require(&[Role::Admin])?;
        for source in &prepared.sources {
            let again = session
                .reverify_blob_input(ctx, &source.input.digest, MAX_TOTAL_READ_BYTES)
                .await?;
            if again != source.input.bytes {
                return Err(Error::Conflict(
                    "import blob bytes changed before registration".into(),
                ));
            }
        }
        let request = &prepared.request;
        if let Some(cached) = session
            .cached::<SourceSelectionRecord, _>(
                ctx,
                "e16.import.register",
                &request.request_key,
                request,
            )
            .await?
        {
            validate_selection_record(ctx, &cached)?;
            let (live, sources) = self
                .load_live_selection_in_session(ctx, session, &cached.id)
                .await?;
            return Ok((live, status_counts(&sources)));
        }
        let request_digest = fingerprint(request)?;
        let selection_id = format!("e16sel-{}", &request_digest[..24]);
        if let Some(existing) = session
            .get::<SourceSelectionRecord>(ctx, IMPORT_ARTIFACT_KIND, &selection_id)
            .await?
        {
            if existing.input_digest != request_digest || existing.owner_actor != ctx.actor() {
                return Err(Error::Conflict(
                    "prepared selection differs or is stale".into(),
                ));
            }
            let (live, sources) = self
                .load_live_selection_in_session(ctx, session, &selection_id)
                .await?;
            return Ok((live, status_counts(&sources)));
        }
        let timestamp = now()?;
        let watermark = current_watermark(session, ctx).await?;
        let mut records = Vec::with_capacity(prepared.sources.len());
        for source in &prepared.sources {
            let id = import_source_id(&selection_id, &source.source_id);
            let record = ImportEnvelope {
                schema_version: IMPORT_SOURCE_SCHEMA.into(),
                id,
                namespace: ctx.namespace().into(),
                owner_actor: ctx.actor().into(),
                request_key: request.request_key.clone(),
                // The persisted identity must match the legacy register
                // formula exactly: `dependency_integrity` recomputes it from
                // `source_spec(source)` and `raw_digest`, so a different
                // tuple here would make every managed source uncheckable.
                input_digest: fingerprint(&(
                    ImportSourceSpec {
                        source_id: source.source_id.clone(),
                        path: source.input.path.clone(),
                        reader: source.reader,
                        expected_digest: source.input.digest.clone(),
                    },
                    source.input.digest.as_str(),
                ))?,
                created_at: timestamp,
                updated_at: timestamp,
                source_refs: vec![ImportTypedRef {
                    kind: "blob".into(),
                    id: source.input.digest.clone(),
                    content_digest: source.input.digest.clone(),
                }],
                revoke_watermark: watermark,
                payload: ImportSourcePayload {
                    selection_id: selection_id.clone(),
                    logical_source_id: source.source_id.clone(),
                    authorized_path: source.input.path.clone(),
                    reader: source.reader,
                    reader_version: source.reader.as_label().into(),
                    raw_digest: source.input.digest.clone(),
                    observed_digest: None,
                    raw_blob_digest: source.input.digest.clone(),
                    blob_published: false,
                    byte_len: 0,
                    status: ImportSourceReadStatus::Prepared,
                    task_origin: TaskOrigin::ImportedHistory,
                    execution_attestation: ExecutionAttestation::UnverifiedImport,
                    purpose: request.purpose,
                },
            };
            records.push(self.finalize_source_in_session(
                ctx,
                &record,
                ImportSourceReadStatus::Ready,
                Some(source.input.digest.clone()),
                source.input.bytes.len() as u64,
                true,
            )?);
        }
        let source_refs = source_record_refs(&records)?;
        let selection = ImportEnvelope {
            schema_version: SOURCE_SELECTION_SCHEMA.into(),
            id: selection_id.clone(),
            namespace: ctx.namespace().into(),
            owner_actor: ctx.actor().into(),
            request_key: request.request_key.clone(),
            input_digest: request_digest.clone(),
            created_at: timestamp,
            updated_at: timestamp,
            source_refs,
            revoke_watermark: watermark,
            payload: SourceSelectionPayload {
                purpose: request.purpose,
                allow_model_excerpts: request.allow_model_excerpts,
                outbound_authorized: request.outbound_authorized,
                retention_scope: request.retention_scope,
                roots: request.roots.clone(),
                import_source_ids: records.iter().map(|record| record.id.clone()).collect(),
            },
        };
        // Every id is judged before the first row is written, so a refusal
        // leaves no half-registered selection behind.
        session.ensure_revocation_namespace_live(ctx).await?;
        for record in &records {
            ensure_import_source_id_is_free(session, ctx, &record.id).await?;
        }
        for record in &records {
            put_new_artifact(session, ctx, record).await?;
            session
                .put_edge(
                    ctx,
                    IMPORT_ARTIFACT_KIND,
                    &record.id,
                    "blob",
                    &record.payload.raw_blob_digest,
                )
                .await?;
            session
                .audit(ctx, "e16.import.source.finalize", &record.id)
                .await?;
        }
        put_new_artifact(session, ctx, &selection).await?;
        for record in &records {
            session
                .put_edge(
                    ctx,
                    IMPORT_ARTIFACT_KIND,
                    &selection.id,
                    IMPORT_ARTIFACT_KIND,
                    &record.id,
                )
                .await?;
        }
        session
            .cache(
                ctx,
                "e16.import.register",
                &request.request_key,
                request,
                &selection.id,
                &selection,
            )
            .await?;
        session
            .audit(ctx, "e16.import.register", &selection.id)
            .await?;
        Ok((selection, status_counts(&records)))
    }

    /// The final in-memory source shape of one management registration. The
    /// returned record is validated exactly like a persisted v1 source.
    fn finalize_source_in_session(
        &self,
        ctx: &Context,
        prepared: &ImportSourceRecord,
        status: ImportSourceReadStatus,
        observed_digest: Option<String>,
        byte_len: u64,
        blob_published: bool,
    ) -> Result<ImportSourceRecord> {
        let mut current = prepared.clone();
        current.payload.status = status;
        current.payload.observed_digest = observed_digest;
        current.payload.byte_len = byte_len;
        current.payload.blob_published = blob_published;
        current.updated_at = prepared.created_at;
        validate_source_record(ctx, &current)?;
        Ok(current)
    }

    /// Build the execution plan outside the job transaction: the live
    /// selection, its exact sources and the stored result or the v2 result
    /// derived from the pinned blobs.
    pub(crate) async fn prepare_management_execution(
        &self,
        ctx: &Context,
        selection_id: &str,
    ) -> Result<PreparedManagementExecution> {
        ctx.require(&[Role::Admin])?;
        identifier(selection_id)?;
        let (selection, source_records) = self.load_live_selection(ctx, selection_id).await?;
        let result_id = import_result_id_v2(&selection.id, &selection.input_digest)?;
        let mut session = self.store.session().await?;
        let result_exists = session
            .get::<Value>(ctx, IMPORT_ARTIFACT_KIND, &result_id)
            .await?
            .is_some();
        session.commit().await?;
        let result = if result_exists {
            let existing = self.load_live_result(ctx, &result_id).await?;
            if existing.payload.selection_id != selection_id {
                return Err(Error::Conflict("import result identity mismatch".into()));
            }
            existing
        } else {
            self.derive_import_result(ctx, &selection, &source_records)
                .await?
        };
        Ok(PreparedManagementExecution {
            selection,
            sources: source_records,
            result,
        })
    }

    /// One transaction end to end: reload the live selection and sources,
    /// re-verify every pinned blob under the held publication lock, then
    /// return the identical stored result or insert the derived result with
    /// its edges, event-capacity admission and audit row. The caller commits
    /// together with the job's terminal state.
    pub(crate) async fn execute_in_session(
        &self,
        ctx: &Context,
        session: &mut evo_storage::Session,
        prepared: &PreparedManagementExecution,
    ) -> Result<ImportResultRecord> {
        ctx.require(&[Role::Admin])?;
        let (selection, sources) = self
            .load_live_selection_in_session(ctx, session, &prepared.selection.id)
            .await?;
        if fingerprint(&selection)? != fingerprint(&prepared.selection)?
            || fingerprint(&sources)? != fingerprint(&prepared.sources)?
        {
            return Err(Error::Conflict(
                "import selection changed before execution".into(),
            ));
        }
        for source in &sources {
            if source.payload.status != ImportSourceReadStatus::Ready {
                continue;
            }
            let byte_len = usize::try_from(source.payload.byte_len)
                .map_err(|_| Error::Invalid("invalid import source length".into()))?;
            let bytes = session
                .reverify_blob_input(ctx, &source.payload.raw_blob_digest, byte_len)
                .await?;
            if bytes.len() != byte_len || hash(&bytes) != source.payload.raw_digest {
                return Err(Error::Conflict("source_changed".into()));
            }
        }
        if let Some(existing) = session
            .get::<ImportResultRecord>(ctx, IMPORT_ARTIFACT_KIND, &prepared.result.id)
            .await?
        {
            if fingerprint(&existing)? != fingerprint(&prepared.result)? {
                return Err(Error::Conflict("immutable import result differs".into()));
            }
            return Ok(existing);
        }
        session
            .put_new_import_result_below_event_capacity(
                ctx,
                &prepared.result.id,
                ctx.actor(),
                &prepared.result,
                prepared.result.payload.aggregate_summary.total_events as u64,
            )
            .await?;
        session
            .put_edge(
                ctx,
                IMPORT_ARTIFACT_KIND,
                &prepared.result.id,
                IMPORT_ARTIFACT_KIND,
                &selection.id,
            )
            .await?;
        for source in &sources {
            session
                .put_edge(
                    ctx,
                    IMPORT_ARTIFACT_KIND,
                    &prepared.result.id,
                    IMPORT_ARTIFACT_KIND,
                    &source.id,
                )
                .await?;
        }
        session
            .audit(ctx, "e16.import.complete", &prepared.result.id)
            .await?;
        Ok(prepared.result.clone())
    }

    /// The current (v2) result after its static checks. A v1 result is legacy
    /// history: readable through `read_result_version`, never a current input.
    pub async fn load_live_result(
        &self,
        ctx: &Context,
        result_id: &str,
    ) -> Result<ImportResultRecord> {
        match self.load_static_result(ctx, result_id).await? {
            StoredImportResult::V2(record) => Ok(record),
            StoredImportResult::LegacyV1(_) => Err(Error::Conflict("import_result_legacy".into())),
        }
    }

    pub(crate) async fn load_static_result(
        &self,
        ctx: &Context,
        result_id: &str,
    ) -> Result<StoredImportResult> {
        Ok(self.static_result(ctx, result_id).await?.stored)
    }

    /// Admin read projection through the static version table (§5.1.1): the
    /// stored version, its read state, both lineage ids of its selection and the
    /// stored text. A valid v1 record reads as legacy; a v2 record is
    /// current_verified only when the fixed derivation recomputed from the exact
    /// raw bytes equals it.
    pub async fn read_result_version(
        &self,
        ctx: &Context,
        result_id: &str,
    ) -> Result<ImportResultReadView> {
        ctx.require(&[Role::Admin])?;
        let read = self.static_result(ctx, result_id).await?;
        let (schema_version, state) = match &read.stored {
            StoredImportResult::LegacyV1(_) => (
                LEGACY_IMPORT_RESULT_SCHEMA_V1,
                ImportResultReadState::Legacy,
            ),
            StoredImportResult::V2(record) => {
                let rebuilt = self
                    .reconstruct_local_import(ctx, &read.selection, &read.sources)
                    .await?;
                if fingerprint(&import_result_payload_v2(&read.selection.id, &rebuilt))?
                    != fingerprint(&record.payload)?
                {
                    return Err(Error::Conflict(
                        "persisted import derivation mismatch".into(),
                    ));
                }
                (
                    IMPORT_RESULT_SCHEMA_V2,
                    ImportResultReadState::CurrentVerified,
                )
            }
        };
        self.static_result(ctx, result_id).await?;
        Ok(ImportResultReadView {
            schema_version: schema_version.into(),
            state,
            selection_id: read.selection.id.clone(),
            legacy_v1_id: legacy_import_result_id_v1(&read.selection.id),
            current_v2_id: import_result_id_v2(&read.selection.id, &read.selection.input_digest)?,
            record: read.body,
        })
    }

    /// Static identity, closure, live-source and watermark checks of a stored
    /// result of either known version; for Admin also the raw bytes its evidence
    /// binds. Nothing is re-derived here.
    async fn static_result(&self, ctx: &Context, result_id: &str) -> Result<StaticImportResult> {
        ctx.require(&[Role::Admin, Role::Evaluator])?;
        identifier(result_id)?;
        let mut session = self.store.session().await?;
        let body = session
            .object_row(ctx, IMPORT_ARTIFACT_KIND, result_id)
            .await?
            .ok_or(Error::NotFound)?
            .body;
        let stored = StoredImportResult::decode(&body)?;
        validate_result_record(ctx, &stored)?;
        let result = stored.core();
        let selection: SourceSelectionRecord = session
            .need(ctx, IMPORT_ARTIFACT_KIND, result.selection_id)
            .await?;
        let source_records =
            load_and_validate_source_records(&mut session, ctx, &selection).await?;
        validate_import_sources_own(&mut session, ctx, &selection, &source_records).await?;
        let expected_id = match &stored {
            StoredImportResult::V2(_) => {
                import_result_id_v2(&selection.id, &selection.input_digest)?
            }
            StoredImportResult::LegacyV1(_) => legacy_import_result_id_v1(&selection.id),
        };
        if result.id != expected_id {
            return Err(Error::Conflict("import_result_invalid".into()));
        }
        let selection_digest = fingerprint(&selection)?;
        if result.revoke_watermark != selection.revoke_watermark
            || result.source_refs.first().is_none_or(|source| {
                source.kind != "source_selection"
                    || source.id != selection.id
                    || source.content_digest != selection_digest
            })
            || result.source_refs.len() != source_records.len() + 1
            || result.total_events != result.event_records.len()
        {
            return Err(Error::Conflict("import result closure mismatch".into()));
        }
        for (source_ref, source) in result.source_refs.iter().skip(1).zip(&source_records) {
            if source_ref.kind != "import_source"
                || source_ref.id != source.id
                || source_ref.content_digest != fingerprint(source)?
            {
                return Err(Error::Conflict("import result source mismatch".into()));
            }
        }
        // Evaluator access is a summary-only historical view, not a byte proof.
        if ctx.role() == Role::Admin {
            validate_import_evidence_sources(
                result.evidence_set,
                result.locators,
                &selection,
                &source_records,
            )?;
        }
        session.commit().await?;
        if ctx.role() == Role::Admin {
            for source in &source_records {
                if source.payload.status != ImportSourceReadStatus::Ready {
                    continue;
                }
                let bytes = self
                    .store
                    .read_blob(
                        ctx,
                        &source.payload.raw_blob_digest,
                        source.payload.byte_len as usize,
                    )
                    .await?;
                if hash(&bytes) != source.payload.raw_digest {
                    return Err(Error::Conflict("source_changed".into()));
                }
                verify_import_evidence_bytes(&result, &source.id, &bytes)?;
            }
        }
        let mut session = self.store.session().await?;
        validate_import_sources_own(&mut session, ctx, &selection, &source_records).await?;
        session.commit().await?;
        Ok(StaticImportResult {
            stored,
            body,
            selection,
            sources: source_records,
        })
    }

    pub async fn read_event_fragment(
        &self,
        ctx: &Context,
        result_id: &str,
        source_id: &str,
        event_index: usize,
    ) -> Result<Vec<u8>> {
        ctx.require(&[Role::Admin])?;
        // Bounded raw fragments of both known versions stay readable.
        let result = self.load_static_result(ctx, result_id).await?;
        let event = result
            .core()
            .event_records
            .iter()
            .find(|event| event.source_id == source_id && event.event_index == event_index)
            .ok_or(Error::NotFound)?;
        let mut session = self.store.session().await?;
        let source: ImportSourceRecord = session.need(ctx, IMPORT_ARTIFACT_KIND, source_id).await?;
        validate_source_record(ctx, &source)?;
        if source.payload.status != ImportSourceReadStatus::Ready {
            return Err(Error::Conflict("event source is not readable".into()));
        }
        session.commit().await?;
        let bytes = self
            .store
            .read_blob(
                ctx,
                &source.payload.raw_blob_digest,
                source.payload.byte_len as usize,
            )
            .await?;
        let locator = EvidenceLocator::build(
            source_id,
            &source.payload.raw_digest,
            event.byte_start,
            event.byte_end,
            event.event_index,
            &event.raw_fragment_digest,
        )?;
        let fragment = locator.verify_and_extract(&bytes)?.to_vec();
        self.load_static_result(ctx, result_id).await?;
        Ok(fragment)
    }

    /// Read an owner's local, unreviewed E16 development snapshot. No model,
    /// candidate, approval, or durable artifact is produced by this consumer.
    pub async fn read_local_development_material(
        &self,
        ctx: &Context,
        request: &ImportMaterialReadRequest,
    ) -> Result<ImportedDevelopmentMaterial> {
        self.read_local_material_at_boundary(ctx, request, std::future::ready(Ok(())))
            .await
    }

    // Private boundary future permits deterministic tests of committed DB
    // changes after byte reconstruction. The public entry supplies only Ready.
    async fn read_local_material_at_boundary<F>(
        &self,
        ctx: &Context,
        request: &ImportMaterialReadRequest,
        boundary: F,
    ) -> Result<ImportedDevelopmentMaterial>
    where
        F: std::future::Future<Output = Result<()>>,
    {
        ctx.require(&[Role::Admin])?;
        request.validate()?;
        let snapshot = self
            .load_local_material_snapshot(ctx, &request.result)
            .await?;
        let rebuilt = self
            .reconstruct_local_import(ctx, &snapshot.selection, &snapshot.sources)
            .await?;
        if fingerprint(&import_result_payload_v2(&snapshot.selection.id, &rebuilt))?
            != fingerprint(&snapshot.result.payload)?
        {
            return Err(Error::Conflict(
                "persisted import derivation mismatch".into(),
            ));
        }
        let evidence = rebuilt.evidence_set.as_ref().ok_or_else(|| {
            Error::Invalid("import produced no usable development material".into())
        })?;
        let (excerpts, projection_coverage) =
            project_local_import_events(request, &rebuilt.events)?;
        let mut material = ImportedDevelopmentMaterial {
            namespace: ctx.namespace().into(),
            revoke_watermark: snapshot.result.revoke_watermark,
            result: request.result.clone(),
            selection: import_artifact_ref(
                ImportArtifactSchema::SourceSelection,
                &snapshot.selection,
            )?,
            sources: snapshot
                .sources
                .iter()
                .map(|source| {
                    Ok(ImportedSourceBinding {
                        artifact: import_artifact_ref(ImportArtifactSchema::ImportSource, source)?,
                        raw_digest: source.payload.raw_digest.clone(),
                    })
                })
                .collect::<Result<_>>()?,
            evidence_set_id: evidence.id.clone(),
            evidence_digest: evidence.digest.clone(),
            aggregate_summary: rebuilt.aggregate_summary,
            independent_cluster_count: evidence.independent_clusters.len(),
            excerpts,
            projection_coverage,
            digest: String::new(),
            task_origin: TaskOrigin::ImportedHistory,
            execution_attestation: ExecutionAttestation::UnverifiedImport,
            purpose: Purpose::Development,
            privacy_status: ImportMaterialPrivacyStatus::Unreviewed,
            dispatch_policy: ImportMaterialDispatchPolicy::NoModelDispatch,
        };
        material.digest = fingerprint(&material)?;
        material.validate()?;
        boundary.await?;
        let current = self
            .load_local_material_snapshot(ctx, &request.result)
            .await?;
        if fingerprint(&(&snapshot.result, &snapshot.selection, &snapshot.sources))?
            != fingerprint(&(&current.result, &current.selection, &current.sources))?
        {
            return Err(Error::Conflict(
                "import material sources changed while reading".into(),
            ));
        }
        Ok(material)
    }

    async fn load_local_material_snapshot(
        &self,
        ctx: &Context,
        pinned: &ImportArtifactRef,
    ) -> Result<LocalImportSnapshot> {
        let mut session = self.store.session().await?;
        let body = session
            .object_row(ctx, IMPORT_ARTIFACT_KIND, &pinned.id)
            .await?
            .ok_or(Error::NotFound)?
            .body;
        let stored = StoredImportResult::decode(&body)?;
        validate_result_record(ctx, &stored)?;
        // A v1 result is refused before any reconstruction or new derivation.
        let StoredImportResult::V2(result) = stored else {
            return Err(Error::Conflict("import_result_legacy".into()));
        };
        if result.id != pinned.id
            || fingerprint(&result)? != pinned.object_digest
            || result.input_digest != fingerprint(&result.source_refs)?
        {
            return Err(Error::Conflict(
                "fixed import result identity mismatch".into(),
            ));
        }
        let selection: SourceSelectionRecord = session
            .need(ctx, IMPORT_ARTIFACT_KIND, &result.payload.selection_id)
            .await?;
        validate_selection_record(ctx, &selection)?;
        if selection.id != result.payload.selection_id
            || selection.payload.purpose != Purpose::Development
            || result.id != import_result_id_v2(&selection.id, &selection.input_digest)?
            || result.owner_actor != selection.owner_actor
            || result.request_key != selection.request_key
            || result.revoke_watermark != selection.revoke_watermark
            || selection.payload.import_source_ids.len() > MAX_DISCOVERED_FILES
            || selection
                .payload
                .import_source_ids
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != selection.payload.import_source_ids.len()
            || selection.payload.import_source_ids
                != selection
                    .source_refs
                    .iter()
                    .map(|source| source.id.clone())
                    .collect::<Vec<_>>()
        {
            return Err(Error::Conflict(
                "local import selection identity mismatch".into(),
            ));
        }
        let sources = load_and_validate_source_records(&mut session, ctx, &selection).await?;
        let mut specs = Vec::new();
        for (key, source) in selection.payload.import_source_ids.iter().zip(&sources) {
            let spec = ImportSourceSpec {
                source_id: source.payload.logical_source_id.clone(),
                path: source.payload.authorized_path.clone(),
                reader: source.payload.reader,
                expected_digest: source.payload.raw_digest.clone(),
            };
            if source.id != *key
                || source.id != import_source_id(&selection.id, &spec.source_id)
                || source.owner_actor != selection.owner_actor
                || source.request_key != selection.request_key
                || source.revoke_watermark != selection.revoke_watermark
                || source.payload.purpose != Purpose::Development
                || source.payload.reader_version != source.payload.reader.as_label()
                || source.input_digest != fingerprint(&(&spec, spec.expected_digest.as_str()))?
            {
                return Err(Error::Conflict(
                    "local import source identity mismatch".into(),
                ));
            }
            specs.push(spec);
        }
        let registration = ImportRegistrationRequest {
            schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
            request_key: selection.request_key.clone(),
            roots: selection.payload.roots.clone(),
            purpose: selection.payload.purpose,
            allow_model_excerpts: selection.payload.allow_model_excerpts,
            outbound_authorized: selection.payload.outbound_authorized,
            retention_scope: selection.payload.retention_scope,
            sources: specs,
        };
        validate_registration_request(&registration)?;
        if selection.input_digest != fingerprint(&registration)?
            || selection.id != format!("e16sel-{}", &fingerprint(&registration)?[..24])
        {
            return Err(Error::Conflict(
                "local import registration digest mismatch".into(),
            ));
        }
        let expected_refs = std::iter::once(Ok(ImportTypedRef {
            kind: "source_selection".into(),
            id: selection.id.clone(),
            content_digest: fingerprint(&selection)?,
        }))
        .chain(sources.iter().map(|source| {
            Ok(ImportTypedRef {
                kind: "import_source".into(),
                id: source.id.clone(),
                content_digest: fingerprint(source)?,
            })
        }))
        .collect::<Result<Vec<_>>>()?;
        if result.source_refs != expected_refs {
            return Err(Error::Conflict(
                "local import result closure mismatch".into(),
            ));
        }
        validate_live_import_sources(&mut session, ctx, Some(&result.id), &selection, &sources)
            .await?;
        session.commit().await?;
        Ok(LocalImportSnapshot {
            result,
            selection,
            sources,
        })
    }

    /// Re-runs the pinned readers over the exact registered raw bytes.
    async fn reconstruct_local_import(
        &self,
        ctx: &Context,
        selection: &SourceSelectionRecord,
        sources: &[ImportSourceRecord],
    ) -> Result<ImportAttempt> {
        let mut remaining = MAX_TOTAL_READ_BYTES;
        let mut ready = Vec::new();
        let mut owned = Vec::new();
        let mut unavailable = SourceCoverage {
            discovery_exhausted: true,
            files_known: true,
            ..SourceCoverage::default()
        };
        for source in sources {
            match source.payload.status {
                ImportSourceReadStatus::Prepared => {
                    return Err(Error::Conflict("import source is still Prepared".into()));
                }
                ImportSourceReadStatus::Ready => {}
                ImportSourceReadStatus::Missing => {
                    unavailable.missing += 1;
                    continue;
                }
                ImportSourceReadStatus::PermissionDenied => {
                    unavailable.permission_denied += 1;
                    continue;
                }
                ImportSourceReadStatus::ZeroRecords => {
                    unavailable.zero_records += 1;
                    continue;
                }
                ImportSourceReadStatus::TooLarge => {
                    unavailable.event_truncated += 1;
                    unavailable.outbound_truncated += 1;
                    continue;
                }
                ImportSourceReadStatus::SourceChanged | ImportSourceReadStatus::InvalidFile => {
                    unavailable.parsed_fail += 1;
                    unavailable.bytes_read = unavailable
                        .bytes_read
                        .saturating_add(source.payload.byte_len);
                    continue;
                }
            }
            let len = usize::try_from(source.payload.byte_len)
                .map_err(|_| Error::Invalid("invalid import source length".into()))?;
            let charge = len
                .checked_add(1)
                .filter(|charge| *charge <= remaining)
                .ok_or_else(|| {
                    Error::Invalid(
                        "fixed import cannot be verified within local read budget".into(),
                    )
                })?;
            remaining -= charge;
            let bytes = self
                .store
                .read_blob(ctx, &source.payload.raw_blob_digest, len)
                .await?;
            if bytes.len() != len || hash(&bytes) != source.payload.raw_digest {
                return Err(Error::Conflict("source_changed".into()));
            }
            ready.push(source);
            owned.push(bytes);
        }
        let pinned: Vec<_> = ready
            .iter()
            .zip(&owned)
            .map(|(source, bytes)| PinnedSourceBytes {
                source_name: &source.id,
                raw_bytes: bytes,
                reader: source.payload.reader,
                cluster_hint: Some(&source.payload.logical_source_id),
            })
            .collect();
        let logical_selection = SourceSelection {
            roots: selection.payload.roots.clone(),
            run_ids: ready.iter().map(|source| source.id.clone()).collect(),
            purpose: selection.payload.purpose,
            allow_model_excerpts: selection.payload.allow_model_excerpts
                && selection.payload.outbound_authorized
                && selection.payload.retention_scope
                    == ImportRetentionScope::LocalWithAuthorizedExcerpts,
        };
        let mut rebuilt = if pinned.is_empty() {
            ImportAttempt {
                evidence_set: None,
                aggregate_summary: AggregateSummary {
                    coverage: SourceCoverage {
                        discovery_exhausted: true,
                        files_known: true,
                        ..SourceCoverage::default()
                    },
                    ..AggregateSummary::default()
                },
                events: Vec::new(),
                locators: Vec::new(),
                native_traces: Vec::new(),
            }
        } else {
            ingest_selected_sources(&logical_selection, &pinned, None, true)?
        };
        merge_coverage(&mut rebuilt.aggregate_summary.coverage, &unavailable);
        if let Some(set) = rebuilt.evidence_set.take() {
            rebuilt.evidence_set = Some(EvidenceSet::build(
                set.id,
                set.members,
                rebuilt.aggregate_summary.coverage.clone(),
                set.independent_clusters,
            )?);
        }
        Ok(rebuilt)
    }

    async fn load_live_selection(
        &self,
        ctx: &Context,
        selection_id: &str,
    ) -> Result<(SourceSelectionRecord, Vec<ImportSourceRecord>)> {
        let mut session = self.store.session().await?;
        let result = self
            .load_live_selection_in_session(ctx, &mut session, selection_id)
            .await?;
        session.commit().await?;
        Ok(result)
    }

    /// The same live closure read inside a caller-owned transaction, so a
    /// management writer rechecks it between its first write and commit.
    async fn load_live_selection_in_session(
        &self,
        ctx: &Context,
        session: &mut evo_storage::Session,
        selection_id: &str,
    ) -> Result<(SourceSelectionRecord, Vec<ImportSourceRecord>)> {
        let selection: SourceSelectionRecord = session
            .need(ctx, IMPORT_ARTIFACT_KIND, selection_id)
            .await?;
        validate_selection_record(ctx, &selection)?;
        let sources = load_and_validate_source_records(session, ctx, &selection).await?;
        validate_live_import_sources(session, ctx, None, &selection, &sources).await?;
        Ok((selection, sources))
    }
}

/// The bounded status histogram of one selection's persisted sources.
fn status_counts(sources: &[ImportSourceRecord]) -> ManagementImportStatusCounts {
    let mut counts = ManagementImportStatusCounts::default();
    for source in sources {
        match source.payload.status {
            ImportSourceReadStatus::Prepared => counts.prepared += 1,
            ImportSourceReadStatus::Ready => counts.ready += 1,
            ImportSourceReadStatus::Missing => counts.missing += 1,
            ImportSourceReadStatus::PermissionDenied => counts.permission_denied += 1,
            ImportSourceReadStatus::SourceChanged => counts.source_changed += 1,
            ImportSourceReadStatus::ZeroRecords => counts.zero_records += 1,
            ImportSourceReadStatus::InvalidFile => counts.invalid_file += 1,
            ImportSourceReadStatus::TooLarge => counts.too_large += 1,
        }
    }
    counts
}

struct LocalImportSnapshot {
    result: ImportResultRecord,
    selection: SourceSelectionRecord,
    sources: Vec<ImportSourceRecord>,
}

fn import_artifact_ref<T: Serialize>(
    schema: ImportArtifactSchema,
    record: &ImportEnvelope<T>,
) -> Result<ImportArtifactRef> {
    Ok(ImportArtifactRef {
        schema,
        id: record.id.clone(),
        object_digest: fingerprint(record)?,
    })
}

fn persisted_local_event(event: &ImportedEvent) -> PersistedImportEvent {
    PersistedImportEvent {
        source_id: event.source_id.clone(),
        event_index: event.event_index,
        byte_start: event.byte_start,
        byte_end: event.byte_end,
        raw_fragment_digest: event.raw_fragment_digest.clone(),
        normalized_content_digest: hash(event.content.as_bytes()),
        role: event.role.clone(),
        kind: event.kind.clone(),
    }
}

/// The fixed v2 derivation (reader_set.v2, normalization.v2) of one import
/// attempt: `execute` persists it and every current verification recomputes it
/// from the exact raw bytes.
fn import_result_payload_v2(selection_id: &str, attempt: &ImportAttempt) -> ImportResultPayload {
    let coverage = &attempt.aggregate_summary.coverage;
    let events = attempt.events.len() as u64;
    // This import's own statistics: the readers drop or filter no event and
    // render no tool parameters, so none are truncated.
    let import_coverage = TraceCoverage {
        discovery_exhausted: coverage.discovery_exhausted,
        files_known: coverage.files_known,
        bytes_read: coverage.bytes_read,
        parsed_ok: coverage.parsed_ok,
        parsed_fail: coverage.parsed_fail,
        permission_denied: coverage.permission_denied,
        missing: coverage.missing,
        unsupported_format: coverage.unsupported_format,
        zero_records: coverage.zero_records,
        events_seen: events,
        events_exported: events,
        events_filtered: 0,
        event_truncated: coverage.event_truncated,
        char_truncated: coverage.char_truncated,
        parameter_truncated_events: 0,
        parameter_truncated_bytes: 0,
        excerpt_truncated: coverage.excerpt_truncated,
        outbound_truncated: coverage.outbound_truncated,
    };
    // A Native header's declared coverage can lower the result to partial,
    // never raise it to complete.
    let state = if attempt.evidence_set.is_none() {
        ImportResultState::Failed
    } else if import_coverage.complete()
        && attempt
            .native_traces
            .iter()
            .all(|trace| trace.header.coverage.complete())
    {
        ImportResultState::Complete
    } else {
        ImportResultState::Partial
    };
    ImportResultPayload {
        selection_id: selection_id.into(),
        reader_version: IMPORT_READER_SET_V2.into(),
        normalization_version: IMPORT_NORMALIZATION_V2.into(),
        state,
        evidence_set: attempt.evidence_set.clone(),
        aggregate_summary: attempt.aggregate_summary.clone(),
        import_coverage,
        locators: attempt.locators.clone(),
        event_records: attempt.events.iter().map(persisted_local_event).collect(),
        native_traces: attempt.native_traces.clone(),
        generation_status: "blocked_external_generation_conditions_unavailable".into(),
    }
}

fn project_local_import_events(
    request: &ImportMaterialReadRequest,
    events: &[ImportedEvent],
) -> Result<(Vec<ImportedMaterialExcerpt>, ImportProjectionCoverage)> {
    // Resolve every requested key before clipping: an invalid trailing request
    // cannot hide behind the byte limit of an earlier excerpt.
    let selected = request
        .events
        .iter()
        .map(|key| {
            events
                .iter()
                .find(|event| {
                    event.source_id == key.source_id && event.event_index == key.event_index
                })
                .ok_or(Error::NotFound)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut excerpts = Vec::new();
    let mut next = None;
    for (key, event) in request.events.iter().zip(selected) {
        if event.role.len().saturating_add(event.kind.len()) > MAX_TOTAL_EXCERPT_BYTES {
            next = Some(ImportMaterialCursor {
                event: key.clone(),
                byte_offset: 0,
            });
            break;
        }
        let mut excerpt = ImportedMaterialExcerpt {
            source_id: event.source_id.clone(),
            event_index: event.event_index,
            byte_start: event.byte_start,
            byte_end: event.byte_end,
            raw_fragment_digest: event.raw_fragment_digest.clone(),
            normalized_content_digest: hash(event.content.as_bytes()),
            content_digest: hash(event.content.as_bytes()),
            role: event.role.clone(),
            kind: event.kind.clone(),
            content: event.content.clone(),
            truncated: false,
        };
        excerpts.push(excerpt.clone());
        if serde_json::to_vec(&excerpts)
            .map_err(|_| Error::Internal)?
            .len()
            <= MAX_TOTAL_EXCERPT_BYTES
        {
            continue;
        }
        excerpts.pop();
        excerpt.content.clear();
        excerpt.content_digest = hash(b"");
        excerpt.truncated = true;
        excerpts.push(excerpt.clone());
        if serde_json::to_vec(&excerpts)
            .map_err(|_| Error::Internal)?
            .len()
            > MAX_TOTAL_EXCERPT_BYTES
        {
            excerpts.pop();
            next = Some(ImportMaterialCursor {
                event: key.clone(),
                byte_offset: 0,
            });
            break;
        }
        // Search actual serialized array bytes, including escaping and metadata.
        let boundaries: Vec<_> = event
            .content
            .char_indices()
            .map(|(offset, _)| offset)
            .collect();
        let mut low = 0usize;
        let mut high = boundaries.len();
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            let end = boundaries[middle - 1];
            excerpt.content = event.content[..end].into();
            excerpt.content_digest = hash(excerpt.content.as_bytes());
            *excerpts.last_mut().ok_or(Error::Internal)? = excerpt.clone();
            if serde_json::to_vec(&excerpts)
                .map_err(|_| Error::Internal)?
                .len()
                <= MAX_TOTAL_EXCERPT_BYTES
            {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        let end = if low == 0 { 0 } else { boundaries[low - 1] };
        excerpt.content = event.content[..end].into();
        excerpt.content_digest = hash(excerpt.content.as_bytes());
        *excerpts.last_mut().ok_or(Error::Internal)? = excerpt;
        next = Some(ImportMaterialCursor {
            event: key.clone(),
            byte_offset: end,
        });
        break;
    }
    let coverage = ImportProjectionCoverage {
        requested_events: request.events.len(),
        returned_excerpts: excerpts.len(),
        serialized_excerpt_bytes: serde_json::to_vec(&excerpts)
            .map_err(|_| Error::Internal)?
            .len(),
        partial: next.is_some(),
        next,
    };
    Ok((excerpts, coverage))
}

fn validate_registration_request(request: &ImportRegistrationRequest) -> Result<()> {
    if request.schema_version != IMPORT_REGISTRATION_SCHEMA {
        return Err(Error::Invalid(
            "unsupported import registration schema".into(),
        ));
    }
    identifier(&request.request_key)?;
    if request.sources.is_empty() || request.sources.len() > MAX_DISCOVERED_FILES {
        return Err(Error::Invalid("import requires 1..=200 sources".into()));
    }
    if request.allow_model_excerpts
        && (!request.outbound_authorized
            || request.retention_scope != ImportRetentionScope::LocalWithAuthorizedExcerpts)
    {
        return Err(Error::Forbidden);
    }
    let mut ids = BTreeSet::new();
    let selection = SourceSelection {
        roots: request.roots.clone(),
        run_ids: request
            .sources
            .iter()
            .map(|source| source.source_id.clone())
            .collect(),
        purpose: request.purpose,
        allow_model_excerpts: request.allow_model_excerpts,
    };
    selection.validate()?;
    for source in &request.sources {
        identifier(&source.source_id)?;
        if !ids.insert(source.source_id.as_str()) {
            return Err(Error::Conflict("duplicate import source id".into()));
        }
        assert_authorized_path(&source.path, &request.roots)?;
        validate_digest(&source.expected_digest)?;
    }
    Ok(())
}

fn import_source_id(selection_id: &str, logical_source_id: &str) -> String {
    format!(
        "e16src-{}",
        &hash(format!("{selection_id}\n{logical_source_id}").as_bytes())[..24]
    )
}

fn source_record_refs(records: &[ImportSourceRecord]) -> Result<Vec<ImportTypedRef>> {
    records
        .iter()
        .map(|record| {
            Ok(ImportTypedRef {
                kind: "import_source".into(),
                id: record.id.clone(),
                content_digest: fingerprint(record)?,
            })
        })
        .collect()
}

async fn read_file_bounded(path: &str, max_bytes: usize) -> std::io::Result<Vec<u8>> {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let file = std::fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take(max_bytes as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "import source exceeds remaining read budget",
            ));
        }
        Ok(bytes)
    })
    .await
    .map_err(std::io::Error::other)?
}

fn merge_coverage(target: &mut SourceCoverage, additional: &SourceCoverage) {
    target.discovery_exhausted &= additional.discovery_exhausted;
    target.files_known &= additional.files_known;
    target.bytes_read += additional.bytes_read;
    target.parsed_ok += additional.parsed_ok;
    target.parsed_fail += additional.parsed_fail;
    target.permission_denied += additional.permission_denied;
    target.missing += additional.missing;
    target.unsupported_format += additional.unsupported_format;
    target.zero_records += additional.zero_records;
    target.event_truncated += additional.event_truncated;
    target.char_truncated += additional.char_truncated;
    target.excerpt_truncated += additional.excerpt_truncated;
    target.outbound_truncated += additional.outbound_truncated;
}

async fn current_watermark(session: &mut evo_storage::Session, ctx: &Context) -> Result<u64> {
    session
        .watermark(ctx)
        .await?
        .and_then(|(sequence, _)| u64::try_from(sequence).ok())
        .ok_or_else(|| Error::Conflict("missing source revoke watermark".into()))
}

/// An import source and a run are the two revocation sources, and a revocation
/// tombstone is keyed by the id of its source alone: a run and an import source
/// that share an id would share one tombstone, and whichever was revoked first would
/// make the other unrevocable. The id of an import source is derived from its
/// selection, so a caller that knows the request can predict it, and the id of a run
/// is chosen by the Host. An id that already has a tombstone is refused with
/// `Forbidden`, as every tombstone gate answers (the creation used to fail with that
/// later, while finalizing, after the prepared rows were committed); an id that
/// already names a run with `Conflict`. Nothing is written. An object of any other
/// kind with this id is a different object and does not matter.
async fn ensure_import_source_id_is_free(
    session: &mut evo_storage::Session,
    ctx: &Context,
    id: &str,
) -> Result<()> {
    if session.get::<Value>(ctx, "tombstone", id).await?.is_some() {
        return Err(Error::Forbidden);
    }
    if session.get::<Value>(ctx, "run", id).await?.is_some() {
        return Err(Error::Conflict(format!(
            "import source {id} cannot be created: a run already has this id \
             (a revocation tombstone is keyed by id)"
        )));
    }
    Ok(())
}

async fn put_new_artifact<T: Serialize>(
    session: &mut evo_storage::Session,
    ctx: &Context,
    record: &ImportEnvelope<T>,
) -> Result<()> {
    if session
        .get::<Value>(ctx, IMPORT_ARTIFACT_KIND, &record.id)
        .await?
        .is_some()
    {
        return Err(Error::Conflict(
            "immutable import artifact already exists".into(),
        ));
    }
    session
        .put(ctx, IMPORT_ARTIFACT_KIND, &record.id, ctx.actor(), record)
        .await
}

async fn load_and_validate_source_records(
    session: &mut evo_storage::Session,
    ctx: &Context,
    selection: &SourceSelectionRecord,
) -> Result<Vec<ImportSourceRecord>> {
    validate_selection_record(ctx, selection)?;
    if selection.source_refs.len() != selection.payload.import_source_ids.len() {
        return Err(Error::Conflict("selection source closure mismatch".into()));
    }
    let mut records = Vec::with_capacity(selection.source_refs.len());
    for source_ref in &selection.source_refs {
        if source_ref.kind != "import_source" {
            return Err(Error::Conflict("selection source kind mismatch".into()));
        }
        let record: ImportSourceRecord = session
            .need(ctx, IMPORT_ARTIFACT_KIND, &source_ref.id)
            .await?;
        validate_source_record(ctx, &record)?;
        if record.payload.selection_id != selection.id
            || fingerprint(&record)? != source_ref.content_digest
        {
            return Err(Error::Conflict("selection source digest mismatch".into()));
        }
        records.push(record);
    }
    Ok(records)
}

/// Content use and new derivation: the current watermark and the complete
/// revocation closure of the result (when given), its selection and sources.
/// The walk starts at these artifacts, so it also judges their own tombstones.
async fn validate_live_import_sources(
    session: &mut evo_storage::Session,
    ctx: &Context,
    result_id: Option<&str>,
    selection: &SourceSelectionRecord,
    sources: &[ImportSourceRecord],
) -> Result<()> {
    if current_watermark(session, ctx).await? != selection.revoke_watermark {
        return Err(Error::Conflict("source revoke watermark changed".into()));
    }
    let roots: Vec<_> = result_id
        .map(str::to_string)
        .into_iter()
        .chain(std::iter::once(selection.id.clone()))
        .chain(sources.iter().map(|source| source.id.clone()))
        .map(|id| ("artifact".to_string(), id))
        .collect();
    if crate::revocation_gate::judge_upstream(ctx, session, &roots)
        .await?
        .is_some()
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

/// A stored result's static read: the current watermark and the selection's and
/// sources' own revocation. Reading its content goes through the closure above.
async fn validate_import_sources_own(
    session: &mut evo_storage::Session,
    ctx: &Context,
    selection: &SourceSelectionRecord,
    sources: &[ImportSourceRecord],
) -> Result<()> {
    if current_watermark(session, ctx).await? != selection.revoke_watermark {
        return Err(Error::Conflict("source revoke watermark changed".into()));
    }
    for id in std::iter::once(&selection.id).chain(sources.iter().map(|source| &source.id)) {
        if crate::revocation_gate::judge_dependency(ctx, session, "artifact", id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}

fn validate_selection_record(ctx: &Context, record: &SourceSelectionRecord) -> Result<()> {
    validate_envelope(ctx, record, SOURCE_SELECTION_SCHEMA)?;
    if record.payload.import_source_ids.is_empty()
        || record.payload.import_source_ids.len() != record.source_refs.len()
    {
        return Err(Error::Conflict("invalid persisted source selection".into()));
    }
    Ok(())
}

fn validate_source_record(ctx: &Context, record: &ImportSourceRecord) -> Result<()> {
    validate_envelope(ctx, record, IMPORT_SOURCE_SCHEMA)?;
    if record.payload.task_origin != TaskOrigin::ImportedHistory
        || record.payload.execution_attestation != ExecutionAttestation::UnverifiedImport
    {
        return Err(Error::Conflict("invalid persisted import source".into()));
    }
    validate_digest(&record.payload.raw_digest)?;
    if let Some(observed) = &record.payload.observed_digest {
        validate_digest(observed)?;
    }
    validate_digest(&record.payload.raw_blob_digest)?;
    if record.payload.raw_blob_digest != record.payload.raw_digest
        || record.source_refs.len() != 1
        || record.source_refs[0].kind != "blob"
        || record.source_refs[0].id != record.payload.raw_blob_digest
        || record.source_refs[0].content_digest != record.payload.raw_digest
    {
        return Err(Error::Conflict("invalid import blob registration".into()));
    }
    let empty_digest = hash(b"");
    match record.payload.status {
        ImportSourceReadStatus::Prepared => {
            if record.payload.blob_published
                || record.payload.observed_digest.is_some()
                || record.payload.byte_len != 0
            {
                return Err(Error::Conflict("invalid Prepared import source".into()));
            }
        }
        ImportSourceReadStatus::Ready => {
            if record.payload.observed_digest.as_deref() != Some(record.payload.raw_digest.as_str())
                || record.payload.byte_len == 0
                || record.payload.byte_len > MAX_TOTAL_READ_BYTES as u64
                || !record.payload.blob_published
            {
                return Err(Error::Conflict("invalid ready import source".into()));
            }
        }
        ImportSourceReadStatus::SourceChanged => {
            if record.payload.blob_published
                || record.payload.observed_digest.as_deref()
                    == Some(record.payload.raw_digest.as_str())
            {
                return Err(Error::Conflict("invalid changed import source".into()));
            }
        }
        ImportSourceReadStatus::ZeroRecords => {
            if record.payload.blob_published
                || record.payload.byte_len != 0
                || record.payload.observed_digest.as_deref() != Some(empty_digest.as_str())
            {
                return Err(Error::Conflict("invalid zero-record import source".into()));
            }
        }
        _ => {
            if record.payload.blob_published {
                return Err(Error::Conflict("unreadable source published a blob".into()));
            }
        }
    }
    Ok(())
}

// Content checks within the existing Admin writer/reader. Imported history
// remains unverified; these checks never manufacture source permissions.
fn validate_import_evidence_sources(
    evidence_set: Option<&EvidenceSet>,
    locators: &[EvidenceLocator],
    selection: &SourceSelectionRecord,
    sources: &[ImportSourceRecord],
) -> Result<()> {
    let mut members = BTreeSet::new();
    if let Some(set) = evidence_set {
        let rebuilt = EvidenceSet::build(
            set.id.clone(),
            set.members.clone(),
            set.coverage.clone(),
            set.independent_clusters.clone(),
        )?;
        if fingerprint(&rebuilt)? != fingerprint(set)? {
            return Err(Error::Conflict("import evidence identity mismatch".into()));
        }
        for member in &set.members {
            let source = sources
                .iter()
                .find(|source| source.id == member.source_id)
                .ok_or_else(|| Error::Conflict("import evidence source binding mismatch".into()))?;
            if !members.insert(member.source_id.as_str())
                || source.payload.status != ImportSourceReadStatus::Ready
                || member.content_digest != source.payload.raw_digest
                || member.purpose != selection.payload.purpose
                || member.task_origin != TaskOrigin::ImportedHistory
                || member.execution_attestation != ExecutionAttestation::UnverifiedImport
            {
                return Err(Error::Conflict(
                    "import evidence source binding mismatch".into(),
                ));
            }
        }
    }
    for locator in locators {
        if !members.contains(locator.source_id.as_str()) {
            return Err(Error::Conflict(
                "import locator source binding mismatch".into(),
            ));
        }
    }
    Ok(())
}

fn verify_import_evidence_bytes(result: &ResultCore, source_id: &str, raw: &[u8]) -> Result<()> {
    if let Some(set) = result.evidence_set {
        for member in set
            .members
            .iter()
            .filter(|member| member.source_id == source_id)
        {
            evo_core::evidence::verify_evidence_member_content_v3(member, raw)?;
        }
    }
    for locator in result
        .locators
        .iter()
        .filter(|locator| locator.source_id == source_id)
    {
        evo_core::evidence::verify_evidence_locator_content_v3(locator, raw)?;
        let mut events = result.event_records.iter().filter(|event| {
            event.source_id == source_id && event.event_index == locator.event_index
        });
        let event = events
            .next()
            .ok_or_else(|| Error::Conflict("import locator event binding mismatch".into()))?;
        if events.next().is_some()
            || locator.byte_start != event.byte_start
            || locator.byte_end > event.byte_end
        {
            return Err(Error::Conflict(
                "import locator event binding mismatch".into(),
            ));
        }
    }
    Ok(())
}

fn validate_result_record(ctx: &Context, stored: &StoredImportResult) -> Result<()> {
    match stored {
        StoredImportResult::V2(record) => {
            validate_envelope(ctx, record, IMPORT_RESULT_SCHEMA_V2)?;
            if record.payload.reader_version != IMPORT_READER_SET_V2
                || record.payload.normalization_version != IMPORT_NORMALIZATION_V2
            {
                return Err(Error::Conflict("import_result_invalid".into()));
            }
        }
        StoredImportResult::LegacyV1(record) => {
            validate_envelope(ctx, record, LEGACY_IMPORT_RESULT_SCHEMA_V1)?;
        }
    }
    let record = stored.core();
    if record.generation_status != "blocked_external_generation_conditions_unavailable"
        || record.evidence_set.is_some_and(|set| {
            set.members.iter().any(|member| {
                member.task_origin != TaskOrigin::ImportedHistory
                    || member.execution_attestation != ExecutionAttestation::UnverifiedImport
            })
        })
        || (record.state == ImportResultState::Failed && record.evidence_set.is_some())
        || (record.state != ImportResultState::Failed && record.evidence_set.is_none())
    {
        return Err(Error::Conflict("invalid persisted import result".into()));
    }
    Ok(())
}

fn validate_envelope<T>(
    ctx: &Context,
    record: &ImportEnvelope<T>,
    expected_schema: &str,
) -> Result<()> {
    if record.schema_version != expected_schema
        || record.namespace != ctx.namespace()
        || record.owner_actor.is_empty()
        || record.created_at <= 0
        || record.updated_at < record.created_at
    {
        return Err(Error::Conflict("import envelope identity mismatch".into()));
    }
    identifier(&record.id)?;
    identifier(&record.owner_actor)?;
    identifier(&record.request_key)?;
    validate_digest(&record.input_digest)?;
    if ctx.role() == Role::Admin && record.owner_actor != ctx.actor() {
        return Err(Error::NotFound);
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid("invalid lowercase sha256 digest".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_codex_are_unsupported_not_empty() {
        assert!(
            detect_format("codex")
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        assert!(detect_format("mystery").is_err());
        assert!(parse_fixture(SourceFormat::ClaudeFixture, "").is_err());
    }

    #[test]
    fn import_is_unverified() {
        let ev = parse_fixture(
            SourceFormat::RsiaTraceV1,
            include_str!("../../../fixtures/imports/rsia.trace.v1.reader-golden.json"),
        )
        .unwrap();
        assert_eq!(ev[0].attestation, ExecutionAttestation::UnverifiedImport);
        assert_eq!(ev[0].task_origin, TaskOrigin::ImportedHistory);
        assert!(tool_result_is_not_preference("user", "chat"));
        assert!(!tool_result_is_not_preference("user", "tool_result"));
        assert!(!tool_result_is_not_preference("user", "command_echo"));
        for role in ["User", " user", "assistant", "tool", ""] {
            assert!(
                !tool_result_is_not_preference(role, "tool_result"),
                "{role:?}"
            );
            assert!(!tool_result_is_not_preference(role, "summary"), "{role:?}");
        }
        assert!(matches!(
            parse_fixture(SourceFormat::ClaudeFixture, r#"{"role":"user"}"#),
            Err(Error::Invalid(message)) if message == "content must be a string"
        ));
    }

    #[test]
    fn cluster_id_groups_same_incident_retries() {
        assert_eq!(extract_cluster_id("incident_a.try1"), "incident_a");
        assert_eq!(extract_cluster_id("incident_a.try2"), "incident_a");
        assert_eq!(extract_cluster_id("task_1:fork_1"), "task_1");
        assert_eq!(extract_cluster_id("standalone_job"), "standalone_job");
        assert_eq!(extract_cluster_id("incident_q.try1:fork2"), "incident_q");
        assert_eq!(extract_cluster_id("incident_q:try1.retry2"), "incident_q");
    }

    #[test]
    fn clusters_are_the_closure_of_incident_and_content_relations() {
        let d = |s: &str| hash(s.as_bytes());
        let inputs = vec![
            ("incident_1.try1".to_string(), d("unique")),
            ("incident_1.try2".to_string(), d("shared")),
            ("other.run".to_string(), d("shared")),
            ("third.run".to_string(), d("third")),
        ];
        let clusters = independent_clusters(&inputs);
        assert_eq!(clusters.len(), 2, "{clusters:?}");
        assert!(clusters.contains("third"));
        assert!(clusters.contains(&format!("content_{}", &d("shared")[..16])));
        let plain = independent_clusters(&[
            ("a.try1".to_string(), d("x")),
            ("a.try2".to_string(), d("y")),
            ("b.run".to_string(), d("z")),
        ]);
        assert_eq!(plain, BTreeSet::from(["a".to_string(), "b".to_string()]));
        assert!(independent_clusters(&[]).is_empty());
    }
}

#[cfg(test)]
mod local_material_boundary_tests {
    use super::*;

    async fn fixture() -> (
        tempfile::TempDir,
        Store,
        Context,
        PersistentImportService,
        SourceSelectionRecord,
        ImportResultRecord,
        ImportMaterialReadRequest,
    ) {
        fixture_with_excerpts(false).await
    }

    async fn fixture_with_excerpts(
        allow_excerpts: bool,
    ) -> (
        tempfile::TempDir,
        Store,
        Context,
        PersistentImportService,
        SourceSelectionRecord,
        ImportResultRecord,
        ImportMaterialReadRequest,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let admin = Context::new("boundary-tenant", "admin", Role::Admin).unwrap();
        let store = Store::open(&directory.path().join("import.sqlite3"))
            .await
            .unwrap();
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&admin, &hash(b"boundary watermark"))
            .await
            .unwrap();
        session.commit().await.unwrap();
        let bytes = br#"{"role":"user","content":"boundary observation"}"#;
        let path = directory.path().join("history.jsonl");
        tokio::fs::write(&path, bytes).await.unwrap();
        let service = PersistentImportService::new(store.clone());
        let selection = service
            .register(
                &admin,
                ImportRegistrationRequest {
                    schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                    request_key: "boundary-registration".into(),
                    roots: vec![directory.path().to_string_lossy().into_owned()],
                    purpose: Purpose::Development,
                    allow_model_excerpts: allow_excerpts,
                    outbound_authorized: allow_excerpts,
                    retention_scope: if allow_excerpts {
                        ImportRetentionScope::LocalWithAuthorizedExcerpts
                    } else {
                        ImportRetentionScope::LocalPrivate
                    },
                    sources: vec![ImportSourceSpec {
                        source_id: "incident.try1".into(),
                        path: path.to_string_lossy().into_owned(),
                        reader: SourceFormat::ClaudeFixture,
                        expected_digest: hash(bytes),
                    }],
                },
            )
            .await
            .unwrap();
        let result = service.execute(&admin, &selection.id).await.unwrap();
        let request = ImportMaterialReadRequest {
            result: import_artifact_ref(ImportArtifactSchema::ImportResult, &result).unwrap(),
            events: vec![evo_core::evidence::ImportEventKey {
                source_id: selection.payload.import_source_ids[0].clone(),
                event_index: 0,
            }],
        };
        (directory, store, admin, service, selection, result, request)
    }

    #[tokio::test]
    async fn live_reader_rejects_wrong_member_preimage_after_healthy_import() {
        let (_directory, store, admin, service, _selection, result, _request) = fixture().await;
        let healthy = service.load_live_result(&admin, &result.id).await.unwrap();
        assert_eq!(
            fingerprint(&healthy).unwrap(),
            fingerprint(&result).unwrap()
        );
        let mut changed = result.clone();
        changed.payload.evidence_set.as_mut().unwrap().members[0].content_digest =
            hash(br#"{"role":"user","content":"another valid observation"}"#);
        // Keep the set internally well-formed; only its source binding is wrong.
        let set = changed.payload.evidence_set.as_mut().unwrap();
        *set = EvidenceSet::build(
            set.id.clone(),
            set.members.clone(),
            set.coverage.clone(),
            set.independent_clusters.clone(),
        )
        .unwrap();
        let mut session = store.session().await.unwrap();
        session
            .put(
                &admin,
                IMPORT_ARTIFACT_KIND,
                &changed.id,
                admin.actor(),
                &changed,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert!(matches!(
            service.load_live_result(&admin, &result.id).await,
            Err(Error::Conflict(message)) if message == "import evidence source binding mismatch"
        ));
        store.close().await;
    }

    #[tokio::test]
    async fn live_reader_keeps_locator_digest_and_range_failures_after_healthy_import() {
        for fault in 0..4 {
            let (_directory, store, admin, service, _selection, result, _request) =
                fixture_with_excerpts(true).await;
            let healthy = service.load_live_result(&admin, &result.id).await.unwrap();
            assert_eq!(
                fingerprint(&healthy).unwrap(),
                fingerprint(&result).unwrap()
            );
            assert!(!result.payload.locators.is_empty());
            let mut changed = result.clone();
            let locator = &mut changed.payload.locators[0];
            match fault {
                0 => locator.source_digest = hash(br#"{"role":"user","content":"other"}"#),
                1 => locator.excerpt_digest = hash(b"another actual excerpt"),
                2 => locator.byte_end += 1,
                3 => locator.excerpt_digest = "readable_excerpt".into(),
                _ => unreachable!(),
            }
            let mut session = store.session().await.unwrap();
            session
                .put(
                    &admin,
                    IMPORT_ARTIFACT_KIND,
                    &changed.id,
                    admin.actor(),
                    &changed,
                )
                .await
                .unwrap();
            session.commit().await.unwrap();
            let error = service
                .load_live_result(&admin, &result.id)
                .await
                .unwrap_err();
            match fault {
                0 => assert!(
                    matches!(error, Error::Conflict(message) if message == "source_changed")
                ),
                1 => assert!(
                    matches!(error, Error::Conflict(message) if message == "excerpt_digest_mismatch")
                ),
                2 => assert!(
                    matches!(error, Error::Invalid(message) if message == "offset out of bounds")
                ),
                3 => assert!(matches!(error, Error::Invalid(_))),
                _ => unreachable!(),
            }
            store.close().await;
        }
    }

    #[tokio::test]
    async fn material_boundary_refuses_committed_revoke_and_watermark_changes() {
        let (_directory, store, admin, service, selection, result, request) = fixture().await;
        let boundary = async {
            let status = evo_storage::lifecycle::LifecycleStore::begin_revoke(
                &admin,
                &store,
                evo_storage::lifecycle::TypedObjectRef {
                    kind: "artifact".into(),
                    id: selection.payload.import_source_ids[0].clone(),
                },
                "revoked after byte reconstruction",
                10,
            )
            .await?;
            assert_eq!(status.state, evo_storage::lifecycle::CleanupState::Pending);
            Ok(())
        };
        assert!(
            service
                .read_local_material_at_boundary(&admin, &request, boundary)
                .await
                .is_err()
        );
        assert_eq!(
            result.payload.generation_status,
            "blocked_external_generation_conditions_unavailable"
        );
    }

    #[tokio::test]
    async fn material_boundary_reloads_result_and_unselected_source_after_reconstruction() {
        for change_result in [true, false] {
            let (_directory, store, admin, service, selection, result, mut request) =
                fixture().await;
            request.events.clear(); // Every source is unselected in this boundary test.
            let boundary = async {
                let mut session = store.session().await?;
                if change_result {
                    let mut value: Value = session.need(&admin, "artifact", &result.id).await?;
                    value["updated_at"] = (result.updated_at + 1).into();
                    session
                        .put(&admin, "artifact", &result.id, admin.actor(), &value)
                        .await?;
                } else {
                    let id = &selection.payload.import_source_ids[0];
                    let mut value: Value = session.need(&admin, "artifact", id).await?;
                    value["updated_at"] = (selection.updated_at + 1).into();
                    session
                        .put(&admin, "artifact", id, admin.actor(), &value)
                        .await?;
                }
                session.commit().await
            };
            assert!(
                service
                    .read_local_material_at_boundary(&admin, &request, boundary)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn material_boundary_refuses_committed_upstream_tombstone_without_watermark_change() {
        let (_directory, store, admin, service, selection, _result, request) = fixture().await;
        let boundary = async {
            let mut session = store.session().await?;
            let upstream = "upstream-gate-fixture";
            session
                .put(
                    &admin,
                    "run",
                    upstream,
                    admin.actor(),
                    &serde_json::json!({"id":upstream,"body":"gate fixture only"}),
                )
                .await?;
            session
                .put_edge(
                    &admin,
                    "artifact",
                    &selection.payload.import_source_ids[0],
                    "run",
                    upstream,
                )
                .await?;
            session
                .put(
                    &admin,
                    "tombstone",
                    upstream,
                    admin.actor(),
                    &serde_json::json!({"id":upstream,"source_kind":"run"}),
                )
                .await?;
            session.commit().await
        };
        assert!(matches!(
            service
                .read_local_material_at_boundary(&admin, &request, boundary)
                .await,
            Err(Error::Forbidden)
        ));
    }
    #[tokio::test]
    async fn q03_import_live_read_blocks_transitive_source() {
        let (_directory, store, admin, service, selection, _result, request) = fixture().await;
        let host = Context::new(admin.namespace(), "q03-host", Role::Host).unwrap();
        let id = "q03-upstream";
        crate::evidence::store_trace_authority(
            &store,
            &host,
            &crate::evidence::StoredTraceAuthority {
                schema_version: "rsia.optimization.source.v1".into(),
                record: crate::evidence::StoredRunRecord {
                    id: id.into(),
                    body: id.as_bytes().to_vec(),
                    parent_family: "family".into(),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                trace: evo_core::optimization::OptimizationTrace {
                    run_id: id.into(),
                    parent_family: "family".into(),
                    source_digest: hash(id.as_bytes()),
                    purpose: Purpose::Development,
                    outcome: evo_core::optimization::TraceOutcome::Success,
                    diagnosis: None,
                    excerpt: id.into(),
                    seed: 1,
                },
                excerpt_start: 0,
                excerpt_end: id.len(),
            },
        )
        .await
        .unwrap();
        let mut session = store.session().await.unwrap();
        session
            .put(
                &admin,
                "artifact",
                "q03-middle",
                admin.actor(),
                &serde_json::json!({"id":"q03-middle"}),
            )
            .await
            .unwrap();
        session
            .put_edge(
                &admin,
                "artifact",
                &selection.payload.import_source_ids[0],
                "artifact",
                "q03-middle",
            )
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", "q03-middle", "run", id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        service
            .read_local_development_material(&admin, &request)
            .await
            .unwrap();
        let boundary = async {
            let mut session = store.session().await?;
            // Deliberately corrupt, committed raw tombstone: the other physical
            // kind does not excuse an invalid identity or skip its upstream graph.
            session.put(&admin, "tombstone", id, admin.actor(), &serde_json::json!({
                "id":id,"schema_version":"rsia.revoke_tombstone.v1","source_kind":"artifact",
                "reason":"q03-corrupt-fixture","watermark_seq":0,"watermark_digest":hash(b"watermark"),"created_at":1,
            })).await?;
            session.commit().await
        };
        assert!(matches!(
            service
                .read_local_material_at_boundary(&admin, &request, boundary)
                .await,
            Err(Error::Forbidden)
        ));
    }
}
