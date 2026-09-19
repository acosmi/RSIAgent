//! Versioned history readers. Import is not a trusted execution identity.
//! Strict implementation of v4.1 §6.3, §5.6, §13.1 E16.1.
use evo_core::evidence::{
    AggregateSummary, EvidenceLocator, EvidenceMember, EvidenceSet, ExecutionAttestation,
    MAX_DISCOVERED_FILES, MAX_EVENT_BYTES, MAX_EXCERPTS, MAX_HEADER_PROBE_BYTES,
    MAX_TOTAL_EXCERPT_BYTES, MAX_TOTAL_READ_BYTES, SourceCoverage, SourceSelection, TaskOrigin,
    assert_authorized_path,
};
use evo_core::{Error, Result, identifier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedEvent {
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

pub fn find_json_array_element_spans(text: &str, array_key: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let key_pattern = format!("\"{}\"", array_key);
    let key_pos = match text.find(&key_pattern) {
        Some(pos) => pos + key_pattern.len(),
        None => return spans,
    };
    let array_start = match text[key_pos..].find('[') {
        Some(pos) => key_pos + pos + 1,
        None => return spans,
    };

    let bytes = text.as_bytes();
    let mut idx = array_start;
    let len = bytes.len();
    let mut in_string = false;
    let mut escape = false;
    let mut depth = 0;
    let mut elem_start = 0;

    while idx < len {
        let b = bytes[idx];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' => {
                    if depth == 0 {
                        elem_start = idx;
                    }
                    depth += 1;
                }
                b'}' => {
                    if depth > 0 {
                        depth -= 1;
                        if depth == 0 {
                            spans.push((elem_start, idx + 1));
                        }
                    }
                }
                b']' if depth == 0 => break,
                _ => {}
            }
        }
        idx += 1;
    }
    spans
}

pub fn find_top_level_json_array_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let array_start = match text.find('[') {
        Some(pos) => pos + 1,
        None => return spans,
    };
    let bytes = text.as_bytes();
    let mut idx = array_start;
    let len = bytes.len();
    let mut in_string = false;
    let mut escape = false;
    let mut depth = 0;
    let mut elem_start = 0;

    while idx < len {
        let b = bytes[idx];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' => {
                    if depth == 0 {
                        elem_start = idx;
                    }
                    depth += 1;
                }
                b'}' => {
                    if depth > 0 {
                        depth -= 1;
                        if depth == 0 {
                            spans.push((elem_start, idx + 1));
                        }
                    }
                }
                b']' if depth == 0 => break,
                _ => {}
            }
        }
        idx += 1;
    }
    spans
}

pub fn detect_format_from_bytes(bytes: &[u8]) -> Result<SourceFormat> {
    let s = std::str::from_utf8(bytes).map_err(|_| Error::Invalid("invalid_utf8".into()))?;
    let mut probe_len = s.len().min(MAX_HEADER_PROBE_BYTES);
    while !s.is_char_boundary(probe_len) {
        probe_len -= 1;
    }
    let probe = &s[..probe_len];

    // Structured JSON parsing on probe slice
    let first_val = serde_json::Deserializer::from_str(probe)
        .into_iter::<serde_json::Value>()
        .next()
        .and_then(|r| r.ok());

    if let Some(val) = first_val {
        if let Some(obj) = val.as_object() {
            // Check schema_version first
            if let Some(ver) = obj.get("schema_version").and_then(|v| v.as_str()) {
                match ver {
                    "rsia.trace.v1" | "rsia.optimization.source.v1" => {
                        return Ok(SourceFormat::RsiaTraceV1);
                    }
                    "rsih.pi.fixture" => return Ok(SourceFormat::RsihPiFixture),
                    "claude.fixture" => return Ok(SourceFormat::ClaudeFixture),
                    v if v.contains("codex") => {
                        return Err(Error::Invalid("unsupported_format:codex".into()));
                    }
                    other => {
                        return Err(Error::Invalid(format!("unknown schema_version: {}", other)));
                    }
                }
            }

            // Check format field
            if let Some(fmt) = obj.get("format").and_then(|v| v.as_str()) {
                match fmt {
                    "rsia.trace.v1" | "rsia.optimization.source.v1" => {
                        return Ok(SourceFormat::RsiaTraceV1);
                    }
                    "rsih.pi.fixture" => return Ok(SourceFormat::RsihPiFixture),
                    "claude.fixture" => return Ok(SourceFormat::ClaudeFixture),
                    v if v.contains("codex") => {
                        return Err(Error::Invalid("unsupported_format:codex".into()));
                    }
                    other => return Err(Error::Invalid(format!("unsupported_format: {}", other))),
                }
            }

            // Check product or origin field
            if obj
                .get("product")
                .or_else(|| obj.get("origin"))
                .and_then(|v| v.as_str())
                .is_some_and(|p| p.contains("codex"))
            {
                return Err(Error::Invalid("unsupported_format:codex".into()));
            }

            // If it has "events" array without explicit allowed schema_version, reject
            if obj.contains_key("events") {
                return Err(Error::Invalid(
                    "missing or invalid schema_version for rsia.trace".into(),
                ));
            }

            // If it has "prompts" array, it is RSIH Pi
            if obj.contains_key("prompts") {
                return Ok(SourceFormat::RsihPiFixture);
            }

            // If it has "role" and ("content" or "text")
            if obj.contains_key("role") && (obj.contains_key("content") || obj.contains_key("text"))
            {
                if obj.contains_key("kind") {
                    return Ok(SourceFormat::RsiaTraceV1);
                }
                return Ok(SourceFormat::ClaudeFixture);
            }
        } else if let Some(first_elem) = val
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_object())
        {
            if first_elem.contains_key("text") || first_elem.contains_key("prompts") {
                return Ok(SourceFormat::RsihPiFixture);
            }
            if first_elem.contains_key("kind") {
                return Ok(SourceFormat::RsiaTraceV1);
            }
            if first_elem.contains_key("role") {
                return Ok(SourceFormat::ClaudeFixture);
            }
        }
    }

    Err(Error::Invalid("unsupported_format".into()))
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

    match format {
        SourceFormat::RsihPiFixture => parse_rsih_pi(body, limits),
        SourceFormat::RsiaTraceV1 => parse_rsia_trace(body, limits),
        SourceFormat::ClaudeFixture => parse_claude_fixture(body, limits),
    }
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
            let (byte_start, byte_end) = spans.get(idx).copied().unwrap_or((0, body.len()));
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
            });
        }
    } else if let Some(arr) = parsed.as_array() {
        let spans = find_top_level_json_array_spans(body);
        for (idx, item) in arr.iter().enumerate() {
            let (byte_start, byte_end) = spans.get(idx).copied().unwrap_or((0, body.len()));
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
                let (byte_start, byte_end) = spans.get(idx).copied().unwrap_or((0, body.len()));
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

/// Ingests authorized external history sources under §6.3 resource bounds and security policies.
pub fn ingest_imported_sources(
    selection: &SourceSelection,
    sources: &[(&str, &[u8])],
    limits: Option<ImportForensicLimits>,
) -> Result<IngestResult> {
    selection.validate()?;
    let limits = limits.unwrap_or_default();

    if sources.is_empty() {
        return Err(Error::Invalid("source selection is empty".into()));
    }

    // Path authorization & rejection of home scan
    for (path, _) in sources {
        if *path == "/" || *path == std::env::var("HOME").unwrap_or_default() {
            return Err(Error::Forbidden);
        }
        assert_authorized_path(path, &selection.roots)?;
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
    let mut clusters = BTreeSet::new();
    let mut counter_examples = Vec::new();
    let mut tool_calls_count = 0;
    let mut failure_count = 0;
    let mut seen_sources = BTreeSet::new();

    for &(source_name, raw_bytes) in &sources[..count_to_process] {
        let member_id = if identifier(source_name).is_ok() {
            source_name.to_string()
        } else {
            // Path with slashes: already passed assert_authorized_path above; derive stable identifier
            format!("imp_{}", &evo_core::hash(source_name.as_bytes())[..16])
        };

        if !seen_sources.insert(member_id.clone()) {
            return Err(Error::Conflict("duplicate source record".into()));
        }

        if coverage.bytes_read + raw_bytes.len() as u64 > limits.max_total_read_bytes as u64 {
            coverage.event_truncated += 1;
            coverage.outbound_truncated += 1;
            break;
        }

        if raw_bytes.is_empty() || raw_bytes.iter().all(|b| b.is_ascii_whitespace()) {
            coverage.zero_records += 1;
            continue;
        }

        let format = match detect_format_from_bytes(raw_bytes) {
            Ok(fmt) => fmt,
            Err(_) => {
                coverage.unsupported_format += 1;
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
        coverage.bytes_read += raw_bytes.len() as u64;

        let content_digest = evo_core::hash(raw_bytes);

        for mut ev in parsed_events {
            // Unconditionally separate 3 trust axes (V005)
            ev.task_origin = TaskOrigin::ImportedHistory;
            ev.attestation = ExecutionAttestation::UnverifiedImport;

            if ev.content.len() > limits.max_event_bytes {
                coverage.char_truncated += 1;
                coverage.event_truncated += 1;
                let mut trunc_len = limits.max_event_bytes;
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
                let max_len = (ev.byte_end - ev.byte_start).min(limits.max_total_excerpt_bytes);
                let excerpt_start = ev.byte_start;
                let excerpt_end = ev.byte_start + max_len;
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
                }
            }

            all_events.push(ev);
        }

        let cluster_id = extract_cluster_id(&member_id);
        clusters.insert(cluster_id);

        members.push(EvidenceMember {
            source_id: member_id,
            content_digest,
            task_origin: TaskOrigin::ImportedHistory,
            execution_attestation: ExecutionAttestation::UnverifiedImport,
            purpose: selection.purpose,
        });
    }

    if members.is_empty() {
        return Err(Error::Invalid("no valid imported sources ingested".into()));
    }

    let summary = AggregateSummary {
        total_sources: members.len(),
        total_events: all_events.len(),
        tool_calls_count,
        failure_count,
        unique_clusters: clusters.len(),
        counter_examples,
        coverage: coverage.clone(),
    };

    let raw_set_id = format!(
        "es_imp_{}",
        &evo_core::hash(format!("{:?}", selection).as_bytes())[..12]
    );
    let evidence_set = EvidenceSet::build(raw_set_id, members, coverage, clusters)?;

    Ok(IngestResult {
        evidence_set,
        aggregate_summary: summary,
        events: all_events,
        locators: all_locators,
    })
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
