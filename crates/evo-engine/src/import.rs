//! Versioned history readers. Import is not a trusted execution identity.
use evo_core::evidence::{ExecutionAttestation, TaskOrigin};
use evo_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    RsiaTraceV1,
    RsihPiFixture,
    ClaudeFixture,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedEvent {
    pub origin_name: String,
    pub role: String,
    pub format: SourceFormat,
    pub task_origin: TaskOrigin,
    pub attestation: ExecutionAttestation,
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

pub fn parse_fixture(format: SourceFormat, body: &str) -> Result<Vec<ImportedEvent>> {
    if body.trim().is_empty() {
        return Err(Error::Invalid(
            "zero records is not empty history success".into(),
        ));
    }
    if body.contains("\u{0000}") {
        return Err(Error::Invalid("parse_error".into()));
    }
    Ok(vec![ImportedEvent {
        origin_name: "user".into(),
        role: "user".into(),
        format,
        task_origin: TaskOrigin::ImportedHistory,
        attestation: ExecutionAttestation::UnverifiedImport,
    }])
}

pub fn tool_result_is_not_preference(role: &str, kind: &str) -> bool {
    !(role == "user" && (kind == "tool_result" || kind == "command_echo"))
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
        assert!(tool_result_is_not_preference("user", "chat"));
        assert!(!tool_result_is_not_preference("user", "tool_result"));
    }
}
