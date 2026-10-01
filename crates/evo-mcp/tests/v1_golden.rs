//! Freeze the actual router descriptors by name; router enumeration order is not promised.
use evo_core::contract::V1_TOOL_DESCRIPTOR_MAX;
use evo_mcp::tool_descriptors;
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::PathBuf};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/contracts/v1")
        .join(format!("tool_{name}.json"));
    fs::read(&path)
        .unwrap_or_else(|error| panic!("missing frozen v1 golden {}: {error}", path.display()))
}

fn check_descriptor(value: &Value, golden: &[u8]) -> Result<(), &'static str> {
    let bytes = serde_json::to_vec(value).unwrap();
    if bytes.len() > V1_TOOL_DESCRIPTOR_MAX {
        return Err("descriptor exceeds 2000 bytes");
    }
    if bytes != golden {
        return Err("descriptor differs from frozen golden");
    }
    Ok(())
}

#[test]
fn four_real_router_descriptors_match_frozen_bytes_and_budget() {
    assert_eq!(V1_TOOL_DESCRIPTOR_MAX, 2000);
    let tools = tool_descriptors().unwrap();
    assert_eq!(tools.len(), 4);
    let mut by_name = BTreeMap::new();
    for tool in tools {
        let name = tool["name"].as_str().unwrap().to_owned();
        assert!(by_name.insert(name, tool).is_none(), "duplicate tool name");
    }
    assert_eq!(
        by_name.keys().map(String::as_str).collect::<Vec<_>>(),
        ["evo_feedback", "evo_inspect", "evo_prepare", "evo_propose"]
    );
    for (name, descriptor) in by_name {
        let golden = fixture(&name);
        check_descriptor(&descriptor, &golden).unwrap();
        let round_trip: Value = serde_json::from_slice(&golden).unwrap();
        assert_eq!(serde_json::to_vec(&round_trip).unwrap(), golden);
    }
}

#[test]
fn descriptor_guard_detects_small_description_drift_and_over_budget_bytes() {
    let descriptor = tool_descriptors()
        .unwrap()
        .into_iter()
        .find(|tool| tool["name"] == "evo_prepare")
        .unwrap();
    let golden = fixture("evo_prepare");
    check_descriptor(&descriptor, &golden).unwrap();
    let mut changed = descriptor.clone();
    changed["description"] = Value::String("Changed wording".into());
    assert!(serde_json::to_vec(&changed).unwrap().len() <= 2000);
    assert_eq!(
        check_descriptor(&changed, &golden),
        Err("descriptor differs from frozen golden")
    );
    changed = descriptor;
    changed["description"] = Value::String(String::new());
    let overhead = serde_json::to_vec(&changed).unwrap().len();
    changed["description"] = Value::String("x".repeat(2000 - overhead));
    let exact = serde_json::to_vec(&changed).unwrap();
    assert_eq!(exact.len(), 2000);
    check_descriptor(&changed, &exact).unwrap();
    changed["description"] =
        Value::String(format!("{}x", changed["description"].as_str().unwrap()));
    assert_eq!(serde_json::to_vec(&changed).unwrap().len(), 2001);
    assert_eq!(
        check_descriptor(&changed, &golden),
        Err("descriptor exceeds 2000 bytes")
    );
}
