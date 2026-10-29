//! Complete content contracts only. These values grant no OS/Store authority.
use evo_core::contract::*;
use evo_core::prepare_contract::{
    CompleteHostPrepareConfig, CompletePrepareOperation, HostPrepareConfig, PrepareOperation,
};
use evo_core::{Prepare, Strategy, canonical_fingerprint, fingerprint, hash};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn source() -> ToolSourceV1 {
    ToolSourceV1 {
        schema_version: TOOL_SOURCE_SCHEMA.into(),
        domain: "mcp_protocol".into(),
        producer_id: "rmcp.ToolRouter.list_all".into(),
        producer_version: "rmcp-3.3.0".into(),
        definitions: serde_json::from_str(include_str!(
            "../../evo-engine/tests/fixtures/ag168-actual-mcp-descriptors.json"
        ))
        .unwrap(),
    }
}
fn caps() -> HostCapabilities {
    HostCapabilities {
        available: ["fs_read".into()].into(),
        granted: BTreeSet::new(),
    }
}
fn snapshot() -> SystemSnapshotV3 {
    let source = source();
    SystemSnapshotV3 {
        schema_version: SYSTEM_SNAPSHOT_SCHEMA_V3.into(),
        profile_id: "p1".into(),
        host_id: "software-protocol-fixture".into(),
        host_version: "1.0.0".into(),
        model_id: "disabled".into(),
        tools: source.names().unwrap(),
        mandatory_context_digest: hash(b"mandatory\r\n"),
        tool_source: source,
    }
}
fn field<T: DeserializeOwned>(v: &Value, name: &str) -> T {
    serde_json::from_value(v[name].clone()).unwrap()
}
fn old_baseline() -> FrozenCompilerProfileContentV1 {
    let v: Value = serde_json::from_str(include_str!(
        "../../evo-engine/tests/fixtures/ag168-legacy-frozen-profile-v1.raw.json"
    ))
    .unwrap();
    FrozenCompilerProfileContentV1 {
        id: field(&v, "id"),
        schema_version: field(&v, "schema_version"),
        namespace: field(&v, "namespace"),
        profile_id: field(&v, "profile_id"),
        version: field(&v, "version"),
        host_registration_id: field(&v, "host_registration_id"),
        provider_registration_id: field(&v, "provider_registration_id"),
        resource_instrumentation_id: field(&v, "resource_instrumentation_id"),
        system_snapshot: field(&v, "system_snapshot"),
        capabilities: field(&v, "capabilities"),
        environment_digest: field(&v, "environment_digest"),
        baseline_skill: field(&v, "baseline_skill"),
        baseline_strategy: field(&v, "baseline_strategy"),
        dependencies: vec![],
        purpose: field(&v, "purpose"),
        application_order: field(&v, "application_order"),
        authorized_by: field(&v, "authorized_by"),
    }
}
fn original_root() -> EmptyRootContentV1 {
    let v: Value = serde_json::from_str(include_str!(
        "../../evo-engine/tests/fixtures/ag168-legacy-authenticated-bundle-v2-candidate.raw.json"
    ))
    .unwrap();
    let v = &v["original_parent"]["root"];
    EmptyRootContentV1 {
        id: field(v, "id"),
        schema_version: field(v, "schema_version"),
        namespace: field(v, "namespace"),
        profile_registration_id: field(v, "profile_registration_id"),
        profile_digest: field(v, "profile_digest"),
        host_registration_id: field(v, "host_registration_id"),
        skill: field(v, "skill"),
        strategy: field(v, "strategy"),
        authorized_by: field(v, "authorized_by"),
    }
}
fn baseline() -> FrozenCompilerProfileContentV2 {
    let old = old_baseline();
    let system_snapshot = snapshot();
    let capabilities = caps();
    FrozenCompilerProfileContentV2 {
        id: "complete-content-profile".into(),
        schema_version: "rsia.frozen_compiler_profile.v2".into(),
        namespace: old.namespace,
        profile_id: old.profile_id,
        version: old.version,
        host_registration_id: old.host_registration_id,
        provider_registration_id: old.provider_registration_id,
        resource_instrumentation_id: old.resource_instrumentation_id,
        environment_digest: system_snapshot.environment_digest(&capabilities).unwrap(),
        system_snapshot,
        capabilities,
        baseline_skill: old.baseline_skill,
        baseline_strategy: old.baseline_strategy,
        dependencies: old.dependencies,
        purpose: old.purpose,
        application_order: old.application_order,
        authorized_by: old.authorized_by,
    }
}
fn proof(b: FrozenCompilerProfileContentV2) -> CompleteProfileContentProofV3 {
    let mut root = original_root();
    root.profile_registration_id = b.id.clone();
    root.profile_digest = fingerprint(&b).unwrap();
    root.host_registration_id = b.host_registration_id.clone();
    let p = Profile {
        id: b.profile_id.clone(),
        evolution_enabled: true,
        parent_digest: fingerprint(&root).unwrap(),
        baseline_digest: fingerprint(&b).unwrap(),
    };
    CompleteProfileContentProofV3::from_content(p, ParentContentV3::EmptyRoot(root), b).unwrap()
}

#[test]
fn snapshot_v3_commits_every_descriptor_field_and_array_order() {
    let original = snapshot();
    original.validate().unwrap();
    let before = original.digest().unwrap();
    let environment = original.environment_digest(&caps()).unwrap();
    for mutation in 0..5 {
        let mut changed = original.clone();
        match mutation {
            0 => changed.tool_source.definitions[0]["description"] = json!("changed description"),
            1 => {
                changed.tool_source.definitions[0]["inputSchema"]["properties"]["nested"] =
                    json!({"type":"array","items":{"enum":["b","a"]}})
            }
            2 => {
                changed.tool_source.definitions[0]["annotations"] =
                    json!({"readOnlyHint":true,"nested":{"title":"complete annotation"}})
            }
            3 => {
                changed.tool_source.definitions.reverse();
                changed.tools = changed.tool_source.names().unwrap();
            }
            _ => {
                changed.tool_source.definitions[0]["_meta"] =
                    json!({"ordered":[2,1],"raw":"e\u{301}\r\n"})
            }
        }
        changed.validate().unwrap();
        assert_ne!(changed.digest().unwrap(), before);
        assert_ne!(changed.environment_digest(&caps()).unwrap(), environment);
    }
    assert_eq!(
        before,
        canonical_fingerprint(&("rsia.system_snapshot.content.v3", &original)).unwrap()
    );
    let raw = serde_json::to_string(&original).unwrap();
    let loaded: LoadedSystemSnapshot = serde_json::from_str(&raw).unwrap();
    assert_eq!(serde_json::to_string(&loaded).unwrap(), raw);
    let duplicate = raw.replacen(
        "\"description\":",
        "\"description\":\"first\",\"description\":",
        1,
    );
    assert!(serde_json::from_str::<LoadedSystemSnapshot>(&duplicate).is_err());
}

#[test]
fn snapshot_v3_domains_limits_and_raw_capabilities_are_bound() {
    let original = snapshot();
    let a = caps();
    let mut b = a.clone();
    b.available.insert("network".into());
    assert_eq!(a.effective(), b.effective());
    assert_ne!(
        original.environment_digest(&a).unwrap(),
        original.environment_digest(&b).unwrap()
    );
    let mut c = a.clone();
    c.granted.insert("fs_read".into());
    assert_ne!(
        original.environment_digest(&a).unwrap(),
        original.environment_digest(&c).unwrap()
    );
    let mut unknown = original.clone();
    unknown.tool_source.domain = "external_native".into();
    assert!(unknown.validate().is_err());
    let mut oversized = original.clone();
    oversized.tool_source.definitions[0]["description"] = json!("x".repeat(V1_TOOL_DESCRIPTOR_MAX));
    assert!(oversized.validate().is_err());
    let mut wrong_order = original.clone();
    wrong_order.tools.reverse();
    assert!(wrong_order.validate().is_err());
    let adapter = evo_core::model_protocol::registered_chat_wire_v1_tool_source();
    adapter.validate().unwrap();
    let mut native = original.clone();
    native.tools.clear();
    native.tool_source = adapter.clone();
    evo_core::model_protocol::verify_registered_chat_wire_v1_snapshot(&native).unwrap();
    native
        .tool_source
        .definitions
        .push(json!({"name":"native_tool"}));
    assert!(evo_core::model_protocol::verify_registered_chat_wire_v1_snapshot(&native).is_err());
    let http = ToolSourceV1 { schema_version: TOOL_SOURCE_SCHEMA.into(), domain: "http_protocol".into(), producer_id: "evo-http.typed_routes".into(), producer_version: "rsia.http_typed_routes.v1".into(), definitions: MODEL_TOOLS.into_iter().zip(["/v1/tools/prepare","/v1/tools/feedback","/v1/tools/propose","/v1/tools/inspect"]).map(|(name,path)|json!({"name":name,"path":path,"method":"POST","input_schema":{"type":"object"}})).collect() };
    http.validate().unwrap();
    assert_ne!(fingerprint(&http).unwrap(), fingerprint(&adapter).unwrap());
}

#[test]
fn complete_profile_mirror_keeps_all_seventeen_original_fields() {
    let b = baseline();
    let original = fingerprint(&b).unwrap();
    let value = serde_json::to_value(&b).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 17);
    for name in [
        "id",
        "schema_version",
        "namespace",
        "profile_id",
        "version",
        "host_registration_id",
        "provider_registration_id",
        "resource_instrumentation_id",
        "system_snapshot",
        "capabilities",
        "environment_digest",
        "baseline_skill",
        "baseline_strategy",
        "dependencies",
        "purpose",
        "application_order",
        "authorized_by",
    ] {
        let mut changed = value.clone();
        changed[name] = json!({"original":changed[name].clone(),"changed":true});
        assert_ne!(
            canonical_fingerprint(&changed).unwrap(),
            canonical_fingerprint(&value).unwrap(),
            "{name}"
        );
    }
    let content = proof(b.clone());
    assert_eq!(fingerprint(content.baseline_content()).unwrap(), original);
    let mut changed = b.clone();
    changed.system_snapshot.tool_source.definitions[0]["description"] = json!("nested change");
    assert!(
        CompleteProfileContentProofV3::from_content(
            content.profile().clone(),
            ParentContentV3::EmptyRoot(original_root()),
            changed
        )
        .is_err()
    );
    assert_eq!(content.profile().baseline_digest, original);
}

#[test]
fn complete_profile_replay_keeps_old_pure_content_bytes() {
    let old = old_baseline();
    assert_eq!(
        fingerprint(&old).unwrap(),
        "7c4bf89fcfdbb89012a5d9bd8068598de003796a76dc2a3a5f64e03ab6ca0fd6"
    );
    let raw: Value = serde_json::from_str(include_str!(
        "../../evo-engine/tests/fixtures/ag168-legacy-authenticated-bundle-v2-candidate.raw.json"
    ))
    .unwrap();
    let p: Profile = field(&raw["input"], "profile");
    let bundle: ResolvedBundleV3 = field(&raw, "bundle");
    let old_proof =
        ProfileContentProofV3::from_content(p, ParentContentV3::EmptyRoot(original_root()), old)
            .unwrap();
    let bytes = serde_json::to_vec(&bundle).unwrap();
    replay_resolved_bundle_v3(&bundle, &old_proof, &BTreeSet::new()).unwrap();
    assert_eq!(serde_json::to_vec(&bundle).unwrap(), bytes);
    assert_eq!(
        bundle.digest,
        "6f10197b6c77393850f1a0482966d4175a4b052989edb09bd599b127c3b600b9"
    );
    let complete = proof(baseline());
    let new = compile_complete_bundle_v3(CompleteCompilePartsV3 {
        content: &complete,
        skill_patch: &SkillPatch::default(),
        improver_patch: &ImproverPatch::default(),
        revoked: &BTreeSet::new(),
    })
    .unwrap();
    replay_complete_resolved_bundle_v3(&new, &complete, &BTreeSet::new()).unwrap();
    assert_eq!(new.origins.len(), 8);
    assert_ne!(new.baseline_digest, bundle.baseline_digest);
    assert_eq!(new.compiler_version, bundle.compiler_version);
    assert_eq!(new.schema_version, bundle.schema_version);
}

#[test]
fn complete_prepare_operation_versions_identity_without_changing_old_vectors() {
    let legacy = HostPrepareConfig {
        profile_id: "p1".into(),
        system_snapshot: SystemSnapshot {
            schema_version: "rsia.system_snapshot.v2".into(),
            profile_id: "p1".into(),
            host_id: "host".into(),
            host_version: "1".into(),
            model_id: "disabled".into(),
            tools: vec!["read_config".into()],
            mandatory_context_digest: hash(b"mandatory"),
        },
        host_surface_id: "surface".into(),
        host_capabilities: caps(),
        evolution_enabled: false,
        capability_level: CapabilityLevel::ToolOnly,
    };
    legacy.validate().unwrap();
    let request = Prepare {
        request_key: "key".into(),
        goal: "goal\r\n".into(),
        capabilities: vec![],
    };
    let original = serde_json::to_vec(&PrepareOperation {
        request: &request,
        config: &legacy,
    })
    .unwrap();
    assert!(original.starts_with(b"{\"request\":{\"request_key\":\"key\""));
    let complete = CompleteHostPrepareConfig::from_producer(&legacy, source()).unwrap();
    let new = CompletePrepareOperation {
        schema_version: "rsia.prepare_operation.v3",
        request: &request,
        config: &complete,
    };
    assert_ne!(fingerprint(&new).unwrap(), hash(&original));
    assert_eq!(
        serde_json::to_vec(&PrepareOperation {
            request: &request,
            config: &legacy
        })
        .unwrap(),
        original
    );
    let mut changed = complete.clone();
    changed.system_snapshot.tool_source.definitions[0]["description"] = json!("another schema");
    assert_ne!(
        fingerprint(&CompletePrepareOperation {
            schema_version: "rsia.prepare_operation.v3",
            request: &request,
            config: &changed
        })
        .unwrap(),
        fingerprint(&new).unwrap()
    );
    let mut bad = legacy;
    bad.system_snapshot.schema_version = SYSTEM_SNAPSHOT_SCHEMA_V3.into();
    assert!(bad.validate().is_err());
    assert_eq!(
        fingerprint(&Strategy::default()).unwrap(),
        fingerprint(&old_baseline().baseline_strategy).unwrap()
    );
}
