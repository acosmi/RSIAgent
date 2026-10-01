use evo_core::contract::{HostSurfaceManifest, SurfaceCoverage};
use evo_core::{Error, fingerprint};

const REFERENCE_JSON: &str = include_str!("../../../fixtures/reference-host/host_surface.v1.json");
const SERIALIZED_REFERENCE: &str = r#"{"schema_version":"rsia.host_surface.v1","host":"reference","host_version":"0.1.0","adapter_version":"0.1.0","source_digest":"reference_host_surface_v1","items":[{"name":"instruction","coverage":"supported","mapped_field":"skill.content","consumer":"compiler","reason":"run-level instruction projection"},{"name":"model","coverage":"runtime_owned","mapped_field":null,"consumer":null,"reason":"model identity is host-declared"},{"name":"theme","coverage":"unsupported","mapped_field":null,"consumer":null,"reason":"UI theme is out of scope"}]}"#;
const REFERENCE_DIGEST: &str = "e03c7b9dbe5566b5bcc0d7a74d78e66faee90960a424dfc04d07b03bbe7d7ac1";

fn surface() -> HostSurfaceManifest {
    serde_json::from_str(REFERENCE_JSON).unwrap()
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|name| (*name).into()).collect()
}

fn assert_invalid(result: evo_core::Result<()>, expected: &str) {
    assert!(
        matches!(result, Err(Error::Invalid(ref message)) if message == expected),
        "expected Invalid({expected:?}), got {result:?}"
    );
}

fn assert_missing(manifest: &HostSurfaceManifest, extracted: &[&str], missing: &str) {
    let extracted = names(extracted);
    let result = manifest.validate_against_extraction(&extracted);
    println!(
        "manifest_json={} manifest_digest={} extracted_json={} result={result:?}",
        serde_json::to_string(manifest).unwrap(),
        fingerprint(manifest).unwrap(),
        serde_json::to_string(&extracted).unwrap(),
    );
    assert_invalid(
        result,
        &format!("host surface field {missing} is missing from extraction"),
    );
}

#[test]
fn missing_supported_name_is_rejected() {
    assert_missing(&surface(), &["model", "theme"], "instruction");
}

#[test]
fn missing_runtime_owned_name_is_rejected() {
    assert_missing(&surface(), &["instruction", "theme"], "model");
}

#[test]
fn missing_unsupported_name_is_rejected() {
    assert_missing(&surface(), &["instruction", "model"], "theme");
}

#[test]
fn multiple_missing_names_are_rejected_with_nonempty_extraction() {
    assert_missing(&surface(), &["model"], "instruction");
}

#[test]
fn nonempty_prefix_truncation_is_rejected() {
    assert_missing(&surface(), &["instruction"], "model");
}

#[test]
fn rename_to_an_already_classified_name_is_rejected() {
    assert_missing(&surface(), &["model", "model", "theme"], "instruction");
}

#[test]
fn first_missing_name_follows_manifest_order() {
    let mut manifest = surface();
    manifest.items.reverse();
    assert_missing(&manifest, &["model"], "theme");
}

#[test]
fn complete_extraction_accepts_reordering() {
    let manifest = surface();
    manifest
        .validate_against_extraction(&names(&["theme", "instruction", "model"]))
        .unwrap();
    let mut reordered = manifest.clone();
    reordered.items.reverse();
    reordered
        .validate_against_extraction(&names(&["model", "theme", "instruction"]))
        .unwrap();
    // Membership is order-independent; the serialized manifest digest is not.
    assert_ne!(
        fingerprint(&manifest).unwrap(),
        fingerprint(&reordered).unwrap()
    );
}

#[test]
fn complete_duplicate_names_keep_existing_behavior() {
    let mut manifest = surface();
    manifest.items.push(manifest.items[0].clone());
    manifest
        .validate_against_extraction(&names(&["instruction", "model", "theme", "model"]))
        .unwrap();
}

#[test]
fn serialized_reference_manifest_and_fingerprint_are_unchanged() {
    let manifest = surface();
    assert_eq!(
        serde_json::to_string(&manifest).unwrap(),
        SERIALIZED_REFERENCE
    );
    assert_eq!(fingerprint(&manifest).unwrap(), REFERENCE_DIGEST);
    manifest
        .validate_against_extraction(&names(&["instruction", "model", "theme"]))
        .unwrap();
    println!("reference_json={SERIALIZED_REFERENCE} reference_digest={REFERENCE_DIGEST}");
}

#[test]
fn schema_and_empty_extraction_errors_keep_priority() {
    let mut manifest = surface();
    manifest.schema_version = "rsia.host_surface.unknown".into();
    manifest.items[0].consumer = None;
    assert_invalid(
        manifest.validate_against_extraction(&names(&["new_name"])),
        "unsupported host surface schema",
    );
    let mut manifest = surface();
    manifest.items[0].consumer = None;
    assert_invalid(
        manifest.validate_against_extraction(&[]),
        "empty extraction is not a successful cover",
    );
}

#[test]
fn unclassified_name_errors_keep_priority_and_exact_spelling() {
    for name in ["new_name", "Model", "modél"] {
        let mut manifest = surface();
        manifest.items[0].consumer = None;
        assert_invalid(
            manifest.validate_against_extraction(&names(&[name])),
            &format!("extracted field {name} is unclassified"),
        );
    }
}

#[test]
fn supported_item_errors_keep_priority_even_when_that_item_is_missing() {
    let extracted = names(&["model", "theme"]);
    let mut manifest = surface();
    manifest.items[0].mapped_field = None;
    assert_invalid(
        manifest.validate_against_extraction(&extracted),
        "supported item needs mapped_field",
    );
    manifest.items[0].mapped_field = Some("unknown.field".into());
    assert_invalid(
        manifest.validate_against_extraction(&extracted),
        "no consumer for field unknown.field",
    );
    manifest.items[0].mapped_field = Some("skill.content".into());
    for consumer in [None, Some(String::new())] {
        manifest.items[0].consumer = consumer;
        assert_invalid(
            manifest.validate_against_extraction(&extracted),
            "supported item needs a consumer",
        );
    }
}

#[test]
fn runtime_owned_and_unsupported_mapping_errors_keep_priority() {
    for (index, coverage, extracted) in [
        (
            1,
            SurfaceCoverage::RuntimeOwned,
            vec!["instruction", "theme"],
        ),
        (
            2,
            SurfaceCoverage::Unsupported,
            vec!["instruction", "model"],
        ),
    ] {
        let mut manifest = surface();
        assert_eq!(manifest.items[index].coverage, coverage);
        manifest.items[index].mapped_field = Some("skill.content".into());
        assert_invalid(
            manifest.validate_against_extraction(&names(&extracted)),
            "runtime_owned/unsupported items cannot map candidate fields",
        );
    }
}

#[test]
fn item_identifier_and_reason_errors_keep_priority() {
    let extracted = names(&["instruction", "theme"]);
    let mut manifest = surface();
    manifest.items[1].name = "bad name".into();
    assert_invalid(
        manifest.validate_against_extraction(&extracted),
        "invalid identifier",
    );
    let mut manifest = surface();
    manifest.items[1].reason.clear();
    assert_invalid(
        manifest.validate_against_extraction(&extracted),
        "reason: expected nonempty text <= 512 bytes",
    );
    manifest.host = "bad host".into();
    assert_invalid(
        manifest.validate_against_extraction(&extracted),
        "invalid identifier",
    );
}

#[test]
fn unknown_wire_fields_and_unknown_coverage_are_rejected() {
    let original: serde_json::Value = serde_json::from_str(REFERENCE_JSON).unwrap();
    let mut manifest = original.clone();
    manifest["new_wire_field"] = true.into();
    assert!(serde_json::from_value::<HostSurfaceManifest>(manifest).is_err());
    let mut manifest = original.clone();
    manifest["items"][0]["new_wire_field"] = true.into();
    assert!(serde_json::from_value::<HostSurfaceManifest>(manifest).is_err());
    let mut manifest = original;
    manifest["items"][0]["coverage"] = "new_coverage".into();
    assert!(serde_json::from_value::<HostSurfaceManifest>(manifest).is_err());
}
