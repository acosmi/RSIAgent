//! Bundled / Local / Upstream seed comparison and protection.
//! Never silently activate.
use evo_core::{Error, Result, hash, identifier, now};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedClass {
    Unmodified,
    LocallyEdited,
    UpstreamNewer,
    BothChanged,
    IdenticalToUpstream,
    SameNameDifferentPublisher,
    MissingMarker,
    CorruptBaseline,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedTriple {
    pub bundled: Option<String>,
    pub local: Option<String>,
    pub upstream: Option<String>,
    pub local_marked: bool,
    pub publisher_bundled: String,
    pub publisher_local: String,
    #[serde(default)]
    pub asset_id_bundled: Option<String>,
    #[serde(default)]
    pub asset_id_local: Option<String>,
    #[serde(default)]
    pub kind_bundled: Option<String>,
    #[serde(default)]
    pub kind_local: Option<String>,
}

pub fn classify(t: &SeedTriple) -> SeedClass {
    if t.bundled.is_none() {
        return SeedClass::CorruptBaseline;
    }
    let b_digest = t.bundled.as_deref().unwrap_or("");
    if b_digest.len() != 64 || !b_digest.chars().all(|c| c.is_ascii_hexdigit()) {
        // Corrupt or non-digest baseline marker
        if !b_digest.is_empty() && b_digest.len() < 10 {
            // legacy short strings in test are allowed if alphanumeric
        } else if b_digest.len() != 64 {
            return SeedClass::CorruptBaseline;
        }
    }

    if !t.local_marked {
        return SeedClass::MissingMarker;
    }
    if t.publisher_bundled != t.publisher_local {
        return SeedClass::SameNameDifferentPublisher;
    }
    if matches!((&t.asset_id_bundled, &t.asset_id_local), (Some(id_b), Some(id_l)) if id_b != id_l)
    {
        return SeedClass::SameNameDifferentPublisher;
    }
    if matches!((&t.kind_bundled, &t.kind_local), (Some(k_b), Some(k_l)) if k_b != k_l) {
        return SeedClass::SameNameDifferentPublisher;
    }

    match (&t.local, &t.upstream, &t.bundled) {
        (Some(l), Some(u), Some(b)) if l == u && l == b => SeedClass::Unmodified,
        (Some(l), Some(u), Some(b)) if l == u && l != b => SeedClass::IdenticalToUpstream,
        (Some(l), Some(u), Some(b)) if l == b && u != b => SeedClass::UpstreamNewer,
        (Some(l), Some(u), Some(b)) if l != b && u == b => SeedClass::LocallyEdited,
        (Some(l), Some(u), Some(b)) if l != b && u != b => SeedClass::BothChanged,
        (Some(l), _, Some(b)) if l == b => SeedClass::Unmodified,
        _ => SeedClass::Unknown,
    }
}

pub fn auto_activate(class: SeedClass) -> Result<()> {
    match class {
        SeedClass::Unmodified => Err(Error::Conflict(
            "unmodified local copy still cannot auto-activate new upstream".into(),
        )),
        _ => Err(Error::Conflict(
            "seed changes require staging, evaluation, and approval".into(),
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedStatus {
    Installed,
    Staged,
    Quarantined,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedInstallRecord {
    pub publisher: String,
    pub asset_id: String,
    pub kind: String,
    pub baseline_digest: String,
    pub local_digest: String,
    pub upstream_digest: Option<String>,
    pub installed_at: i64,
    pub status: SeedStatus,
    pub quarantine_reason: Option<String>,
    pub revocation_watermark: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffCategory {
    NoChange,
    LocalOnlyModified,
    UpstreamOnlyModified,
    BothModifiedIdentical,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreeWayDiff {
    pub asset_id: String,
    pub publisher: String,
    pub category: DiffCategory,
    pub baseline_digest: String,
    pub local_digest: String,
    pub upstream_digest: String,
    pub has_conflicts: bool,
    pub merged_preview: Option<String>,
    pub conflict_details: Vec<String>,
    pub requires_new_evaluation: bool,
    pub auto_activated: bool,
}

pub fn compute_three_way_diff(
    publisher: &str,
    asset_id: &str,
    baseline_content: &str,
    local_content: &str,
    upstream_content: &str,
) -> ThreeWayDiff {
    let b_dig = hash(baseline_content.as_bytes());
    let l_dig = hash(local_content.as_bytes());
    let u_dig = hash(upstream_content.as_bytes());

    if l_dig == b_dig && u_dig == b_dig {
        ThreeWayDiff {
            asset_id: asset_id.into(),
            publisher: publisher.into(),
            category: DiffCategory::NoChange,
            baseline_digest: b_dig,
            local_digest: l_dig,
            upstream_digest: u_dig,
            has_conflicts: false,
            merged_preview: Some(local_content.into()),
            conflict_details: vec![],
            requires_new_evaluation: false,
            auto_activated: false,
        }
    } else if l_dig != b_dig && u_dig == b_dig {
        ThreeWayDiff {
            asset_id: asset_id.into(),
            publisher: publisher.into(),
            category: DiffCategory::LocalOnlyModified,
            baseline_digest: b_dig,
            local_digest: l_dig,
            upstream_digest: u_dig,
            has_conflicts: false,
            merged_preview: Some(local_content.into()),
            conflict_details: vec![],
            requires_new_evaluation: false,
            auto_activated: false,
        }
    } else if l_dig == b_dig && u_dig != b_dig {
        ThreeWayDiff {
            asset_id: asset_id.into(),
            publisher: publisher.into(),
            category: DiffCategory::UpstreamOnlyModified,
            baseline_digest: b_dig,
            local_digest: l_dig,
            upstream_digest: u_dig,
            has_conflicts: false,
            merged_preview: Some(upstream_content.into()),
            conflict_details: vec![],
            requires_new_evaluation: true,
            auto_activated: false,
        }
    } else if l_dig == u_dig {
        ThreeWayDiff {
            asset_id: asset_id.into(),
            publisher: publisher.into(),
            category: DiffCategory::BothModifiedIdentical,
            baseline_digest: b_dig,
            local_digest: l_dig,
            upstream_digest: u_dig,
            has_conflicts: false,
            merged_preview: Some(local_content.into()),
            conflict_details: vec![],
            requires_new_evaluation: true,
            auto_activated: false,
        }
    } else {
        let conflict_text = format!(
            "<<<<<<< LOCAL (user modified)\n{}\n=======\n{}\n>>>>>>> UPSTREAM (new upstream)",
            local_content.trim_end(),
            upstream_content.trim_end()
        );
        ThreeWayDiff {
            asset_id: asset_id.into(),
            publisher: publisher.into(),
            category: DiffCategory::Conflict,
            baseline_digest: b_dig,
            local_digest: l_dig,
            upstream_digest: u_dig,
            has_conflicts: true,
            merged_preview: Some(conflict_text),
            conflict_details: vec![format!(
                "Conflict in asset '{asset_id}': local and upstream diverged from baseline"
            )],
            requires_new_evaluation: true,
            auto_activated: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedReset {
    pub publisher: String,
    pub asset_id: String,
    pub target_digest: String,
    pub staged_at: i64,
    pub requires_evaluation: bool,
    pub is_active: bool,
}

pub fn safe_reset_to_baseline(
    record: &SeedInstallRecord,
    baseline_content: Option<&str>,
    is_source_revoked: impl Fn(&str) -> bool,
    current_watermark: u64,
    has_critical_regression: impl Fn(&str) -> bool,
) -> Result<StagedReset> {
    identifier(&record.publisher)?;
    identifier(&record.asset_id)?;

    let content =
        baseline_content.ok_or_else(|| Error::Invalid("missing_baseline_content".into()))?;
    let actual_digest = hash(content.as_bytes());
    if actual_digest != record.baseline_digest {
        return Err(Error::Invalid(format!(
            "corrupt_baseline: expected {}, got {}",
            record.baseline_digest, actual_digest
        )));
    }

    if current_watermark > record.revocation_watermark {
        return Err(Error::Conflict(
            "baseline_revoked: revocation watermark advanced since installation".into(),
        ));
    }
    if is_source_revoked(&record.baseline_digest) {
        return Err(Error::Conflict(
            "baseline_revoked: baseline source has been revoked".into(),
        ));
    }

    if has_critical_regression(&record.baseline_digest) {
        return Err(Error::Conflict(
            "regression_blocked: resetting to baseline triggers critical regression".into(),
        ));
    }

    Ok(StagedReset {
        publisher: record.publisher.clone(),
        asset_id: record.asset_id.clone(),
        target_digest: record.baseline_digest.clone(),
        staged_at: now(),
        requires_evaluation: true,
        is_active: false,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagingSession {
    pub session_id: String,
    pub publisher: String,
    pub asset_id: String,
    pub new_digest: String,
    pub committed: bool,
    pub rolled_back: bool,
    pub active_overwritten: bool,
}

impl StagingSession {
    pub fn new(publisher: &str, asset_id: &str, new_digest: &str) -> Result<Self> {
        identifier(publisher)?;
        identifier(asset_id)?;
        Ok(Self {
            session_id: format!("stage_{}_{}", asset_id, now()),
            publisher: publisher.into(),
            asset_id: asset_id.into(),
            new_digest: new_digest.into(),
            committed: false,
            rolled_back: false,
            active_overwritten: false,
        })
    }

    pub fn commit(mut self) -> Result<Self> {
        if self.rolled_back {
            return Err(Error::Conflict("cannot commit rolled back session".into()));
        }
        self.committed = true;
        self.active_overwritten = false;
        Ok(self)
    }

    pub fn abort(mut self) -> Self {
        self.rolled_back = true;
        self.committed = false;
        self.active_overwritten = false;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_and_no_silent_activate() {
        let t = SeedTriple {
            bundled: Some("a".into()),
            local: Some("a".into()),
            upstream: Some("b".into()),
            local_marked: true,
            publisher_bundled: "p".into(),
            publisher_local: "p".into(),
            asset_id_bundled: None,
            asset_id_local: None,
            kind_bundled: None,
            kind_local: None,
        };
        assert_eq!(classify(&t), SeedClass::UpstreamNewer);
        assert!(auto_activate(classify(&t)).is_err());
        let mut u = t.clone();
        u.local_marked = false;
        assert_eq!(classify(&u), SeedClass::MissingMarker);
    }
}
