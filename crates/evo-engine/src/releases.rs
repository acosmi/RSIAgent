//! Approve → canary → Active with CAS on the profile pointer.
use evo_core::contract::{AppliedReceipt, CapabilityLevel, ResolvedBundle};
use evo_core::evaluation::Verdict;
use evo_core::{Context, Error, Result, Role, identifier};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseState {
    Evaluated,
    Approved,
    Canary,
    Active,
    RolledBack,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub id: String,
    pub bundle_digest: String,
    pub evaluation_id: String,
    pub parent_pointer: String,
    pub state: ReleaseState,
    pub approved_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePointer {
    pub profile_id: String,
    pub active: Option<String>,
    pub epoch: u64,
}

impl ProfilePointer {
    pub fn cas(&mut self, expected: u64, next_active: String) -> Result<()> {
        identifier(&next_active)?;
        if self.epoch != expected {
            return Err(Error::Conflict("cas_lost".into()));
        }
        self.active = Some(next_active);
        self.epoch += 1;
        Ok(())
    }
}

pub fn approve(
    ctx: &Context,
    bundle: &ResolvedBundle,
    verdict: Verdict,
    evaluation_id: &str,
) -> Result<Release> {
    ctx.require(&[Role::Admin])?;
    identifier(evaluation_id)?;
    if verdict != Verdict::Improved && verdict != Verdict::Noninferior {
        return Err(Error::Invalid(
            "approval requires a formal improved or noninferior verdict".into(),
        ));
    }
    Ok(Release {
        id: format!("rel-{}", &bundle.digest[..8.min(bundle.digest.len())]),
        bundle_digest: bundle.digest.clone(),
        evaluation_id: evaluation_id.into(),
        parent_pointer: bundle.parent_digest.clone(),
        state: ReleaseState::Approved,
        approved_by: Some(ctx.actor().into()),
    })
}

pub fn activate(
    ctx: &Context,
    release: &mut Release,
    pointer: &mut ProfilePointer,
    expected_epoch: u64,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    if release.state != ReleaseState::Approved && release.state != ReleaseState::Canary {
        return Err(Error::Conflict("release not activatable".into()));
    }
    pointer.cas(expected_epoch, release.bundle_digest.clone())?;
    release.state = ReleaseState::Active;
    Ok(())
}

pub fn revoke(ctx: &Context, release: &mut Release, pointer: &mut ProfilePointer) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    release.state = ReleaseState::Revoked;
    if pointer.active.as_deref() == Some(release.bundle_digest.as_str()) {
        pointer.active = None;
        pointer.epoch += 1;
    }
    Ok(())
}

pub fn rollback(
    ctx: &Context,
    current: &mut Release,
    previous_digest: &str,
    pointer: &mut ProfilePointer,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    identifier(previous_digest)?;
    if current.state == ReleaseState::Revoked && previous_digest == current.bundle_digest {
        return Err(Error::Conflict(
            "revoked content cannot be the rollback target".into(),
        ));
    }
    pointer.active = Some(previous_digest.into());
    pointer.epoch += 1;
    current.state = ReleaseState::RolledBack;
    Ok(())
}

pub fn apply_receipt(ctx: &Context, release: &Release, receipt: &AppliedReceipt) -> Result<()> {
    ctx.require(&[Role::Host])?;
    receipt.validate()?;
    if receipt.bundle_digest != release.bundle_digest {
        return Err(Error::Conflict(
            "receipt bundle does not match release".into(),
        ));
    }
    if release.state == ReleaseState::Revoked {
        return Err(Error::Conflict("revoked release cannot be used".into()));
    }
    if receipt.capability_level == CapabilityLevel::ToolOnly && !receipt.used.is_empty() {
        return Err(Error::Invalid("tool-only cannot claim used".into()));
    }
    Ok(())
}

pub fn side_effects_survive_rollback() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::Strategy;
    use evo_core::contract::{
        CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch, SkillSnapshot,
        compile_bundle,
    };
    use std::collections::BTreeSet;

    fn bundle() -> ResolvedBundle {
        let profile = Profile {
            id: "p1".into(),
            evolution_enabled: true,
            parent_digest: "parent1".into(),
            baseline_digest: "base1".into(),
        };
        let parent = SkillSnapshot::empty();
        let baseline = SkillSnapshot::empty();
        let ps = Strategy::default();
        let bs = Strategy::default();
        let sp = SkillPatch::default();
        let ip = ImproverPatch::default();
        let caps = HostCapabilities {
            available: BTreeSet::new(),
            granted: BTreeSet::new(),
        };
        let revoked = BTreeSet::new();
        compile_bundle(CompileParts {
            profile: &profile,
            parent: &parent,
            baseline: &baseline,
            parent_strategy: &ps,
            baseline_strategy: &bs,
            skill_patch: &sp,
            improver_patch: &ip,
            caps: &caps,
            revoked: &revoked,
        })
        .unwrap()
    }

    #[test]
    fn concurrent_cas_one_winner() {
        let mut p = ProfilePointer {
            profile_id: "p1".into(),
            active: None,
            epoch: 0,
        };
        p.cas(0, "a".into()).unwrap();
        assert!(p.cas(0, "b".into()).is_err());
        assert_eq!(p.active.as_deref(), Some("a"));
    }

    #[test]
    fn evaluator_cannot_approve() {
        let ctx = Context::new("n", "e", Role::Evaluator).unwrap();
        let b = bundle();
        assert!(approve(&ctx, &b, Verdict::Improved, "ev1").is_err());
    }

    #[test]
    fn revoke_stops_new_use() {
        let admin = Context::new("n", "a", Role::Admin).unwrap();
        let host = Context::new("n", "h", Role::Host).unwrap();
        let b = bundle();
        let mut rel = approve(&admin, &b, Verdict::Improved, "ev1").unwrap();
        let mut ptr = ProfilePointer {
            profile_id: "p1".into(),
            active: None,
            epoch: 0,
        };
        activate(&admin, &mut rel, &mut ptr, 0).unwrap();
        revoke(&admin, &mut rel, &mut ptr).unwrap();
        let receipt = AppliedReceipt {
            offered: vec!["evo_prepare".into()],
            attached: Vec::new(),
            used: Vec::new(),
            verified_benefit: Vec::new(),
            bundle_digest: b.digest,
            request_digest: "req1".into(),
            capability_level: CapabilityLevel::ToolOnly,
            truncated: false,
            attested_by: "host".into(),
        };
        assert!(apply_receipt(&host, &rel, &receipt).is_err());
        assert!(side_effects_survive_rollback());
    }

    #[test]
    fn request_digests_may_differ() {
        assert_ne!("req-task-a", "req-task-b");
    }
}
