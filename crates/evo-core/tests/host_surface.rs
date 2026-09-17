use evo_core::contract::{HostSurfaceManifest, SURFACE_SCHEMA};
use std::path::PathBuf;

#[test]
fn reference_host_fixture_covers_extracted_fields() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/reference-host/host_surface.v1.json");
    let raw = std::fs::read_to_string(path).unwrap();
    let manifest: HostSurfaceManifest = serde_json::from_str(&raw).unwrap();
    assert_eq!(manifest.schema_version, SURFACE_SCHEMA);
    manifest
        .validate_against_extraction(&["instruction".into(), "model".into(), "theme".into()])
        .unwrap();
}
