use evo_core::contract::{FIELD_CONTRACTS, HostSurfaceManifest, SurfaceCoverage, SurfaceItem};
use evo_core::{Error, fingerprint, hash};

const REFERENCE_JSON: &str = include_str!("../../../fixtures/reference-host/host_surface.v1.json");
const REFERENCE_DIGEST: &str = "e03c7b9dbe5566b5bcc0d7a74d78e66faee90960a424dfc04d07b03bbe7d7ac1";
const LEGACY_JSON: &str = r#"{"schema_version":"rsia.host_surface.v1","host":"reference","host_version":"0.1.0","adapter_version":"0.1.0","source_digest":"reference_host_surface_v1","items":[{"name":"instruction","coverage":"supported","mapped_field":"skill.content","consumer":"reference-host","reason":"run-level instruction projection"},{"name":"model","coverage":"runtime_owned","mapped_field":null,"consumer":null,"reason":"model identity is host-declared"},{"name":"theme","coverage":"unsupported","mapped_field":null,"consumer":null,"reason":"UI theme is out of scope"}]}"#;
const MISMATCH: &str = "supported item consumer differs from field contract";

fn surface() -> HostSurfaceManifest {
    serde_json::from_str(REFERENCE_JSON).unwrap()
}

fn names() -> Vec<String> {
    ["instruction", "model", "theme"].map(String::from).to_vec()
}

fn assert_invalid(result: evo_core::Result<()>, expected: &str) {
    assert!(
        matches!(result, Err(Error::Invalid(ref message)) if message == expected),
        "expected Invalid({expected:?}), got {result:?}"
    );
}

#[test]
fn v1_supported_consumers_match_field_contract_exactly() {
    let mut manifest = surface();
    manifest.items = FIELD_CONTRACTS
        .iter()
        .enumerate()
        .map(|(index, contract)| SurfaceItem {
            name: format!("field-{index}"),
            coverage: SurfaceCoverage::Supported,
            mapped_field: Some(contract.path.into()),
            consumer: Some(contract.consumer.into()),
            reason: "software declaration of the existing v1 field contract".into(),
        })
        .collect();
    let extracted: Vec<_> = manifest
        .items
        .iter()
        .map(|item| item.name.clone())
        .collect();
    manifest.validate_against_extraction(&extracted).unwrap();
    for (index, contract) in FIELD_CONTRACTS.iter().enumerate() {
        for wrong in [
            "reference-host".into(),
            contract.consumer.to_ascii_uppercase(),
            format!(" {} ", contract.consumer),
        ] {
            let mut changed = manifest.clone();
            changed.items[index].consumer = Some(wrong);
            changed
                .validate_frozen_v1_against_extraction(&extracted)
                .unwrap();
            assert_invalid(changed.validate_against_extraction(&extracted), MISMATCH);
        }
    }
}

#[test]
fn v1_consumer_mismatch_keeps_prior_validation_error_priority() {
    for fault in 0..9 {
        let mut manifest = surface();
        manifest.items[0].consumer = Some("reference-host".into());
        let mut extracted = names();
        let expected = match fault {
            0 => {
                manifest.schema_version = "rsia.host_surface.unknown".into();
                "unsupported host surface schema"
            }
            1 => {
                extracted.clear();
                "empty extraction is not a successful cover"
            }
            2 => {
                extracted.push("unclassified".into());
                "extracted field unclassified is unclassified"
            }
            3 => {
                manifest.items[0].mapped_field = None;
                "supported item needs mapped_field"
            }
            4 => {
                manifest.items[0].mapped_field = Some("unknown.field".into());
                "no consumer for field unknown.field"
            }
            5 => {
                let mut conflict = manifest.items[0].clone();
                conflict.reason = "another historical meaning".into();
                manifest.items.push(conflict);
                "conflicting host surface items for name instruction"
            }
            6 => {
                extracted.remove(0);
                "host surface field instruction is missing from extraction"
            }
            7 => {
                manifest.items[1].mapped_field = Some("host.model".into());
                "runtime_owned/unsupported items cannot map candidate fields"
            }
            _ => {
                manifest.items[0].consumer = None;
                "supported item needs a consumer"
            }
        };
        assert_invalid(manifest.validate_against_extraction(&extracted), expected);
        assert_invalid(
            manifest.validate_frozen_v1_against_extraction(&extracted),
            expected,
        );
    }
}

#[test]
fn v1_matching_consumers_keep_wire_clones_and_non_supported_semantics() {
    let original = surface();
    let original_bytes = serde_json::to_vec(&original).unwrap();
    assert_eq!(fingerprint(&original).unwrap(), REFERENCE_DIGEST);
    original.validate_against_extraction(&names()).unwrap();
    let mut clones = original.clone();
    clones.items.extend(original.items.clone());
    let mut extracted = names();
    extracted.extend(names());
    clones.validate_against_extraction(&extracted).unwrap();
    let bytes = serde_json::to_vec(&clones).unwrap();
    let decoded: HostSurfaceManifest = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), bytes);
    assert_eq!(
        fingerprint(&decoded).unwrap(),
        fingerprint(&clones).unwrap()
    );
    for index in [1, 2] {
        let mut manifest = original.clone();
        manifest.items[index].consumer = Some("historical-description".into());
        manifest.validate_against_extraction(&names()).unwrap();
        manifest.items[index].mapped_field = Some("skill.content".into());
        assert_invalid(
            manifest.validate_against_extraction(&names()),
            "runtime_owned/unsupported items cannot map candidate fields",
        );
    }
    assert_eq!(serde_json::to_vec(&surface()).unwrap(), original_bytes);
    assert_eq!(fingerprint(&surface()).unwrap(), REFERENCE_DIGEST);
}

#[test]
fn frozen_v1_validation_preserves_historical_wrong_consumer_bytes() {
    let historical: HostSurfaceManifest = serde_json::from_str(LEGACY_JSON).unwrap();
    let digest = hash(LEGACY_JSON.as_bytes());
    historical
        .validate_frozen_v1_against_extraction(&names())
        .unwrap();
    assert_invalid(historical.validate_against_extraction(&names()), MISMATCH);
    assert_eq!(serde_json::to_string(&historical).unwrap(), LEGACY_JSON);
    assert_eq!(fingerprint(&historical).unwrap(), digest);
    let mut malformed = historical.clone();
    malformed.items[0].consumer = Some(String::new());
    assert_invalid(
        malformed.validate_frozen_v1_against_extraction(&names()),
        "supported item needs a consumer",
    );
    malformed = historical.clone();
    malformed.items[0].reason.clear();
    assert_invalid(
        malformed.validate_frozen_v1_against_extraction(&names()),
        "reason: expected nonempty text <= 512 bytes",
    );
    assert_eq!(serde_json::to_string(&historical).unwrap(), LEGACY_JSON);
    assert_eq!(fingerprint(&historical).unwrap(), digest);
}
