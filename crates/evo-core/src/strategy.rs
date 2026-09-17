//! Generation vs exploration. Not a third publishable asset.
use crate::{Error, Result, Validate, identifier, text};
use serde::{Deserialize, Serialize};

pub const W_DEFAULT: u8 = 1;
pub const MAX_NODES: u8 = 12;
pub const MAX_DEPTH: u8 = 4;
pub const MAX_REPAIR: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationStrategy {
    pub schema_version: String,
    pub instruction: String,
    pub max_candidates: u8,
}

impl Default for GenerationStrategy {
    fn default() -> Self {
        Self {
            schema_version: "rsia.generation.v1".into(),
            instruction: "Propose a narrowly applicable text skill.".into(),
            max_candidates: 2,
        }
    }
}

impl Validate for GenerationStrategy {
    fn validate(&self) -> Result<()> {
        if self.schema_version != "rsia.generation.v1" || !(1..=4).contains(&self.max_candidates) {
            return Err(Error::Invalid("unsupported generation strategy".into()));
        }
        text(&self.instruction, "instruction", 8192)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorationPolicy {
    pub schema_version: String,
    pub w: u8,
    pub max_nodes: u8,
    pub max_depth: u8,
    pub repair_budget: u8,
}

impl Default for ExplorationPolicy {
    fn default() -> Self {
        Self {
            schema_version: "rsia.exploration.v1".into(),
            w: W_DEFAULT,
            max_nodes: MAX_NODES,
            max_depth: MAX_DEPTH,
            repair_budget: MAX_REPAIR,
        }
    }
}

impl Validate for ExplorationPolicy {
    fn validate(&self) -> Result<()> {
        if self.schema_version != "rsia.exploration.v1" {
            return Err(Error::Invalid("unsupported exploration policy".into()));
        }
        if self.w != 1 {
            return Err(Error::Invalid(
                "W>1 is not enabled; no parallel-gain claim".into(),
            ));
        }
        if self.max_nodes > MAX_NODES
            || self.max_depth > MAX_DEPTH
            || self.repair_budget > MAX_REPAIR
        {
            return Err(Error::Invalid(
                "exploration limits exceed admin caps".into(),
            ));
        }
        if self.max_nodes < 1 || self.max_depth < 1 {
            return Err(Error::Invalid("exploration limits too small".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixView {
    pub search_parent: String,
    pub approved_parent: String,
    pub depth: u8,
}

impl PrefixView {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.search_parent)?;
        identifier(&self.approved_parent)?;
        if self.search_parent == self.approved_parent {
            return Err(Error::Invalid(
                "search_parent must be distinct from approved_parent for search nodes".into(),
            ));
        }
        Ok(())
    }
}
