//! Restricted improver inheritance (plan §9, §9.1; E14 increment 1). Recursion
//! depth 1. The only mechanism class open is the exploration policy; the final
//! grader and every other control-plane value are not writable.
//!
//! Inheritance is shown by real dispatched decisions, never by a flag: the
//! evidence comes from [`MechanismUsageRecordV1`] values that only
//! `PersistentCoordinator::verified_mechanism_usage` can build. This module
//! does not prove that an inherited mechanism is better, and it does not start
//! an Improver job (`meta.start` stays blocked).
use crate::exploration::MechanismUsageRecordV1;
use evo_core::contract::assert_candidate_may_write;
use evo_core::improver::ImproverContentV2;
use evo_core::{ArtifactKind, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const META_DEPTH_CAP: u8 = 1;

/// A restricted Improver candidate: the content it would approve and the digest
/// of the approved parent content it is derived from. `depth` is supplied by
/// the caller; it is not derived from lineage here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaCandidate {
    pub depth: u8,
    pub content: ImproverContentV2,
    pub parent_improver_digest: String,
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl MetaCandidate {
    pub fn validate(&self) -> Result<()> {
        if self.depth > META_DEPTH_CAP {
            return Err(Error::Invalid("meta recursion depth cap is 1".into()));
        }
        self.content.validate()?;
        if !is_lowercase_sha256(&self.parent_improver_digest) {
            return Err(Error::Invalid(
                "parent_improver_digest must be a lowercase sha256 digest".into(),
            ));
        }
        if self.content.content_digest()? == self.parent_improver_digest {
            return Err(Error::Invalid(
                "candidate content equals its parent improver: no change to inherit".into(),
            ));
        }
        Ok(())
    }
}

/// What a job's real dispatched decisions show about one approved mechanism.
/// It records that the approved policy appeared in `decisions` dispatched
/// decisions of one world (with `distinct_action_digests` different actions);
/// it is not evidence that the policy is better.
#[derive(Debug, Clone, Serialize)]
pub struct InheritanceEvidenceV1 {
    pub approved_content_digest: String,
    pub policy_digest: String,
    pub caps_digest: String,
    pub world_id: String,
    pub decisions: usize,
    pub distinct_action_digests: usize,
}

/// Checks that real, already dispatched decisions used the approved mechanism
/// (plan §9.1, V036/V086.d). `usage` must be non-empty, come from a single
/// world, and every record must carry the digest of the approved content's own
/// policy. A job that ran with a different policy (for instance the built-in
/// one) is a `Conflict`; there is no boolean to set and no pointer to swap.
pub fn verify_mechanism_inheritance(
    approved: &ImproverContentV2,
    usage: &[MechanismUsageRecordV1],
) -> Result<InheritanceEvidenceV1> {
    approved.validate()?;
    let approved_content_digest = approved.content_digest()?;
    let policy_digest = approved.exploration_policy().digest()?;
    let Some(first) = usage.first() else {
        return Err(Error::Invalid(
            "no real dispatched decision used the approved mechanism".into(),
        ));
    };
    if usage.iter().any(|record| {
        record.world_id() != first.world_id() || record.caps_digest() != first.caps_digest()
    }) {
        return Err(Error::Invalid(
            "mechanism usage must come from a single exploration world".into(),
        ));
    }
    if usage
        .iter()
        .any(|record| record.policy_digest() != policy_digest)
    {
        return Err(Error::Conflict(
            "job used a different mechanism than the approved improver".into(),
        ));
    }
    let distinct_action_digests: BTreeSet<&str> =
        usage.iter().map(|record| record.action_digest()).collect();
    Ok(InheritanceEvidenceV1 {
        approved_content_digest,
        policy_digest,
        caps_digest: first.caps_digest().to_owned(),
        world_id: first.world_id().to_owned(),
        decisions: usage.len(),
        distinct_action_digests: distinct_action_digests.len(),
    })
}

pub fn cannot_write_protected(field: &str) -> Result<()> {
    assert_candidate_may_write(ArtifactKind::Improver, field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::strategy::ElasticPolicyV1;

    fn changed_content() -> ImproverContentV2 {
        let mut content = ImproverContentV2::builtin_default();
        let evo_core::improver::ImproverMechanismV2::ExplorationPolicy { policy } =
            &mut content.mechanism;
        *policy = ElasticPolicyV1 {
            max_focus_actions: 1,
            ..ElasticPolicyV1::default()
        };
        content
    }

    #[test]
    fn depth_and_protected_fields() {
        let parent = ImproverContentV2::builtin_default()
            .content_digest()
            .unwrap();
        let mut c = MetaCandidate {
            depth: 2,
            content: changed_content(),
            parent_improver_digest: parent,
        };
        assert!(c.validate().is_err());
        c.depth = 1;
        c.validate().unwrap();
        assert!(cannot_write_protected("budget").is_err());
        assert!(cannot_write_protected("improver.instruction").is_ok());
    }
}
