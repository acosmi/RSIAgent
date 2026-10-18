//! Pure old-path compatibility checks; these synthetic vectors are not historical records.
use evo_core::contract::{CapabilityLevel, HostCapabilities, SystemSnapshot};
use evo_core::prepare_contract::{HostPrepareConfig as SharedHostPrepareConfig, PrepareOperation};
use evo_core::{Error, Prepare, Validate, fingerprint, hash};
use evo_engine::service::HostPrepareConfig;
use serde_json::json;

const CONFIG_BYTES: &str = r#"{"profile_id":"profile-a","system_snapshot":{"schema_version":"rsia.system_snapshot.v2","profile_id":"profile-a","host_id":"host-a","host_version":"1.0 café","model_id":"model-a","tools":["tool-z","tool-a"],"mandatory_context_digest":"c15677734a3888998ee6488dc8a675b2d7025f1012d62e0fad9c48c9755380f6"},"host_surface_id":"surface-a","host_capabilities":{"available":["tool-a","tool-m","tool-z"],"granted":["tool-a","tool-z"]},"evolution_enabled":true,"capability_level":"tool_only"}"#;
const REQUEST_BYTES: &str = r#"{"request_key":"prepare-a","goal":"检查 café\r\n保留 é","capabilities":["tool-z","tool-a"]}"#;
const OPERATION_BYTES: &str = r#"{"request":{"request_key":"prepare-a","goal":"检查 café\r\n保留 é","capabilities":["tool-z","tool-a"]},"config":{"profile_id":"profile-a","system_snapshot":{"schema_version":"rsia.system_snapshot.v2","profile_id":"profile-a","host_id":"host-a","host_version":"1.0 café","model_id":"model-a","tools":["tool-z","tool-a"],"mandatory_context_digest":"c15677734a3888998ee6488dc8a675b2d7025f1012d62e0fad9c48c9755380f6"},"host_surface_id":"surface-a","host_capabilities":{"available":["tool-a","tool-m","tool-z"],"granted":["tool-a","tool-z"]},"evolution_enabled":true,"capability_level":"tool_only"}}"#;

fn config() -> HostPrepareConfig {
    HostPrepareConfig {
        profile_id: "profile-a".into(),
        system_snapshot: SystemSnapshot {
            schema_version: "rsia.system_snapshot.v2".into(),
            profile_id: "profile-a".into(),
            host_id: "host-a".into(),
            host_version: "1.0 café".into(),
            model_id: "model-a".into(),
            tools: vec!["tool-z".into(), "tool-a".into()],
            mandatory_context_digest: hash(b"mandatory context\n"),
        },
        host_surface_id: "surface-a".into(),
        host_capabilities: HostCapabilities {
            available: ["tool-z".into(), "tool-a".into(), "tool-m".into()].into(),
            granted: ["tool-z".into(), "tool-a".into()].into(),
        },
        evolution_enabled: true,
        capability_level: CapabilityLevel::ToolOnly,
    }
}

fn request() -> Prepare {
    Prepare {
        request_key: "prepare-a".into(),
        goal: "检查 café\r\n保留 e\u{301}".into(),
        capabilities: vec!["tool-z".into(), "tool-a".into()],
    }
}

#[test]
fn old_service_import_and_core_contract_are_the_same_concrete_type() {
    let old: HostPrepareConfig = config();
    old.validate().unwrap();
    let shared: SharedHostPrepareConfig = old;
    let returned: HostPrepareConfig = shared;
    let _: &SharedHostPrepareConfig = &returned;
    assert_eq!(
        serde_json::to_vec(&returned).unwrap(),
        CONFIG_BYTES.as_bytes()
    );
}

#[test]
fn old_config_and_shared_operation_keep_original_typed_bytes_and_hashes() {
    let config: HostPrepareConfig = config();
    let request = request();
    config.validate().unwrap();
    request.validate().unwrap();
    assert_eq!(
        serde_json::to_vec(&config).unwrap(),
        CONFIG_BYTES.as_bytes()
    );
    assert_eq!(
        serde_json::to_vec(&request).unwrap(),
        REQUEST_BYTES.as_bytes()
    );
    assert_eq!(
        fingerprint(&config).unwrap(),
        "99d0cd324d55fc6c112971bc107d18f114fa9f68e56aa78e6c08aab60209b93b"
    );
    let operation = PrepareOperation {
        request: &request,
        config: &config,
    };
    assert_eq!(
        serde_json::to_vec(&operation).unwrap(),
        OPERATION_BYTES.as_bytes()
    );
    assert_eq!(
        fingerprint(&operation).unwrap(),
        "f23d95bf3549269e929bf91b1e61811bd7b915867ef146155f96f1b3f489c7af"
    );
}

fn refusal(config: &HostPrepareConfig) -> (&'static str, String) {
    match config.validate() {
        Err(Error::Conflict(message)) => ("Conflict", message),
        Err(Error::Invalid(message)) => ("Invalid", message),
        result => panic!("expected original validation refusal, got {result:?}"),
    }
}

#[test]
fn old_validation_entry_keeps_exact_refusal_classes_and_messages() {
    type Mutation = fn(&mut HostPrepareConfig);
    let cases: [(Mutation, &str, &str); 8] = [
        (
            |v| v.system_snapshot.profile_id = "profile-b".into(),
            "Conflict",
            "system snapshot profile mismatch",
        ),
        (
            |v| v.system_snapshot.schema_version = "unsupported".into(),
            "Invalid",
            "unsupported system snapshot schema",
        ),
        (
            |v| v.system_snapshot.tools.push("tool-z".into()),
            "Conflict",
            "duplicate host tool",
        ),
        (
            |v| v.system_snapshot.mandatory_context_digest = "A".repeat(64),
            "Invalid",
            "mandatory_context_digest: expected lowercase sha256 digest",
        ),
        (
            |v| {
                v.host_capabilities.granted.insert("unavailable".into());
            },
            "Conflict",
            "granted capabilities must be available",
        ),
        (
            |v| v.host_surface_id = "bad id".into(),
            "Invalid",
            "invalid identifier",
        ),
        (
            |v| v.system_snapshot.host_version.clear(),
            "Invalid",
            "host_version: expected nonempty text <= 128 bytes",
        ),
        (
            |v| v.system_snapshot.tools = (0..65).map(|i| format!("tool-{i}")).collect(),
            "Invalid",
            "too many host tools",
        ),
    ];
    for (mutate, kind, message) in cases {
        let mut value = config();
        mutate(&mut value);
        assert_eq!(refusal(&value), (kind, message.to_string()));
    }
    config().validate().unwrap();
}

#[test]
fn old_import_keeps_strict_nested_shapes_and_existing_collection_semantics() {
    for object in [None, Some("system_snapshot"), Some("host_capabilities")] {
        let mut value = serde_json::to_value(config()).unwrap();
        let target = match object {
            Some(field) => &mut value[field],
            None => &mut value,
        };
        target
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), json!(true));
        let error = serde_json::from_value::<HostPrepareConfig>(value).unwrap_err();
        assert!(
            error.to_string().starts_with("unknown field `unexpected`"),
            "{error}"
        );
    }
    let mut value = serde_json::to_value(config()).unwrap();
    value["host_capabilities"]["available"] = json!(["tool-z", "tool-a", "tool-m", "tool-z"]);
    value["host_capabilities"]["granted"] = json!(["tool-z", "tool-a", "tool-z"]);
    let old: HostPrepareConfig = serde_json::from_value(value).unwrap();
    old.validate().unwrap();
    assert_eq!(serde_json::to_vec(&old).unwrap(), CONFIG_BYTES.as_bytes());
    assert_eq!(old.system_snapshot.tools, ["tool-z", "tool-a"]);
}

#[test]
fn legacy_operation_borrows_the_unsorted_vectors_and_utf8_goal() {
    let mut config = config();
    let mut request = request();
    assert_eq!(
        request.goal.as_bytes(),
        "检查 café\r\n保留 e\u{301}".as_bytes()
    );
    request.capabilities.reverse();
    let changed_request = PrepareOperation {
        request: &request,
        config: &config,
    };
    assert_ne!(
        serde_json::to_vec(&changed_request).unwrap(),
        OPERATION_BYTES.as_bytes()
    );
    request.capabilities.reverse();
    config.system_snapshot.tools.reverse();
    let changed_tools = PrepareOperation {
        request: &request,
        config: &config,
    };
    assert_ne!(
        serde_json::to_vec(&changed_tools).unwrap(),
        OPERATION_BYTES.as_bytes()
    );
}
