use evo_core::hash;
use evo_engine::seeds::{SeedClass, SeedTriple, auto_activate, classify};

fn complete() -> SeedTriple {
    let b = hash(b"baseline");
    SeedTriple {
        bundled: Some(b.clone()),
        local: Some(b.clone()),
        upstream: Some(b),
        local_marked: true,
        publisher_bundled: "rsia.builtin".into(),
        publisher_local: "rsia.builtin".into(),
        asset_id_bundled: Some("seed-identity".into()),
        asset_id_local: Some("seed-identity".into()),
        kind_bundled: Some("skill".into()),
        kind_local: Some("skill".into()),
    }
}

#[test]
fn malformed_or_missing_baseline_is_unknown_without_legacy_short_exceptions() {
    for b in [
        None,
        Some(String::new()),
        Some("a".into()),
        Some("f".repeat(63)),
        Some("f".repeat(65)),
        Some("g".repeat(64)),
        Some("A".repeat(64)),
        Some("é".repeat(32)),
        Some(" ".repeat(64)),
    ] {
        let mut t = complete();
        t.bundled = b.clone();
        if let Some(value) = b {
            t.local = Some(value.clone());
            t.upstream = Some(value);
        }
        assert_eq!(classify(&t), SeedClass::Unknown, "{t:?}");
        assert!(auto_activate(classify(&t)).is_err());
    }
}

#[test]
fn missing_identity_on_either_or_both_sides_is_unknown_even_without_a_marker() {
    for field in 0..8 {
        for marked in [false, true] {
            let mut t = complete();
            t.local_marked = marked;
            match field {
                0 => t.asset_id_bundled = None,
                1 => t.asset_id_local = None,
                2 => t.kind_bundled = None,
                3 => t.kind_local = None,
                4 => {
                    t.asset_id_bundled = None;
                    t.asset_id_local = None;
                }
                5 => {
                    t.kind_bundled = None;
                    t.kind_local = None;
                }
                6 => t.publisher_bundled.clear(),
                _ => t.publisher_local.clear(),
            }
            assert_eq!(classify(&t), SeedClass::Unknown, "{field} {marked}");
        }
    }
}

#[test]
fn unconfirmable_identifiers_or_unsupported_kinds_are_unknown() {
    for field in 0..8 {
        let mut t = complete();
        match field {
            0 => t.asset_id_bundled = Some(String::new()),
            1 => t.asset_id_local = Some("invalid identity".into()),
            2 => t.publisher_bundled = "invalid publisher".into(),
            3 => t.publisher_local = "é".into(),
            4 => t.kind_bundled = Some("SKILL".into()),
            5 => t.kind_local = Some("unknown".into()),
            6 => t.kind_local = Some(String::new()),
            _ => t.asset_id_bundled = Some("a".repeat(129)),
        }
        assert_eq!(classify(&t), SeedClass::Unknown, "{field}");
    }
}

#[test]
fn absent_or_invalid_local_and_upstream_content_do_not_prove_no_update() {
    for field in 0..6 {
        let mut t = complete();
        match field {
            0 => t.upstream = None,
            1 => t.local = None,
            2 => t.local = Some("g".repeat(64)),
            3 => t.upstream = Some("A".repeat(64)),
            4 => t.local = Some(String::new()),
            _ => t.upstream = Some("short".into()),
        }
        assert_eq!(classify(&t), SeedClass::Unknown, "{field}");
    }
}

#[test]
fn complete_inputs_keep_all_relationships_identity_differences_and_missing_marker() {
    let b = hash(b"baseline");
    let l = hash(b"edited local");
    let u = hash(b"new upstream");
    for kind in ["skill", "improver", "harness"] {
        for (local, upstream, expected) in [
            (b.clone(), b.clone(), SeedClass::Unmodified),
            (l.clone(), b.clone(), SeedClass::LocallyEdited),
            (b.clone(), u.clone(), SeedClass::UpstreamNewer),
            (l.clone(), u.clone(), SeedClass::BothChanged),
            (u.clone(), u.clone(), SeedClass::IdenticalToUpstream),
        ] {
            let mut t = complete();
            t.kind_bundled = Some(kind.into());
            t.kind_local = Some(kind.into());
            t.local = Some(local);
            t.upstream = Some(upstream);
            assert_eq!(classify(&t), expected);
            assert!(auto_activate(expected).is_err());
        }
    }
    for field in 0..3 {
        let mut t = complete();
        match field {
            0 => t.publisher_local = "different.publisher".into(),
            1 => t.asset_id_local = Some("different-asset".into()),
            _ => t.kind_local = Some("harness".into()),
        }
        assert_eq!(classify(&t), SeedClass::SameNameDifferentPublisher);
        assert!(auto_activate(classify(&t)).is_err());
    }
    let mut t = complete();
    t.local_marked = false;
    assert_eq!(classify(&t), SeedClass::MissingMarker);
    assert!(auto_activate(classify(&t)).is_err());
}

#[test]
fn legacy_classification_wire_values_and_legacy_triple_shapes_remain_readable() {
    for (kind, wire) in [
        (SeedClass::Unmodified, "\"unmodified\""),
        (SeedClass::LocallyEdited, "\"locally_edited\""),
        (SeedClass::UpstreamNewer, "\"upstream_newer\""),
        (SeedClass::BothChanged, "\"both_changed\""),
        (SeedClass::IdenticalToUpstream, "\"identical_to_upstream\""),
        (
            SeedClass::SameNameDifferentPublisher,
            "\"same_name_different_publisher\"",
        ),
        (SeedClass::MissingMarker, "\"missing_marker\""),
        (SeedClass::CorruptBaseline, "\"corrupt_baseline\""),
        (SeedClass::Unknown, "\"unknown\""),
    ] {
        assert_eq!(serde_json::to_string(&kind).unwrap(), wire);
        assert_eq!(serde_json::from_str::<SeedClass>(wire).unwrap(), kind);
        assert!(auto_activate(kind).is_err());
    }
    const LEGACY: &str = r#"{"bundled":"a","local":"a","upstream":"b","local_marked":true,"publisher_bundled":"p","publisher_local":"p"}"#;
    let old: SeedTriple = serde_json::from_str(LEGACY).unwrap();
    assert_eq!(classify(&old), SeedClass::Unknown);
    assert!(old.asset_id_bundled.is_none() && old.kind_local.is_none());
}
