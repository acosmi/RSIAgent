use evo_core::Error;
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceClassificationV2, HostSurfaceManifestV2,
    HostSurfaceRegistrationV2, MODEL_TOOLS, ReviewedHostSourceEnvelopeV1, SurfaceCoverage,
    field_contract, host_surface_v2_allows_field, host_surface_v2_from_source,
    validate_host_capability_level, validate_host_surface_v2_new_use,
};

fn manifest(name: &str, coverage: SurfaceCoverage, field: &str) -> HostSurfaceManifestV2 {
    let supported = coverage == SurfaceCoverage::Supported;
    let source = ReviewedHostSourceEnvelopeV1 {
        schema_version: "rsia.host_source_envelope.v1".into(),
        source_kind: "reviewed_fixture".into(),
        source_schema: "rsia.host_reviewed_declarations.v1".into(),
        extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8: format!(
            r#"{{"schema_version":"rsia.host_reviewed_declarations.v1","host":"software-host","host_version":"1","declarations":[{{"name":"{name}","value_type":"string","enum_values":[],"default":{{"kind":"none"}}}}]}}"#
        ),
    };
    let registration = HostSurfaceRegistrationV2 {
        adapter_version: "1".into(),
        classifications: vec![HostSurfaceClassificationV2 {
            name: name.into(),
            coverage,
            mapped_field: supported.then(|| field.into()),
            consumer: supported.then(|| field_contract(field).unwrap().consumer.into()),
            reason: "local software contract; no native host claim".into(),
            renamed_from: None,
            test_ref: supported.then(|| "software projection mutation".into()),
            capability: supported.then(|| "fs_read".into()),
        }],
        retired: vec![],
    };
    host_surface_v2_from_source(&source, &registration)
        .unwrap()
        .0
}

fn capabilities() -> HostCapabilities {
    HostCapabilities {
        available: ["fs_read".into()].into(),
        granted: ["fs_read".into()].into(),
    }
}

#[test]
fn tool_only_manifest_excludes_rsia_protocol_names() {
    for name in MODEL_TOOLS {
        for coverage in [
            SurfaceCoverage::RuntimeOwned,
            SurfaceCoverage::Unsupported,
            SurfaceCoverage::Supported,
        ] {
            // The frozen source algorithm still reads these historical names.
            let historical = manifest(name, coverage, "skill.content");
            let before = serde_json::to_vec(&historical).unwrap();
            assert!(matches!(validate_host_surface_v2_new_use(&historical),
                Err(Error::Invalid(reason)) if reason == "host_surface_rsia_tool_is_not_configuration"));
            assert_eq!(serde_json::to_vec(&historical).unwrap(), before);
        }
    }
    validate_host_surface_v2_new_use(&manifest("model", SurfaceCoverage::RuntimeOwned, ""))
        .unwrap();
}

#[test]
fn tool_only_capability_accepts_only_zero_supported() {
    let caps = capabilities();
    for coverage in [SurfaceCoverage::RuntimeOwned, SurfaceCoverage::Unsupported] {
        validate_host_capability_level(
            &manifest("model", coverage, ""),
            &caps,
            CapabilityLevel::ToolOnly,
        )
        .unwrap();
    }
    for field in ["host.model", "skill.content"] {
        assert!(matches!(validate_host_capability_level(
            &manifest("setting", SurfaceCoverage::Supported, field), &caps, CapabilityLevel::ToolOnly),
            Err(Error::Conflict(reason)) if reason == "host_capability_tool_only_requires_zero_supported"));
    }
}

#[test]
fn attached_requires_v2_supported_skill_content() {
    let caps = capabilities();
    let good = manifest("instruction", SurfaceCoverage::Supported, "skill.content");
    validate_host_capability_level(&good, &caps, CapabilityLevel::Attached).unwrap();
    let mut missing_test = good.clone();
    missing_test.items[0].test_ref = None;
    let mut wrong_consumer = good.clone();
    wrong_consumer.items[0].consumer = Some("runner".into());
    let mut legacy = good.clone();
    legacy.schema_version = "rsia.host_surface.v1".into();
    for invalid in [
        manifest("model", SurfaceCoverage::RuntimeOwned, ""),
        manifest("model", SurfaceCoverage::Supported, "host.model"),
        missing_test,
        wrong_consumer,
        legacy,
    ] {
        assert!(
            matches!(validate_host_capability_level(&invalid, &caps, CapabilityLevel::Attached),
            Err(Error::Conflict(reason)) if reason == "host_capability_attached_requires_skill_content")
        );
    }
}

#[test]
fn attached_uses_available_and_granted_skill_content_capability() {
    let surface = manifest("instruction", SurfaceCoverage::Supported, "skill.content");
    let caps = capabilities();
    validate_host_capability_level(&surface, &caps, CapabilityLevel::Attached).unwrap();
    assert!(host_surface_v2_allows_field(
        &surface,
        &caps,
        "skill.content"
    ));
    for missing in [
        HostCapabilities {
            available: caps.available.clone(),
            granted: Default::default(),
        },
        HostCapabilities {
            available: Default::default(),
            granted: caps.granted.clone(),
        },
    ] {
        assert!(
            matches!(validate_host_capability_level(&surface, &missing, CapabilityLevel::Attached),
            Err(Error::Conflict(reason)) if reason == "host_capability_attached_requires_skill_content")
        );
        assert!(!host_surface_v2_allows_field(
            &surface,
            &missing,
            "skill.content"
        ));
    }
}
