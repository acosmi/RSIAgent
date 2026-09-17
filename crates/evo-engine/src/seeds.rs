//! Bundled / Local / Upstream seed comparison. Never silently activate.
use evo_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedClass {
    Unmodified,
    LocallyEdited,
    UpstreamNewer,
    BothChanged,
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
}

pub fn classify(t: &SeedTriple) -> SeedClass {
    if t.bundled.is_none() {
        return SeedClass::CorruptBaseline;
    }
    if !t.local_marked {
        return SeedClass::MissingMarker;
    }
    if t.publisher_bundled != t.publisher_local {
        return SeedClass::SameNameDifferentPublisher;
    }
    match (&t.local, &t.upstream, &t.bundled) {
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
        };
        assert_eq!(classify(&t), SeedClass::UpstreamNewer);
        assert!(auto_activate(classify(&t)).is_err());
        let mut u = t.clone();
        u.local_marked = false;
        assert_eq!(classify(&u), SeedClass::MissingMarker);
    }
}
