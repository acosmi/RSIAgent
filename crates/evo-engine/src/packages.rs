//! Asset package gates. Foreign approval is not local permission.
use evo_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

pub const MAX_FILES: usize = 100;
pub const MAX_TOTAL: u64 = 10 * 1024 * 1024;
pub const MAX_FILE: u64 = 1024 * 1024;
pub const MAX_ZIP_RATIO: u64 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageMember {
    pub path: String,
    pub size: u64,
    pub compressed: u64,
}

pub fn reject_member(m: &PackageMember) -> Result<()> {
    let p = Path::new(&m.path);
    if p.is_absolute() {
        return Err(Error::Invalid("absolute path".into()));
    }
    let mut norm = String::new();
    for c in p.components() {
        match c {
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(Error::Invalid("path traversal".into()));
            }
            Component::Normal(s) => {
                let s = s.to_string_lossy();
                if !norm.is_empty() {
                    norm.push('/');
                }
                norm.push_str(&s);
            }
            Component::CurDir => {}
        }
    }
    if m.path != norm {
        return Err(Error::Invalid("duplicate or non-normalized path".into()));
    }
    let lower = m.path.to_ascii_lowercase();
    if lower.ends_with(".sh")
        || lower.ends_with(".exe")
        || lower.ends_with(".so")
        || lower.contains("symlink")
    {
        return Err(Error::Invalid("executable or link member".into()));
    }
    if m.size > MAX_FILE {
        return Err(Error::Invalid("file too large".into()));
    }
    if m.compressed > 0 && m.size / m.compressed.max(1) > MAX_ZIP_RATIO {
        return Err(Error::Invalid("zip bomb ratio".into()));
    }
    Ok(())
}

pub fn reject_package(members: &[PackageMember]) -> Result<()> {
    if members.len() > MAX_FILES {
        return Err(Error::Invalid("too many files".into()));
    }
    let mut total = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    for m in members {
        reject_member(m)?;
        if !seen.insert(&m.path) {
            return Err(Error::Invalid("duplicate path".into()));
        }
        total = total.saturating_add(m.size);
    }
    if total > MAX_TOTAL {
        return Err(Error::Invalid("package too large".into()));
    }
    Ok(())
}

pub fn privacy_block(text: &str) -> Result<()> {
    for needle in ["BEGIN PRIVATE KEY", "sk-", "/Users/", "ANSWER:"] {
        if text.contains(needle) {
            return Err(Error::Invalid("privacy_gate".into()));
        }
    }
    Ok(())
}

pub fn foreign_approval_is_not_local(_: &str) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_and_exec_rejected() {
        assert!(
            reject_member(&PackageMember {
                path: "../x".into(),
                size: 1,
                compressed: 1
            })
            .is_err()
        );
        assert!(
            reject_member(&PackageMember {
                path: "/etc/passwd".into(),
                size: 1,
                compressed: 1
            })
            .is_err()
        );
        assert!(
            reject_member(&PackageMember {
                path: "run.sh".into(),
                size: 1,
                compressed: 1
            })
            .is_err()
        );
        assert!(
            reject_member(&PackageMember {
                path: "skill.md".into(),
                size: 1,
                compressed: 1
            })
            .is_ok()
        );
    }

    #[test]
    fn privacy_and_foreign_approval() {
        assert!(privacy_block("hello").is_ok());
        assert!(privacy_block("BEGIN PRIVATE KEY").is_err());
        assert!(foreign_approval_is_not_local("formal_eval_from_elsewhere"));
    }
}
