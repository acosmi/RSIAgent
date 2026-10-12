use evo_core::contract::{HostSurfaceManifest, SurfaceCoverage};
use evo_core::{Error, fingerprint};

const REFERENCE_JSON: &str = include_str!("../../../fixtures/reference-host/host_surface.v1.json");
const REFERENCE_DIGEST: &str = "e03c7b9dbe5566b5bcc0d7a74d78e66faee90960a424dfc04d07b03bbe7d7ac1";

fn surface() -> HostSurfaceManifest {
    serde_json::from_str(REFERENCE_JSON).unwrap()
}

fn extracted() -> Vec<String> {
    ["theme", "instruction", "model", "model"]
        .map(String::from)
        .to_vec()
}

fn assert_conflict(manifest: &HostSurfaceManifest, name: &str) {
    let result = manifest.validate_against_extraction(&extracted());
    assert!(
        matches!(result, Err(Error::Invalid(ref message))
            if message == &format!("conflicting host surface items for name {name}")),
        "{result:?}"
    );
}

#[test]
fn conflicting_duplicates_compare_every_v1_item_field() {
    for change in 0..4 {
        let mut m = surface();
        let mut item = m.items[0].clone();
        match change {
            0 => item.reason = "another valid explanation".into(),
            1 => item.consumer = Some("another-consumer".into()),
            2 => item.mapped_field = Some("host.model".into()),
            _ => {
                item.coverage = SurfaceCoverage::Unsupported;
                item.mapped_field = None;
                item.consumer = None;
            }
        }
        m.items.push(item);
        assert_conflict(&m, "instruction");
    }
}

#[test]
fn nonadjacent_conflicts_are_rejected_in_both_orders() {
    for reverse in [false, true] {
        let mut m = surface();
        m.items.extend(m.items.clone());
        let mut conflict = m.items[1].clone();
        conflict.consumer = Some("runtime-description".into());
        m.items.insert(0, conflict);
        if reverse {
            m.items.reverse();
        }
        assert_conflict(&m, "model");
    }
}

#[test]
fn exact_clones_keep_v1_wire_digest_and_duplicate_extraction_behavior() {
    let original = surface();
    assert_eq!(fingerprint(&original).unwrap(), REFERENCE_DIGEST);
    let original_bytes = serde_json::to_vec(&original).unwrap();
    let mut clones = original.clone();
    clones.items.extend(original.items.clone());
    clones.items.push(original.items[0].clone());
    clones.validate_against_extraction(&extracted()).unwrap();
    let bytes = serde_json::to_vec(&clones).unwrap();
    let decoded: HostSurfaceManifest = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), bytes);
    assert_eq!(
        fingerprint(&decoded).unwrap(),
        fingerprint(&clones).unwrap()
    );
    assert_eq!(serde_json::to_vec(&surface()).unwrap(), original_bytes);
}

#[test]
fn v1_rejects_typed_fields_and_keeps_schema_and_name_guards() {
    for field in [
        "value_type",
        "enum_values",
        "default",
        "renamed_from",
        "test_ref",
        "capability",
    ] {
        let mut raw = serde_json::to_value(surface()).unwrap();
        raw["items"][0][field] = serde_json::json!("unexpected");
        assert!(
            serde_json::from_value::<HostSurfaceManifest>(raw).is_err(),
            "{field}"
        );
    }
    for field in ["source_kind", "retired"] {
        let mut raw = serde_json::to_value(surface()).unwrap();
        raw[field] = serde_json::json!("unexpected");
        assert!(
            serde_json::from_value::<HostSurfaceManifest>(raw).is_err(),
            "{field}"
        );
    }
    let mut m = surface();
    m.schema_version = "rsia.host_surface.v2".into();
    assert!(m.validate_against_extraction(&extracted()).is_err());
    assert!(surface().validate_against_extraction(&[]).is_err());
    assert!(
        surface()
            .validate_against_extraction(&["model".into()])
            .is_err()
    );
}
