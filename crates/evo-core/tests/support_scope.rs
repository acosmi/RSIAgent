//! Support scope verification suite for E16.6.
//! Covers V071, V072, V074, V080, V098.
use serde_json::Value;
use std::path::PathBuf;

#[test]
fn support_scope_indexes_v41_and_declares_subset() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reports/support-scope.json");
    let content = std::fs::read_to_string(path).expect("support-scope.json must exist");
    let v: Value = serde_json::from_str(&content).expect("support-scope.json must be valid JSON");

    // V071: Must strictly index v4.1 and its frozen SHA-256
    assert_eq!(v["plan_version"], "v4.1");
    assert_eq!(
        v["plan_sha256"],
        "45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150"
    );

    // V074: Explicit subset_only, not_full_route_complete
    assert_eq!(v["declaration"], "subset_only");
    assert_eq!(v["not_full_route_complete"], true);
    assert_eq!(v["legacy_equivalence_unverified"], true);
    assert_eq!(v["dimensions"]["effect"], "not_claimed");

    // V080 & V098: Full traceability commitments verified
    let b_list = v["traceability"]["b_commitments"]
        .as_array()
        .expect("B commitments must be an array");
    assert_eq!(b_list.len(), 10, "B01-B10 must have 10 commitments");

    let u_list = v["traceability"]["u_commitments"]
        .as_array()
        .expect("U commitments must be an array");
    assert_eq!(u_list.len(), 8, "U01-U08 must have 8 commitments");

    let k_list = v["traceability"]["k_commitments"]
        .as_array()
        .expect("K commitments must be an array");
    assert_eq!(k_list.len(), 10, "K01-K10 must have 10 commitments");

    let so_list = v["traceability"]["so_sources"]
        .as_array()
        .expect("SO sources must be an array");
    assert_eq!(so_list.len(), 18, "SO01-SO18 must have 18 sources");

    let v_list = v["traceability"]["v_scenarios"]
        .as_array()
        .expect("V scenarios must be an array");
    assert_eq!(
        v_list.len(),
        98,
        "V001-V098 must have 98 validation scenarios"
    );
}

// =========================================================================
// F09 Adversarial Regression (from controller review)
// =========================================================================
#[test]
fn test_review_support_scope_concrete_mapping() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("reports/support-scope.json");
    let content = std::fs::read_to_string(&path).expect("support-scope.json must exist");
    let v: Value = serde_json::from_str(&content).expect("support-scope.json must be valid JSON");

    let e_scopes = v["e_scopes"]
        .as_object()
        .expect("e_scopes must be an object");

    // Must map all sub-scopes of E16 specifically
    let e16_subscopes = ["E16.1", "E16.2", "E16.3", "E16.4", "E16.5", "E16.6"];
    for sub in e16_subscopes {
        assert!(e_scopes.contains_key(sub), "missing e_scope for {sub}");
        let entry = &e_scopes[sub];
        let impl_files = entry["impl_files"].as_array().expect("impl_files array");
        assert!(
            !impl_files.is_empty(),
            "{sub} must have non-empty impl_files"
        );
        for f in impl_files {
            let rel = f.as_str().expect("file path must be string");
            assert!(
                root.join(rel).exists(),
                "impl file {rel} for {sub} does not exist on disk"
            );
        }

        let test_files = entry["test_files"].as_array().expect("test_files array");
        assert!(
            !test_files.is_empty(),
            "{sub} must have non-empty test_files"
        );
        for f in test_files {
            let rel = f.as_str().expect("file path must be string");
            assert!(
                root.join(rel).exists(),
                "test file {rel} for {sub} does not exist on disk"
            );
        }

        let cmd = entry["test_command"].as_str().expect("test_command string");
        assert!(
            cmd.starts_with("cargo test"),
            "{sub} test_command must start with cargo test"
        );
    }

    // Concrete assertions on specific implementation anchors
    assert_eq!(
        e_scopes["E16.1"]["impl_files"][0],
        "crates/evo-engine/src/import.rs"
    );
    assert_eq!(
        e_scopes["E16.2"]["impl_files"][0],
        "crates/evo-engine/src/packages.rs"
    );
    assert_eq!(
        e_scopes["E16.3"]["impl_files"][0],
        "crates/evo-engine/src/seeds.rs"
    );
    assert_eq!(
        e_scopes["E16.4"]["impl_files"][0],
        "crates/evo-engine/src/hosts.rs"
    );
    assert_eq!(
        e_scopes["E16.5"]["impl_files"][0],
        "crates/evo-engine/src/capacity.rs"
    );
    assert_eq!(
        e_scopes["E16.6"]["impl_files"][0],
        "reports/support-scope.json"
    );

    // Verify SO sources concrete mappings
    let so_list = v["traceability"]["so_sources"]
        .as_array()
        .expect("so_sources array");
    assert_eq!(so_list.len(), 18);
    for so in so_list {
        let commit = so["commit"].as_str().expect("so commit");
        assert_eq!(
            commit, "79124b37e9a6371e13b753f8bcd7adb1e493ade1",
            "SO commit must be pinned to frozen commit"
        );
        let blob_sha = so["blob_sha"].as_str().expect("blob sha");
        assert_eq!(blob_sha.len(), 40, "blob sha must be 40-char hex string");
        let e_tasks = so["e_tasks"].as_array().expect("e_tasks array");
        assert!(
            !e_tasks.is_empty(),
            "SO source must map to at least one E task"
        );
    }
}
