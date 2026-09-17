use serde_json::Value;
use std::path::PathBuf;

#[test]
fn support_scope_indexes_v33_and_declares_subset() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reports/support-scope.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(v["plan_version"], "v3.3");
    assert_eq!(
        v["plan_sha256"],
        "1b5870341be7c027c369a08c6e6bfd8f06dc494fc7174fff8930f42ae247294a"
    );
    assert_eq!(v["declaration"], "subset_only");
    assert_eq!(v["not_full_route_complete"], true);
    assert_eq!(v["legacy_equivalence_unverified"], true);
    assert_eq!(v["dimensions"]["effect"], "not_claimed");
}
