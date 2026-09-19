//! Support scope verification suite for E16.6.
//! Covers V071, V072, V074, V080, V098.
//!
//! CP-001: the previous version of this suite only checked file existence
//! for the E16.1-E16.6 sub-scopes and only checked traceability array
//! *lengths*, which let twelve fabricated `impl_files`/`test_files` paths
//! (E00, E03, E05, E06, E07, E08, E11, E13) and a length-only id check ship
//! undetected. This version validates every declared `e_scope` (not just
//! E16.x), validates exact id *sets* (not just counts), and adds synthetic
//! negative fixtures proving the validators actually reject fabricated
//! entries rather than merely happening to accept the current manifest.

use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_manifest(root: &Path) -> Value {
    let path = root.join("reports/support-scope.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("support-scope.json must exist at {path:?}: {e}"));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("support-scope.json must be valid JSON: {e}"))
}

/// Validates that a `traceability` array of bare-string ids is exactly the
/// contiguous set `{prefix}<width-padded 1..=count>`, with no duplicates,
/// gaps, or unexpected extras. A naive `.len() == count` check cannot
/// distinguish this from an array that repeats one id in place of another.
fn validate_id_set(
    field: &str,
    arr: &[Value],
    prefix: &str,
    count: usize,
    width: usize,
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for item in arr {
        let id = item
            .as_str()
            .ok_or_else(|| format!("{field}: entry {item} is not a string"))?;
        if !seen.insert(id.to_string()) {
            return Err(format!("{field}: duplicate id {id}"));
        }
    }
    let expected: BTreeSet<String> = (1..=count)
        .map(|i| format!("{prefix}{i:0width$}"))
        .collect();
    if seen != expected {
        let missing: Vec<_> = expected.difference(&seen).cloned().collect();
        let extra: Vec<_> = seen.difference(&expected).cloned().collect();
        return Err(format!(
            "{field}: id set mismatch, missing={missing:?} extra={extra:?}"
        ));
    }
    Ok(())
}

/// Validates the `so_sources` array: exact SO01..=SO18 id set (object
/// form), no duplicates, and required provenance fields (pinned commit,
/// blob sha, at least one mapped E task) on every entry.
fn validate_so_sources(arr: &[Value]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for so in arr {
        let id = so["id"]
            .as_str()
            .ok_or("so_sources entry missing string id")?;
        if !seen.insert(id.to_string()) {
            return Err(format!("so_sources: duplicate id {id}"));
        }
        let commit = so["commit"]
            .as_str()
            .ok_or_else(|| format!("{id}: missing commit"))?;
        if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "{id}: commit must be a 40-char hex sha, got {commit:?}"
            ));
        }
        let blob_sha = so["blob_sha"]
            .as_str()
            .ok_or_else(|| format!("{id}: missing blob_sha"))?;
        if blob_sha.len() != 40 || !blob_sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "{id}: blob_sha must be a 40-char hex sha, got {blob_sha:?}"
            ));
        }
        let e_tasks = so["e_tasks"]
            .as_array()
            .ok_or_else(|| format!("{id}: e_tasks must be an array"))?;
        if e_tasks.is_empty() {
            return Err(format!("{id}: e_tasks must not be empty"));
        }
    }
    let expected: BTreeSet<String> = (1..=18).map(|i| format!("SO{i:02}")).collect();
    if seen != expected {
        return Err(format!("so_sources: id set mismatch, got {seen:?}"));
    }
    Ok(())
}

/// Parses a `cargo test ... -p <crate> (--test <name> | --lib <module>::tests)`
/// command and returns the concrete repo-relative file it must exercise.
fn resolve_test_command_target(cmd: &str) -> Result<PathBuf, String> {
    if !cmd.trim_start().starts_with("cargo test") {
        return Err(format!(
            "test_command must start with 'cargo test', got {cmd:?}"
        ));
    }
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let crate_name = tokens
        .windows(2)
        .find(|w| w[0] == "-p")
        .map(|w| w[1])
        .ok_or_else(|| format!("test_command missing -p <crate>: {cmd:?}"))?;

    if let Some(w) = tokens.windows(2).find(|w| w[0] == "--test") {
        return Ok(PathBuf::from(format!(
            "crates/{crate_name}/tests/{}.rs",
            w[1]
        )));
    }
    if let Some(w) = tokens.windows(2).find(|w| w[0] == "--lib") {
        let module = w[1].strip_suffix("::tests").ok_or_else(|| {
            format!(
                "--lib filter must target a `::tests` module, got {:?}",
                w[1]
            )
        })?;
        let module_path = module.replace("::", "/");
        return Ok(PathBuf::from(format!(
            "crates/{crate_name}/src/{module_path}.rs"
        )));
    }
    Err(format!(
        "test_command must use --test <name> or --lib <module>::tests: {cmd:?}"
    ))
}

/// Validates a single `e_scopes` entry: non-empty impl/test evidence (except
/// for `status == "planned"`, which may legitimately have none yet), real
/// on-disk files, and a test_command whose derived target is both listed in
/// test_files and present on disk.
fn validate_e_scope_entry(name: &str, entry: &Value, root: &Path) -> Result<(), String> {
    let status = entry["status"]
        .as_str()
        .ok_or_else(|| format!("{name}: missing status"))?;

    let impl_files = entry["impl_files"]
        .as_array()
        .ok_or_else(|| format!("{name}: impl_files must be an array"))?;
    if impl_files.is_empty() {
        return Err(format!("{name}: impl_files must not be empty"));
    }
    for f in impl_files {
        let rel = f
            .as_str()
            .ok_or_else(|| format!("{name}: impl_files entry not a string"))?;
        if !root.join(rel).is_file() {
            return Err(format!("{name}: impl file {rel} does not exist on disk"));
        }
    }

    let test_files = entry["test_files"]
        .as_array()
        .ok_or_else(|| format!("{name}: test_files must be an array"))?;
    let test_command = entry["test_command"].as_str().unwrap_or("");

    if status == "planned" {
        // Planned work may legitimately have no test evidence yet, but any
        // test_files it does list must still be real.
        for f in test_files {
            let rel = f
                .as_str()
                .ok_or_else(|| format!("{name}: test_files entry not a string"))?;
            if !root.join(rel).is_file() {
                return Err(format!("{name}: test file {rel} does not exist on disk"));
            }
        }
        return Ok(());
    }

    if test_files.is_empty() {
        return Err(format!(
            "{name}: status {status:?} requires non-empty test_files"
        ));
    }
    if test_command.is_empty() {
        return Err(format!(
            "{name}: status {status:?} requires a non-empty test_command"
        ));
    }

    let mut test_file_strs = Vec::with_capacity(test_files.len());
    for f in test_files {
        let rel = f
            .as_str()
            .ok_or_else(|| format!("{name}: test_files entry not a string"))?;
        if !root.join(rel).is_file() {
            return Err(format!("{name}: test file {rel} does not exist on disk"));
        }
        test_file_strs.push(rel.to_string());
    }

    let target = resolve_test_command_target(test_command).map_err(|e| format!("{name}: {e}"))?;
    let target_str = target.to_string_lossy().replace('\\', "/");
    if !test_file_strs.iter().any(|t| t == &target_str) {
        return Err(format!(
            "{name}: test_command target {target_str:?} is not one of test_files {test_file_strs:?}"
        ));
    }
    if !root.join(&target).is_file() {
        return Err(format!(
            "{name}: test_command target {target_str} does not exist on disk"
        ));
    }

    Ok(())
}

#[test]
fn support_scope_indexes_v41_and_declares_subset() {
    let root = repo_root();
    let v = load_manifest(&root);

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

    // V080 & V098: Full traceability commitments verified as exact id sets,
    // not just array length (a list that repeats one id in place of a
    // missing one would previously still pass a bare `.len()` check).
    validate_id_set(
        "b_commitments",
        v["traceability"]["b_commitments"]
            .as_array()
            .expect("b_commitments array"),
        "B",
        10,
        2,
    )
    .expect("B01-B10 commitments must be an exact, duplicate-free set");
    validate_id_set(
        "u_commitments",
        v["traceability"]["u_commitments"]
            .as_array()
            .expect("u_commitments array"),
        "U",
        8,
        2,
    )
    .expect("U01-U08 commitments must be an exact, duplicate-free set");
    validate_id_set(
        "k_commitments",
        v["traceability"]["k_commitments"]
            .as_array()
            .expect("k_commitments array"),
        "K",
        10,
        2,
    )
    .expect("K01-K10 commitments must be an exact, duplicate-free set");
    validate_id_set(
        "v_scenarios",
        v["traceability"]["v_scenarios"]
            .as_array()
            .expect("v_scenarios array"),
        "V",
        98,
        3,
    )
    .expect("V001-V098 scenarios must be an exact, duplicate-free set");
    validate_so_sources(
        v["traceability"]["so_sources"]
            .as_array()
            .expect("so_sources array"),
    )
    .expect("SO01-SO18 sources must be an exact set with valid provenance fields");
}

// =========================================================================
// F09 Adversarial Regression (from controller review)
// =========================================================================
#[test]
fn test_review_support_scope_concrete_mapping() {
    let root = repo_root();
    let v = load_manifest(&root);

    let e_scopes = v["e_scopes"]
        .as_object()
        .expect("e_scopes must be an object");

    // CP-001: every declared e_scope (not just E16.x) must resolve to real,
    // on-disk impl/test files with a reproducible, matching test_command.
    // Twelve entries (E00, E03, E05, E06, E07, E08, E11, E13) previously
    // pointed at files that never existed; this loop now covers all of
    // E00-E18 so a future fabricated path cannot slip back in unnoticed.
    for (name, entry) in e_scopes {
        validate_e_scope_entry(name, entry, &root)
            .unwrap_or_else(|e| panic!("e_scope validation failed: {e}"));
    }

    // Must map all sub-scopes of E16 specifically
    let e16_subscopes = ["E16.1", "E16.2", "E16.3", "E16.4", "E16.5", "E16.6"];
    for sub in e16_subscopes {
        assert!(e_scopes.contains_key(sub), "missing e_scope for {sub}");
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

    // Verify SO sources concrete mappings (frozen commit pin unchanged
    // without fresh re-verification evidence).
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

// =========================================================================
// CP-001 negative fixtures: synthetic manifests proving the validators
// reject fabricated/incomplete entries, not merely pass the (now-corrected)
// real manifest. None of these mutate any file in the repository.
// =========================================================================

#[test]
fn rejects_impl_file_that_does_not_exist_on_disk() {
    let root = repo_root();
    let entry = json!({
        "status": "verified_fabricated",
        "impl_files": ["crates/evo-core/src/this_file_does_not_exist.rs"],
        "test_files": ["crates/evo-core/tests/support_scope.rs"],
        "test_command": "cargo test --locked --offline -p evo-core --test support_scope"
    });
    let err = validate_e_scope_entry("FAKE", &entry, &root)
        .expect_err("a nonexistent impl_files path must be rejected");
    assert!(
        err.contains("does not exist on disk"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_duplicate_id_in_a_commitment_set() {
    // B01 repeated, B10 absent -- array length still equals 10.
    let arr: Vec<Value> = vec![
        json!("B01"),
        json!("B01"),
        json!("B02"),
        json!("B03"),
        json!("B04"),
        json!("B05"),
        json!("B06"),
        json!("B07"),
        json!("B08"),
        json!("B09"),
    ];
    assert_eq!(
        arr.len(),
        10,
        "fixture must keep the expected count to prove length alone is insufficient"
    );
    let err = validate_id_set("b_commitments", &arr, "B", 10, 2)
        .expect_err("a duplicated id must be rejected even though array length matches");
    assert!(err.contains("duplicate id B01"), "unexpected error: {err}");
}

#[test]
fn rejects_commitment_set_with_correct_length_but_wrong_ids() {
    // B10 replaced by an out-of-range B11: length is still 10, there are no
    // duplicates, yet the id set is not the required B01..B10. This is the
    // exact class of bug a bare `.len() == 10` check cannot catch.
    let arr: Vec<Value> = vec![
        json!("B01"),
        json!("B02"),
        json!("B03"),
        json!("B04"),
        json!("B05"),
        json!("B06"),
        json!("B07"),
        json!("B08"),
        json!("B09"),
        json!("B11"),
    ];
    let err = validate_id_set("b_commitments", &arr, "B", 10, 2)
        .expect_err("an id set with the right length but wrong members must be rejected");
    assert!(
        err.contains("missing=") && err.contains("extra="),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_so_sources_with_duplicate_id() {
    let mut arr: Vec<Value> = (1..=18)
        .map(|i| {
            json!({
                "id": format!("SO{i:02}"),
                "commit": "79124b37e9a6371e13b753f8bcd7adb1e493ade1",
                "blob_sha": "520a90eb8af422368c8e498b8487504bb8f6c279",
                "e_tasks": ["E03"]
            })
        })
        .collect();
    // Overwrite SO18 with a duplicate of SO01, leaving SO18 absent.
    arr[17] = arr[0].clone();
    let err = validate_so_sources(&arr).expect_err("a duplicated SO id must be rejected");
    assert!(err.contains("duplicate id SO01"), "unexpected error: {err}");
}

#[test]
fn rejects_so_source_with_malformed_commit_hash() {
    let arr = vec![json!({
        "id": "SO01",
        "commit": "not-a-real-sha",
        "blob_sha": "520a90eb8af422368c8e498b8487504bb8f6c279",
        "e_tasks": ["E03"]
    })];
    let err = validate_so_sources(&arr).expect_err("a malformed commit hash must be rejected");
    assert!(err.contains("40-char hex"), "unexpected error: {err}");
}

#[test]
fn rejects_test_command_pointing_at_a_fake_target() {
    let root = repo_root();
    // Reproduces the historical E00 fake: an integration test target that
    // was never a real file (evidence.rs's tests are embedded unit tests,
    // not a `tests/evidence.rs` integration file).
    let entry = json!({
        "status": "verified_fabricated",
        "impl_files": ["crates/evo-core/src/evidence.rs"],
        "test_files": ["crates/evo-core/tests/evidence.rs"],
        "test_command": "cargo test --locked --offline -p evo-core --test evidence"
    });
    let err = validate_e_scope_entry("FAKE", &entry, &root)
        .expect_err("a test_command referencing a fabricated integration target must be rejected");
    assert!(
        err.contains("does not exist on disk"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_test_command_target_not_listed_in_test_files() {
    let root = repo_root();
    // The command's derived target (crates/evo-core/src/evidence.rs) is a
    // real file, but it is not declared in test_files -- an inconsistency
    // between the two fields that must still be rejected.
    let entry = json!({
        "status": "verified_fabricated",
        "impl_files": ["crates/evo-core/src/evidence.rs"],
        "test_files": ["crates/evo-core/tests/support_scope.rs"],
        "test_command": "cargo test --locked --offline -p evo-core --lib evidence::tests"
    });
    let err = validate_e_scope_entry("FAKE", &entry, &root).expect_err(
        "a test_command target absent from test_files must be rejected even if the file exists",
    );
    assert!(
        err.contains("is not one of test_files"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_verified_status_with_no_test_evidence() {
    let root = repo_root();
    let entry = json!({
        "status": "verified_fabricated",
        "impl_files": ["crates/evo-core/src/evidence.rs"],
        "test_files": [],
        "test_command": ""
    });
    let err = validate_e_scope_entry("FAKE", &entry, &root)
        .expect_err("a verified_* status with no test evidence must be rejected");
    assert!(
        err.contains("requires non-empty test_files"),
        "unexpected error: {err}"
    );
}

#[test]
fn planned_status_tolerates_empty_test_evidence() {
    let root = repo_root();
    let entry = json!({
        "status": "planned",
        "impl_files": ["crates/evo-engine/src/meta.rs"],
        "test_files": [],
        "test_command": ""
    });
    validate_e_scope_entry("FAKE", &entry, &root)
        .expect("status == \"planned\" must tolerate empty test evidence");
}
