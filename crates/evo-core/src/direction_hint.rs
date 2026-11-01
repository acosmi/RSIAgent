//! Direction hint (plan §9.1.1, V118.b): a fixed control-plane record and its descriptive on/off
//! ablation. Library layer only: nothing here is persisted, registered or consumed by a run.
use crate::evaluation::{DataUse, SCORE_MICROS_MAX};
use crate::{Error, Result, canonical_fingerprint, hash, identifier, text};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const DIRECTION_HINT_SCHEMA: &str = "rsia.direction_hint.v1";
pub const DIRECTION_HINT_ABLATION_SCHEMA: &str = "rsia.direction_hint_ablation.v1";
pub const DIRECTION_HINT_BODY_MAX_BYTES: usize = 8 * 1024;

fn digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(format!(
            "{name} must be a lowercase sha256 digest"
        )));
    }
    Ok(())
}

/// Fixed control-plane record; off unless a caller passes one, and never a candidate field.
/// `source_node_ids` and `episode_ids` are stated by the producer. `digest` is the sha256 of the
/// raw UTF-8 bytes of `body` (plan §5.1.2), with no newline or Unicode normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionHintV1 {
    pub schema_version: String,
    pub id: String,
    pub world_id: String,
    pub source_node_ids: Vec<String>,
    pub generation_rule_version: String,
    pub body: String,
    pub digest: String,
    pub episode_ids: Vec<String>,
}

impl DirectionHintV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != DIRECTION_HINT_SCHEMA {
            return Err(Error::Invalid("unsupported direction hint schema".into()));
        }
        for value in [&self.id, &self.world_id, &self.generation_rule_version] {
            identifier(value)?;
        }
        for (name, ids) in [
            ("source_node_ids", &self.source_node_ids),
            ("episode_ids", &self.episode_ids),
        ] {
            if ids.is_empty() || !ids.windows(2).all(|pair| pair[0] < pair[1]) {
                return Err(Error::Invalid(format!(
                    "{name} must be nonempty, unique and sorted"
                )));
            }
            for id in ids {
                identifier(id)?;
            }
        }
        text(
            &self.body,
            "direction hint body",
            DIRECTION_HINT_BODY_MAX_BYTES,
        )?;
        digest(&self.digest, "direction hint digest")?;
        if self.digest != hash(self.body.as_bytes()) {
            return Err(Error::Conflict(
                "direction hint digest differs from the sha256 of its body".into(),
            ));
        }
        Ok(())
    }
}

/// The generation-signature component with an optional hint (plan §7.4.1, V022). `None` returns
/// `base` unchanged; `id` and `world_id` are names and not inputs. This is not the online world
/// signature, which has no generation component and is not changed here.
pub fn generation_signature_with_hint(
    base: &str,
    hint: Option<&DirectionHintV1>,
) -> Result<String> {
    let Some(hint) = hint else {
        return Ok(base.into());
    };
    hint.validate()?;
    canonical_fingerprint(&serde_json::json!({
        "domain": "rsia.direction_hint_signature.v1",
        "base": base,
        "digest": hint.digest,
        "generation_rule_version": hint.generation_rule_version,
        "source_node_ids": hint.source_node_ids,
        "episode_ids": hint.episode_ids,
    }))
}

/// `scope_id` and `cap_micros` are not checked against any registered budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AblationBudgetV1 {
    pub scope_id: String,
    pub cap_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AblationClusterV1 {
    pub cluster_id: String,
    pub off_score_micros: u32,
    pub on_score_micros: u32,
}

/// Descriptive development diagnostic of hint off versus on over the same clusters (plan §9.1.1).
/// It carries cluster scores and their mean difference only: no bound, verdict, family or release
/// field. Costs are known `u64` micros; the `usage_uncertain` metering path (plan §3.6) is not
/// implemented here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionHintAblationV1 {
    pub schema_version: String,
    pub id: String,
    pub data_use: DataUse,
    pub hint: DirectionHintV1,
    pub base_generation_signature: String,
    pub budget: AblationBudgetV1,
    pub clusters: Vec<AblationClusterV1>,
    pub off_cost_micros: u64,
    pub on_dev_cost_micros: u64,
    pub hint_generation_cost_micros: u64,
    pub on_total_cost_micros: u64,
    /// Mean over clusters of `on - off`, rounded toward negative infinity.
    pub mean_delta_micros: i64,
}

impl DirectionHintAblationV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != DIRECTION_HINT_ABLATION_SCHEMA {
            return Err(Error::Invalid(
                "unsupported direction hint ablation schema".into(),
            ));
        }
        identifier(&self.id)?;
        if self.data_use != DataUse::Development {
            return Err(Error::Invalid(
                "direction hint ablation runs on development data only".into(),
            ));
        }
        self.hint.validate()?;
        digest(&self.base_generation_signature, "base_generation_signature")?;
        identifier(&self.budget.scope_id)?;
        if self.clusters.is_empty() {
            return Err(Error::Invalid(
                "direction hint ablation needs at least one cluster".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        let mut total = 0_i64;
        for cluster in &self.clusters {
            identifier(&cluster.cluster_id)?;
            if !seen.insert(&cluster.cluster_id) {
                return Err(Error::Conflict("duplicate ablation cluster".into()));
            }
            if cluster.off_score_micros > SCORE_MICROS_MAX
                || cluster.on_score_micros > SCORE_MICROS_MAX
            {
                return Err(Error::Invalid(
                    "ablation score exceeds the score range".into(),
                ));
            }
            total += i64::from(cluster.on_score_micros) - i64::from(cluster.off_score_micros);
        }
        if self.mean_delta_micros != total.div_euclid(self.clusters.len() as i64) {
            return Err(Error::Conflict(
                "mean_delta_micros differs from the cluster differences".into(),
            ));
        }
        if self
            .on_dev_cost_micros
            .checked_add(self.hint_generation_cost_micros)
            != Some(self.on_total_cost_micros)
        {
            return Err(Error::Conflict(
                "on-arm total differs from development plus hint generation cost".into(),
            ));
        }
        if self
            .off_cost_micros
            .checked_add(self.on_total_cost_micros)
            .is_none_or(|spend| spend > self.budget.cap_micros)
        {
            return Err(Error::Budget);
        }
        Ok(())
    }
}

pub fn direction_hint_ablation_is_not_formal(_: &DirectionHintAblationV1) -> Result<()> {
    Err(Error::Invalid(
        "DirectionHintAblation is descriptive: it cannot become FormalEvaluation or release evidence"
            .into(),
    ))
}
