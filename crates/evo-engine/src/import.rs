//! Versioned history readers. Import is not a trusted execution identity.
//! Strict implementation of v4.1 §6.3, §5.6, §13.1 E16.1.
use evo_core::evidence::{
    AggregateSummary, EvidenceLocator, EvidenceMember, EvidenceSet, ExecutionAttestation,
    MAX_DISCOVERED_FILES, MAX_EVENT_BYTES, MAX_EXCERPTS, MAX_HEADER_PROBE_BYTES,
    MAX_TOTAL_EXCERPT_BYTES, MAX_TOTAL_READ_BYTES, SourceCoverage, SourceSelection, TaskOrigin,
    assert_authorized_path,
};
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier, now};
use evo_storage::Store;
use serde::{Deserialize, Serialize};
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

fn format_from_discriminator(value: &str) -> Result<SourceFormat> {
    match value {
        "rsia.trace.v1" | "rsia.optimization.source.v1" => Ok(SourceFormat::RsiaTraceV1),
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

pub fn tool_result_is_not_preference(role: &str, kind: &str) -> bool {
    !(role == "user" && (kind == "tool_result" || kind == "command_echo" || kind == "summary"))
}

pub fn extract_cluster_id(source_id: &str) -> String {
    if let Some((prefix, _)) = source_id.split_once(':') {
        prefix.to_string()
    } else if let Some((prefix, _)) = source_id.split_once('.') {
        prefix.to_string()
    } else {
        source_id.to_string()
    }
}

pub fn parse_fixture(format: SourceFormat, body: &str) -> Result<Vec<ImportedEvent>> {
    parse_fixture_with_limits(format, body, &ImportForensicLimits::default())
}

pub fn parse_fixture_with_limits(
    format: SourceFormat,
    body: &str,
    limits: &ImportForensicLimits,
) -> Result<Vec<ImportedEvent>> {
    if body.trim().is_empty() {
        return Err(Error::Invalid(
            "zero records is not empty history success".into(),
        ));
    }
    if body.contains('\0') {
        return Err(Error::Invalid("parse_error".into()));
    }
    validate_strict_json_input(format, body)?;
    validate_reader_shape(format, body)?;

    match format {
        SourceFormat::RsihPiFixture => parse_rsih_pi(body, limits),
        SourceFormat::RsiaTraceV1 => parse_rsia_trace(body, limits),
        SourceFormat::ClaudeFixture => parse_claude_fixture(body, limits),
    }
}

fn validate_reader_shape(format: SourceFormat, body: &str) -> Result<()> {
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

fn validate_jsonl_shapes(body: &str, fields: &[&str], format: SourceFormat) -> Result<()> {
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value =
            serde_json::from_str(line.trim()).map_err(|_| Error::Invalid("parse_error".into()))?;
        validate_value_discriminator(&value, format)?;
        validate_event_shape(&value, fields)?;
    }
    Ok(())
}

fn validate_value_discriminator(value: &Value, expected: SourceFormat) -> Result<()> {
    for field in ["schema_version", "format"] {
        if let Some(discriminator) = validate_optional_string(value, field)?
            && format_from_discriminator(discriminator)? != expected
        {
            return Err(Error::Conflict("format discriminator mismatch".into()));
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

fn parse_rsia_trace(body: &str, _limits: &ImportForensicLimits) -> Result<Vec<ImportedEvent>> {
    // Check if JSON object or JSONL
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(schema_ver) = parsed
            .get("schema_version")
            .and_then(|v| v.as_str())
            .filter(|&ver| ver != "rsia.trace.v1" && ver != "rsia.optimization.source.v1")
        {
            return Err(Error::Invalid(format!(
                "unknown schema_version: {}",
                schema_ver
            )));
        }
        if let Some(events_arr) = parsed.get("events").and_then(|v| v.as_array()) {
            if parsed.get("schema_version").is_none() {
                return Err(Error::Invalid(
                    "missing schema_version for rsia.trace".into(),
                ));
            }
            let spans = find_json_array_element_spans(body, "events");
            let mut events = Vec::new();
            for (idx, item) in events_arr.iter().enumerate() {
                let (byte_start, byte_end) = spans
                    .get(idx)
                    .copied()
                    .ok_or_else(|| Error::Invalid("event_locator_mismatch".into()))?;
                let role = item
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user")
                    .to_string();
                let kind = item
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("chat")
                    .to_string();
                let content_raw = item
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let content = content_raw.to_string();

                events.push(ImportedEvent {
                    source_id: String::new(),
                    origin_name: "rsia_trace".into(),
                    role,
                    format: SourceFormat::RsiaTraceV1,
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
            if !events.is_empty() {
                return Ok(events);
            }
        } else if parsed.is_object() {
            if let Some(schema_ver) = parsed
                .get("schema_version")
                .and_then(|v| v.as_str())
                .filter(|&ver| ver != "rsia.trace.v1" && ver != "rsia.optimization.source.v1")
            {
                return Err(Error::Invalid(format!(
                    "unknown schema_version: {}",
                    schema_ver
                )));
            }
            // Single object fixture fallback
            let role = parsed
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let content = parsed
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or(body);
            let kind = parsed
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("chat")
                .to_string();
            return Ok(vec![ImportedEvent {
                source_id: String::new(),
                origin_name: "rsia_trace".into(),
                role,
                format: SourceFormat::RsiaTraceV1,
                task_origin: TaskOrigin::ImportedHistory,
                attestation: ExecutionAttestation::UnverifiedImport,
                kind,
                content: content.to_string(),
                event_index: 0,
                byte_start: 0,
                byte_end: body.len(),
                raw_fragment_digest: String::new(),
            }]);
        }
    }

    // Line-delimited JSONL
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
        if let Some(schema_ver) = item
            .get("schema_version")
            .and_then(|v| v.as_str())
            .filter(|&ver| ver != "rsia.trace.v1" && ver != "rsia.optimization.source.v1")
        {
            return Err(Error::Invalid(format!(
                "unknown schema_version: {}",
                schema_ver
            )));
        }
        let role = item
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("user")
            .to_string();
        let kind = item
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("chat")
            .to_string();
        let content_raw = item
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let content = content_raw.to_string();

        events.push(ImportedEvent {
            source_id: String::new(),
            origin_name: "rsia_trace".into(),
            role,
            format: SourceFormat::RsiaTraceV1,
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
        } else if role == "user"
            && item
                .get("content")
                .and_then(|v| v.as_str())
                .is_some_and(|c| c.contains("tool_result"))
        {
            "tool_result".to_string()
        } else {
            "chat".to_string()
        };

        let content_raw = if let Some(c) = item.get("content").and_then(|v| v.as_str()) {
            c.to_string()
        } else if let Some(c) = item.get("content") {
            c.to_string()
        } else {
            String::new()
        };

        let content = content_raw;

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

    // Path authorization & rejection of home scan
    for source in sources {
        if source.source_name == "/"
            || source.source_name == std::env::var("HOME").unwrap_or_default()
        {
            return Err(Error::Forbidden);
        }
        assert_authorized_path(source.source_name, &selection.roots)?;
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

        let parsed_events = match parse_fixture_with_limits(format, body, &limits) {
            Ok(evs) => evs,
            Err(_) => {
                coverage.parsed_fail += 1;
                continue;
            }
        };

        if parsed_events.is_empty() {
            coverage.zero_records += 1;
            continue;
        }

        coverage.parsed_ok += 1;

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
                if let Ok(loc) = EvidenceLocator::build(
                    &member_id,
                    &content_digest,
                    excerpt_start,
                    excerpt_end,
                    ev.event_index,
                    &excerpt_digest,
                ) {
                    all_locators.push(loc);
                    excerpt_bytes_used += excerpt.len();
                    if excerpt_end < ev.byte_end {
                        coverage.excerpt_truncated += 1;
                    }
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

        members.push(EvidenceMember {
            source_id: member_id,
            content_digest,
            task_origin: TaskOrigin::ImportedHistory,
            execution_attestation: ExecutionAttestation::UnverifiedImport,
            purpose: selection.purpose,
        });
    }

    let mut digest_counts = BTreeMap::new();
    for (_, digest) in &cluster_inputs {
        *digest_counts.entry(digest.as_str()).or_insert(0usize) += 1;
    }
    let clusters: BTreeSet<_> = cluster_inputs
        .iter()
        .map(|(hint, digest)| {
            if digest_counts.get(digest.as_str()).copied().unwrap_or(0) > 1 {
                format!("content_{}", &digest[..16])
            } else if hint.contains('.') || hint.contains(':') {
                extract_cluster_id(hint)
            } else {
                "unverified_import_unknown".into()
            }
        })
        .collect();
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
    })
}

pub const IMPORT_REGISTRATION_SCHEMA: &str = "rsia.e16.import_registration.v1";
pub const SOURCE_SELECTION_SCHEMA: &str = "rsia.e16.source_selection.v1";
pub const IMPORT_SOURCE_SCHEMA: &str = "rsia.e16.import_source.v1";
pub const IMPORT_RESULT_SCHEMA: &str = "rsia.e16.import_result.v1";
const IMPORT_ARTIFACT_KIND: &str = "artifact";
const IMPORT_EVENT_CAPACITY: u64 = 10_000;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportResultPayload {
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

#[derive(Clone)]
pub struct PersistentImportService {
    store: Store,
    operation_lock: Arc<Mutex<()>>,
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
            let timestamp = now();
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
        finalized_selection.updated_at = now();
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
            || session
                .get::<Value>(ctx, "tombstone", &current.id)
                .await?
                .is_some()
            || session
                .get::<Value>(ctx, "tombstone", &current.payload.selection_id)
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
        current.updated_at = now();
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
        let result_id = format!("e16res-{}", &hash(selection_id.as_bytes())[..24]);
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

        let (selection, source_records) = self.load_live_selection(ctx, selection_id).await?;
        let mut remaining = MAX_TOTAL_READ_BYTES;
        let mut ready_sources = Vec::new();
        let mut owned_bytes = Vec::new();
        let mut unavailable_coverage = SourceCoverage {
            discovery_exhausted: true,
            files_known: true,
            ..SourceCoverage::default()
        };
        for source in &source_records {
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
        let event_records: Vec<_> = ingest
            .events
            .iter()
            .map(|event| PersistedImportEvent {
                source_id: event.source_id.clone(),
                event_index: event.event_index,
                byte_start: event.byte_start,
                byte_end: event.byte_end,
                raw_fragment_digest: event.raw_fragment_digest.clone(),
                normalized_content_digest: hash(event.content.as_bytes()),
                role: event.role.clone(),
                kind: event.kind.clone(),
            })
            .collect();
        if ingest.aggregate_summary.total_events != event_records.len() {
            return Err(Error::Conflict("import event count mismatch".into()));
        }
        let mut source_refs = vec![ImportTypedRef {
            kind: "source_selection".into(),
            id: selection.id.clone(),
            content_digest: fingerprint(&selection)?,
        }];
        for record in &source_records {
            source_refs.push(ImportTypedRef {
                kind: "import_source".into(),
                id: record.id.clone(),
                content_digest: fingerprint(record)?,
            });
        }
        let timestamp = selection.created_at;
        let state = if ingest.evidence_set.is_none() {
            ImportResultState::Failed
        } else if ingest.aggregate_summary.coverage.complete() {
            ImportResultState::Complete
        } else {
            ImportResultState::Partial
        };
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
            payload: ImportResultPayload {
                selection_id: selection.id.clone(),
                state,
                evidence_set: ingest.evidence_set,
                aggregate_summary: ingest.aggregate_summary,
                locators: ingest.locators,
                event_records,
                generation_status: "blocked_external_generation_conditions_unavailable".into(),
            },
        };

        let mut session = self.store.session().await?;
        validate_live_import_sources(&mut session, ctx, &selection, &source_records).await?;
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
            .put_new_below_json_sum_capacity(
                ctx,
                IMPORT_ARTIFACT_KIND,
                &result.id,
                ctx.actor(),
                &result,
                IMPORT_RESULT_SCHEMA,
                "$.payload.aggregate_summary.total_events",
                result.payload.aggregate_summary.total_events as u64,
                IMPORT_EVENT_CAPACITY,
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

    pub async fn load_live_result(
        &self,
        ctx: &Context,
        result_id: &str,
    ) -> Result<ImportResultRecord> {
        ctx.require(&[Role::Admin, Role::Evaluator])?;
        identifier(result_id)?;
        let mut session = self.store.session().await?;
        let result: ImportResultRecord = session.need(ctx, IMPORT_ARTIFACT_KIND, result_id).await?;
        validate_result_record(ctx, &result)?;
        let selection: SourceSelectionRecord = session
            .need(ctx, IMPORT_ARTIFACT_KIND, &result.payload.selection_id)
            .await?;
        let source_records =
            load_and_validate_source_records(&mut session, ctx, &selection).await?;
        validate_live_import_sources(&mut session, ctx, &selection, &source_records).await?;
        let selection_digest = fingerprint(&selection)?;
        if result.revoke_watermark != selection.revoke_watermark
            || result.source_refs.first().is_none_or(|source| {
                source.kind != "source_selection"
                    || source.id != selection.id
                    || source.content_digest != selection_digest
            })
            || result.source_refs.len() != source_records.len() + 1
            || result.payload.aggregate_summary.total_events != result.payload.event_records.len()
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
            }
        }
        let mut session = self.store.session().await?;
        validate_live_import_sources(&mut session, ctx, &selection, &source_records).await?;
        session.commit().await?;
        Ok(result)
    }

    pub async fn read_event_fragment(
        &self,
        ctx: &Context,
        result_id: &str,
        source_id: &str,
        event_index: usize,
    ) -> Result<Vec<u8>> {
        ctx.require(&[Role::Admin])?;
        let result = self.load_live_result(ctx, result_id).await?;
        let event = result
            .payload
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
        self.load_live_result(ctx, result_id).await?;
        Ok(fragment)
    }

    async fn load_live_selection(
        &self,
        ctx: &Context,
        selection_id: &str,
    ) -> Result<(SourceSelectionRecord, Vec<ImportSourceRecord>)> {
        let mut session = self.store.session().await?;
        let selection: SourceSelectionRecord = session
            .need(ctx, IMPORT_ARTIFACT_KIND, selection_id)
            .await?;
        validate_selection_record(ctx, &selection)?;
        let sources = load_and_validate_source_records(&mut session, ctx, &selection).await?;
        validate_live_import_sources(&mut session, ctx, &selection, &sources).await?;
        session.commit().await?;
        Ok((selection, sources))
    }
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

async fn validate_live_import_sources(
    session: &mut evo_storage::Session,
    ctx: &Context,
    selection: &SourceSelectionRecord,
    sources: &[ImportSourceRecord],
) -> Result<()> {
    if current_watermark(session, ctx).await? != selection.revoke_watermark {
        return Err(Error::Conflict("source revoke watermark changed".into()));
    }
    for id in std::iter::once(selection.id.as_str())
        .chain(sources.iter().map(|source| source.id.as_str()))
    {
        if session.get::<Value>(ctx, "tombstone", id).await?.is_some() {
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

fn validate_result_record(ctx: &Context, record: &ImportResultRecord) -> Result<()> {
    validate_envelope(ctx, record, IMPORT_RESULT_SCHEMA)?;
    if record.payload.generation_status != "blocked_external_generation_conditions_unavailable"
        || record.payload.evidence_set.as_ref().is_some_and(|set| {
            set.members.iter().any(|member| {
                member.task_origin != TaskOrigin::ImportedHistory
                    || member.execution_attestation != ExecutionAttestation::UnverifiedImport
            })
        })
        || (record.payload.state == ImportResultState::Failed
            && record.payload.evidence_set.is_some())
        || (record.payload.state != ImportResultState::Failed
            && record.payload.evidence_set.is_none())
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
        let ev = parse_fixture(SourceFormat::RsiaTraceV1, "{}").unwrap();
        assert_eq!(ev[0].attestation, ExecutionAttestation::UnverifiedImport);
        assert_eq!(ev[0].task_origin, TaskOrigin::ImportedHistory);
        assert!(tool_result_is_not_preference("user", "chat"));
        assert!(!tool_result_is_not_preference("user", "tool_result"));
        assert!(!tool_result_is_not_preference("user", "command_echo"));
    }

    #[test]
    fn cluster_id_groups_same_incident_retries() {
        assert_eq!(extract_cluster_id("incident_a.try1"), "incident_a");
        assert_eq!(extract_cluster_id("incident_a.try2"), "incident_a");
        assert_eq!(extract_cluster_id("task_1:fork_1"), "task_1");
        assert_eq!(extract_cluster_id("standalone_job"), "standalone_job");
    }
}
