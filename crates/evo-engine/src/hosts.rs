//! Extra host adapters. Missing real hosts are blocked, not mocked.
use evo_core::{Error, Result};
use std::path::Path;

pub const CLAUDE_CODE_TARGET: &str = "claude-code-mcp-tool-only";

pub fn claude_code_support(binary: Option<&Path>) -> Result<&'static str> {
    match binary {
        Some(p) if p.is_file() => Ok("installed_but_not_yet_smoked"),
        _ => Err(Error::Invalid(
            "E16.4 blocked: Claude Code is not installed; not substituting a mock host".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn missing_claude_code_is_blocked() {
        assert!(claude_code_support(None).is_err());
        assert!(claude_code_support(Some(&PathBuf::from("/definitely/not/claude"))).is_err());
    }
}
