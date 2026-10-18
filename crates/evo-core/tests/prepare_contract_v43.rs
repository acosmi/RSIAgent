//! Synthetic vectors frozen from the pre-move typed field order, not stored records.
use evo_core::contract::{CapabilityLevel, HostCapabilities, SystemSnapshot};
use evo_core::prepare_contract::{HostPrepareConfig, PrepareOperation};
use evo_core::{Error, Prepare, Validate, fingerprint, hash};
use serde_json::{Value, json};

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

fn invalid(value: &HostPrepareConfig, expected: &str) {
    match value.validate() {
        Err(Error::Invalid(message)) => assert_eq!(message, expected),
        result => panic!("expected Invalid({expected:?}), got {result:?}"),
    }
}

fn conflict(value: &HostPrepareConfig, expected: &str) {
    match value.validate() {
        Err(Error::Conflict(message)) => assert_eq!(message, expected),
        result => panic!("expected Conflict({expected:?}), got {result:?}"),
    }
}

#[test]
fn complete_config_keeps_typed_field_bytes_and_fingerprint() {
    let config = config();
    config.validate().unwrap();
    assert_eq!(
        serde_json::to_vec(&config).unwrap(),
        CONFIG_BYTES.as_bytes()
    );
    assert_eq!(
        fingerprint(&config).unwrap(),
        "99d0cd324d55fc6c112971bc107d18f114fa9f68e56aa78e6c08aab60209b93b"
    );
    assert_eq!(
        config.system_snapshot.digest().unwrap(),
        "d4cdb23b9e21eb819a232bc9c8fa804ed059bcbba074b81f14042559d4b40969"
    );
}

#[test]
fn borrowed_operation_keeps_request_then_config_bytes_and_fingerprint() {
    let config = config();
    let request = request();
    config.validate().unwrap();
    request.validate().unwrap();
    assert_eq!(
        serde_json::to_vec(&request).unwrap(),
        REQUEST_BYTES.as_bytes()
    );
    assert_eq!(
        fingerprint(&request).unwrap(),
        "5d5b40d0437e6890401840d983d1db2fd1285c21a7ed93bb4fb7db5c09e4c69d"
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

#[test]
fn utf8_line_endings_and_both_vector_orders_remain_semantic() {
    let mut config = config();
    let mut request = request();
    let baseline = fingerprint(&PrepareOperation {
        request: &request,
        config: &config,
    })
    .unwrap();
    request.capabilities.reverse();
    assert_ne!(
        fingerprint(&PrepareOperation {
            request: &request,
            config: &config
        })
        .unwrap(),
        baseline
    );
    request.capabilities.reverse();
    config.system_snapshot.tools.reverse();
    assert_ne!(
        fingerprint(&PrepareOperation {
            request: &request,
            config: &config
        })
        .unwrap(),
        baseline
    );
    config.system_snapshot.tools.reverse();
    request.goal = request.goal.replace("\r\n", "\n");
    assert_ne!(
        fingerprint(&PrepareOperation {
            request: &request,
            config: &config
        })
        .unwrap(),
        baseline
    );
    request = self::request();
    request.goal = request.goal.replace("e\u{301}", "é");
    assert_ne!(
        fingerprint(&PrepareOperation {
            request: &request,
            config: &config
        })
        .unwrap(),
        baseline
    );
}

#[test]
fn original_boolean_enum_and_empty_collection_configurations_are_accepted() {
    for enabled in [false, true] {
        for level in [CapabilityLevel::ToolOnly, CapabilityLevel::Attached] {
            let mut value = config();
            value.evolution_enabled = enabled;
            value.capability_level = level;
            value.system_snapshot.tools.clear();
            value.host_capabilities.available.clear();
            value.host_capabilities.granted.clear();
            value.validate().unwrap();
        }
    }
}

#[test]
fn original_identifier_text_tool_and_unbounded_capability_limits_are_accepted() {
    let mut value = config();
    value.profile_id = "p".repeat(128);
    value.system_snapshot.profile_id = value.profile_id.clone();
    value.host_surface_id = "s".repeat(128);
    value.system_snapshot.host_id = "h".repeat(128);
    value.system_snapshot.model_id = "m".repeat(128);
    value.system_snapshot.host_version = "é".repeat(64);
    value.system_snapshot.tools = (0..64).map(|i| format!("tool-{i}")).collect();
    value.host_capabilities.available = (0..65).map(|i| format!("capability-{i}")).collect();
    value.host_capabilities.granted = value.host_capabilities.available.clone();
    value.validate().unwrap();
}

#[test]
fn every_identifier_consumer_keeps_the_original_refusal() {
    let mutations: [fn(&mut HostPrepareConfig); 7] = [
        |v| v.profile_id = "bad id".into(),
        |v| v.host_surface_id = "bad id".into(),
        |v| v.system_snapshot.host_id = "bad id".into(),
        |v| v.system_snapshot.model_id = "bad id".into(),
        |v| v.system_snapshot.tools[0] = "bad id".into(),
        |v| {
            v.host_capabilities.available.insert("bad id".into());
        },
        |v| {
            v.host_capabilities.granted.insert("bad id".into());
        },
    ];
    for mutate in mutations {
        let mut value = config();
        mutate(&mut value);
        invalid(&value, "invalid identifier");
    }
    for bad in [
        String::new(),
        " \t".into(),
        "bad\0id".into(),
        "a".repeat(129),
    ] {
        let mut value = config();
        value.profile_id = bad;
        invalid(&value, "identifier: expected nonempty text <= 128 bytes");
    }
}

#[test]
fn profile_schema_and_validation_order_keep_original_error_classes() {
    let mut value = config();
    value.system_snapshot.profile_id = "profile-b".into();
    conflict(&value, "system snapshot profile mismatch");
    value.system_snapshot.schema_version = "unsupported".into();
    conflict(&value, "system snapshot profile mismatch");
    value.system_snapshot.profile_id = value.profile_id.clone();
    value.system_snapshot.host_id = "bad id".into();
    invalid(&value, "unsupported system snapshot schema");
}

#[test]
fn host_version_and_digest_keep_original_text_and_lowercase_boundaries() {
    for bad in [
        String::new(),
        " \n".into(),
        "bad\0version".into(),
        "a".repeat(129),
        "é".repeat(65),
    ] {
        let mut value = config();
        value.system_snapshot.host_version = bad;
        invalid(&value, "host_version: expected nonempty text <= 128 bytes");
    }
    for bad in [
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
    ] {
        let mut value = config();
        value.system_snapshot.mandatory_context_digest = bad;
        invalid(
            &value,
            "mandatory_context_digest: expected lowercase sha256 digest",
        );
    }
}

#[test]
fn tool_count_uniqueness_and_capability_subset_keep_original_refusals() {
    let mut value = config();
    value.system_snapshot.tools = (0..65).map(|i| format!("tool-{i}")).collect();
    invalid(&value, "too many host tools");
    value.system_snapshot.tools = vec!["bad id".into(), "bad id".into()];
    conflict(&value, "duplicate host tool");
    let mut value = config();
    value.host_capabilities.granted.insert("unavailable".into());
    conflict(&value, "granted capabilities must be available");
}

#[test]
fn all_six_fields_and_nested_required_fields_remain_required() {
    for field in [
        "profile_id",
        "system_snapshot",
        "host_surface_id",
        "host_capabilities",
        "evolution_enabled",
        "capability_level",
    ] {
        let mut value = serde_json::to_value(config()).unwrap();
        value.as_object_mut().unwrap().remove(field);
        let error = serde_json::from_value::<HostPrepareConfig>(value).unwrap_err();
        assert_eq!(error.to_string(), format!("missing field `{field}`"));
    }
    for (object, fields) in [
        (
            "system_snapshot",
            &[
                "schema_version",
                "profile_id",
                "host_id",
                "host_version",
                "model_id",
                "tools",
                "mandatory_context_digest",
            ][..],
        ),
        ("host_capabilities", &["available", "granted"][..]),
    ] {
        for field in fields {
            let mut value = serde_json::to_value(config()).unwrap();
            value[object].as_object_mut().unwrap().remove(*field);
            let error = serde_json::from_value::<HostPrepareConfig>(value).unwrap_err();
            assert_eq!(error.to_string(), format!("missing field `{field}`"));
        }
    }
}

#[test]
fn unknown_fields_and_invalid_scalar_shapes_are_still_rejected() {
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
    for (field, replacement) in [
        ("evolution_enabled", Value::Null),
        ("evolution_enabled", json!("true")),
        ("capability_level", json!("admin")),
        ("capability_level", Value::Null),
    ] {
        let mut value = serde_json::to_value(config()).unwrap();
        value[field] = replacement;
        assert!(serde_json::from_value::<HostPrepareConfig>(value).is_err());
    }
}

#[test]
fn nested_sets_keep_deduplication_and_order_while_tools_remain_a_vector() {
    let mut value = serde_json::to_value(config()).unwrap();
    value["host_capabilities"]["available"] = json!(["tool-z", "tool-a", "tool-m", "tool-z"]);
    value["host_capabilities"]["granted"] = json!(["tool-z", "tool-a", "tool-a"]);
    let value: HostPrepareConfig = serde_json::from_value(value).unwrap();
    value.validate().unwrap();
    assert_eq!(serde_json::to_vec(&value).unwrap(), CONFIG_BYTES.as_bytes());
    assert_eq!(value.system_snapshot.tools, ["tool-z", "tool-a"]);
}
