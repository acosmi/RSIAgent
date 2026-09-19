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
