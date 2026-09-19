//! Strict derived support-scope verification for E16.6 (V071/V072/V074/V080/V098).
//! The v4.1 plan remains normative; this test rejects incomplete or inflated indexes.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

const PLAN_SHA: &str = "45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150";
const SKILLOPT_REPO: &str = "https://github.com/microsoft/SkillOpt.git";
const SKILLOPT_COMMIT: &str = "79124b37e9a6371e13b753f8bcd7adb1e493ade1";
const ALLOWED_STATUSES: &[&str] = &[
    "planned",
    "in_progress",
    "blocked",
    "implemented_not_verified",
    "verified",
    "explicitly_out_of_scope",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_manifest(root: &Path) -> Value {
    let path = root.join("reports/support-scope.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("support-scope.json must exist at {path:?}: {error}"));
    serde_json::from_str(&content)
        .unwrap_or_else(|error| panic!("support-scope.json must be valid JSON: {error}"))
}

fn expected_e_scenarios() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    [
        ("E00", "V001 V071 V072 V073 V080 V098"),
        ("E01", "V010 V011 V012 V013 V084 V085 V096 V097"),
        ("E02", "V002 V003 V005 V009 V043 V044 V045 V046 V047 V048 V060 V062 V078 V079 V081 V082 V083 V084 V085 V086 V087 V088 V089 V090 V091 V092 V093 V094 V095 V096"),
        ("E03", "V004 V005 V006 V017 V051 V052 V054 V055 V056 V057 V058 V087 V088 V089 V090 V094 V096"),
        ("E04", "V007 V008 V009 V038 V083 V085 V086 V087 V093 V094 V096"),
        ("E05", "V010 V011 V012 V013 V028 V084 V085 V090 V091 V094 V097"),
        ("E06", "V014 V015 V016 V017 V047 V049 V050 V059 V064 V070 V075 V077 V078 V081 V084 V085 V089 V091 V094 V095 V096"),
        ("E07", "V003 V010 V014 V015 V016 V042 V047 V059 V070 V077 V087 V088 V089 V091 V096 V097"),
        ("E08", "V017 V018 V038 V056 V069 V075 V081 V082 V083 V084 V085 V086 V090 V092 V094 V096"),
        ("E09", "V019 V020 V022 V024 V025 V081 V086 V088 V089 V090 V091 V093 V094 V095 V096"),
        ("E10", "V020 V021 V022 V023 V025 V026 V027 V081 V086 V094 V096"),
        ("E11", "V022 V026 V027 V028 V042 V081 V094 V096 V097"),
        ("E12", "V029 V030 V031 V034 V082 V083 V093 V094 V096"),
        ("E13", "V016 V032 V037 V038 V082 V084 V087 V090 V091 V094 V096 V097"),
        ("E14", "V015 V033 V034 V035 V036 V086 V090 V092 V094 V096 V097"),
        ("E15", "V026 V033 V035 V036 V042 V085 V092 V096 V097"),
        ("E16", "V081 V082 V083 V084 V085 V086 V087 V088 V089 V090 V091 V092 V093 V094 V095 V096 V097 V098"),
        ("E16.1", "V005 V006 V017 V051 V052 V053 V054 V055 V056 V076 V087 V090 V098"),
        ("E16.2", "V017 V039 V066 V067 V068 V069 V070 V075 V091 V092 V098"),
        ("E16.3", "V014 V018 V038 V063 V064 V065 V078 V091 V092 V098"),
        ("E16.4", "V002 V003 V009 V037 V039 V060 V061 V062 V077 V087 V091 V098"),
        ("E16.5", "V007 V008 V017 V018 V037 V038 V039 V066 V069 V073 V075 V081 V082 V083 V084 V085 V086 V089 V090 V092 V094 V096 V098"),
        ("E16.6", "V001 V039 V042 V071 V072 V073 V074 V080 V098"),
        ("E17", "V040"),
        ("E18", "V041"),
    ]
    .into_iter()
    .map(|(task, scenarios)| (task, scenarios.split_whitespace().collect()))
    .collect()
}

fn expected_e_ids() -> BTreeSet<String> {
    (0..=18)
        .map(|index| format!("E{index:02}"))
        .chain((1..=6).map(|index| format!("E16.{index}")))
        .collect()
}

fn string_set(value: &Value, field: &str) -> Result<BTreeSet<String>, String> {
    let array = value
        .as_array()
        .ok_or_else(|| format!("{field} must be an array"))?;
    let mut result = BTreeSet::new();
    for item in array {
        let text = item
            .as_str()
            .ok_or_else(|| format!("{field} contains a non-string entry"))?;
        if !result.insert(text.to_string()) {
            return Err(format!("{field} contains duplicate {text}"));
        }
    }
    Ok(result)
}

fn nonempty_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| format!("{field} must be a non-empty string"))
}

fn lower_hex(value: &Value, len: usize, field: &str) -> Result<(), String> {
    let text = nonempty_string(value, field)?;
    if text.len() != len
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "{field} must be {len} lowercase hexadecimal characters"
        ));
    }
    Ok(())
}

fn safe_relative(path: &str, field: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if path.is_absolute() || path.components().next().is_none() {
        return Err(format!("{field} must be a non-empty relative path"));
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "{field} contains traversal or non-normal components"
        ));
    }
    Ok(path.to_path_buf())
}

fn validate_repo_file(root: &Path, relative: &str, field: &str) -> Result<(), String> {
    let relative = safe_relative(relative, field)?;
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(format!("{field} contains a non-normal component"));
        };
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| format!("{field} does not exist: {}", relative.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "{field} contains a symlink component: {}",
                relative.display()
            ));
        }
    }
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| format!("{field} does not exist: {}", relative.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "{field} is not a regular file: {}",
            relative.display()
        ));
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize repo root: {error}"))?;
    let canonical_path = path
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize {field}: {error}"))?;
    if !canonical_path.starts_with(canonical_root) {
        return Err(format!("{field} escapes the repository"));
    }
    Ok(())
}

fn valid_ident(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn package_is_declared(root: &Path, package: &str) -> Result<(), String> {
    if !valid_ident(package) {
        return Err("cargo package name is invalid".into());
    }
    let manifest_path = root.join("crates").join(package).join("Cargo.toml");
    let body = std::fs::read_to_string(&manifest_path)
        .map_err(|_| format!("cargo package {package} is not a declared crate"))?;
    let compact: String = body
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    if !compact.contains(&format!("name=\"{package}\"")) {
        return Err(format!("Cargo.toml does not declare package {package}"));
    }
    Ok(())
}

fn resolve_test_command(root: &Path, command: &str) -> Result<PathBuf, String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.len() != 8
        || tokens[0] != "cargo"
        || tokens[1] != "test"
        || tokens[2] != "--locked"
        || tokens[3] != "--offline"
        || tokens[4] != "-p"
    {
        return Err("command must use the finite cargo test grammar".into());
    }
    let package = tokens[5];
    package_is_declared(root, package)?;
    match tokens[6] {
        "--test" => {
            let target = tokens[7];
            if !valid_ident(target) {
                return Err("integration test target is invalid".into());
            }
            let path = PathBuf::from(format!("crates/{package}/tests/{target}.rs"));
            validate_repo_file(root, path.to_str().unwrap(), "test command target")?;
            Ok(path)
        }
        "--lib" => {
            let filter = tokens[7];
            let module = filter
                .strip_suffix("::tests")
                .ok_or("--lib filter must end in ::tests")?;
            if module.is_empty() || !module.split("::").all(valid_ident) {
                return Err("--lib module filter is invalid".into());
            }
            let first = module.split("::").next().unwrap();
            let lib = root.join("crates").join(package).join("src/lib.rs");
            let lib_body = std::fs::read_to_string(&lib)
                .map_err(|_| format!("package {package} has no readable src/lib.rs"))?;
            if !lib_body.contains(&format!("mod {first};")) {
                return Err(format!("module {first} is not declared by {package}"));
            }
            let path = PathBuf::from(format!(
                "crates/{package}/src/{}.rs",
                module.replace("::", "/")
            ));
            validate_repo_file(root, path.to_str().unwrap(), "test command target")?;
            Ok(path)
        }
        _ => Err("command must contain exactly one --test or --lib target".into()),
    }
}

fn expected_merge_for_pr(pr: u64) -> Result<Option<&'static str>, String> {
    Ok(match pr {
        29 => Some("28f892c3d68591d201a8029345257f051fb123ef"),
        30 => Some("faec991036f03f27cf6d2ad3db86f841815fc828"),
        31 => Some("dfc861ce7572f8f9f49a8aaf9f0cc1687cba57f9"),
        32 => Some("9b13237af884c525652813ea508eb8c5f100d6f8"),
        33 => Some("c1cf6a2e91253218033b5883a11baa02c85a5d43"),
        34 => Some("63824fff2e8d0cd0767d31140d064972542f6b53"),
        35 => Some("c017e6a32da3cdf692c3321dd29d6510edada0c4"),
        36 => Some("794cf0f517a57e48166ef2ad44cb5762f3a61d5e"),
        37 => Some("b0662ca1b7a2fe27d4a4622918679eed4934e214"),
        38 => Some("7a8a35908a2f7cc72ed601cbc902a2e4b49120bf"),
        39 => Some("96e62412809610a72d2ab593aaca59a8bcebd6d2"),
        40 => Some("09b3ebe8bad12d59bd3c2f926d3050802372e96f"),
        41 => Some("b1127a228c4a40248631ed417aba0ad7ea1a0884"),
        42 => Some("2e209dcea36d63610c59d3797e039f35d063ec89"),
        43 => Some("59afd1663a0d5f5212077dd92beaaf7920cde0ff"),
        47 => None,
        _ => return Err(format!("no reviewed merge status for PR {pr}")),
    })
}

fn validate_evidence_record(record: &Value, task: &str, root: &Path) -> Result<String, String> {
    let id = nonempty_string(&record["id"], &format!("{task} evidence id"))?;
    nonempty_string(&record["description"], &format!("{id} description"))?;
    if record["plan_version"] != "v4.1" || record["plan_sha256"] != PLAN_SHA {
        return Err(format!("{id} does not bind the v4.1 plan"));
    }
    lower_hex(&record["source_sha"], 40, &format!("{id} source_sha"))?;
    let pr = nonempty_string(&record["pr"], &format!("{id} pr"))?;
    let prefix = "https://github.com/acosmi/RSIAgent/pull/";
    let pr_number = pr
        .strip_prefix(prefix)
        .filter(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| format!("{id} has an invalid pull request URL"))?
        .parse::<u64>()
        .map_err(|_| format!("{id} has an invalid pull request number"))?;
    let expected_merge = expected_merge_for_pr(pr_number)?;
    match expected_merge {
        Some(expected) => {
            lower_hex(&record["merged_sha"], 40, &format!("{id} merged_sha"))?;
            if record["merged_sha"] != expected {
                return Err(format!(
                    "{id} merged_sha differs from the reviewed PR merge"
                ));
            }
        }
        None if !record["merged_sha"].is_null() => {
            return Err(format!("{id} records a merge for an unmerged PR"));
        }
        None => {}
    }
    lower_hex(&record["input_digest"], 64, &format!("{id} input_digest"))?;
    if record["exit_code"].as_i64() != Some(0) {
        return Err(format!("{id} verified execution must have exit_code 0"));
    }
    for field in ["actual_result", "scope", "risk", "rollback"] {
        nonempty_string(&record[field], &format!("{id} {field}"))?;
    }
    let log = nonempty_string(&record["log_path"], &format!("{id} log_path"))?;
    safe_relative(log, &format!("{id} log_path"))?;
    let input_ref = nonempty_string(&record["input_ref"], &format!("{id} input_ref"))?;
    safe_relative(input_ref, &format!("{id} input_ref"))?;
    let source = nonempty_string(&record["record_source"], &format!("{id} record_source"))?;
    safe_relative(source, &format!("{id} record_source"))?;
    let entry = nonempty_string(&record["test_entry"], &format!("{id} test_entry"))?;
    validate_repo_file(root, entry, &format!("{id} test_entry"))?;
    let command = nonempty_string(&record["command"], &format!("{id} command"))?;
    let target = resolve_test_command(root, command)?;
    if target.as_path() != Path::new(entry) {
        return Err(format!(
            "{id} command target {} differs from test_entry {entry}",
            target.display()
        ));
    }
    Ok(id.to_string())
}

fn validate_e_scopes(value: &Value, root: &Path) -> Result<BTreeSet<String>, String> {
    let object = value.as_object().ok_or("e_scopes must be an object")?;
    let actual: BTreeSet<String> = object.keys().cloned().collect();
    let expected = expected_e_ids();
    if actual != expected {
        return Err(format!(
            "e_scopes exact set mismatch: missing={:?} extra={:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        ));
    }
    let expected_scenarios = expected_e_scenarios();
    let mut evidence_ids = BTreeSet::new();
    for (task, entry) in object {
        nonempty_string(&entry["title"], &format!("{task} title"))?;
        let status = nonempty_string(&entry["status"], &format!("{task} status"))?;
        if !ALLOWED_STATUSES.contains(&status) {
            return Err(format!("{task} has unknown status {status}"));
        }
        nonempty_string(&entry["status_reason"], &format!("{task} status_reason"))?;
        let files = entry["implementation_files"]
            .as_array()
            .ok_or_else(|| format!("{task} implementation_files must be an array"))?;
        if status != "planned" && files.is_empty() {
            return Err(format!(
                "{task} non-planned scope needs implementation_files"
            ));
        }
        for file in files {
            let file = nonempty_string(file, &format!("{task} implementation file"))?;
            validate_repo_file(root, file, &format!("{task} implementation file"))?;
        }
        let actual_scenarios = string_set(&entry["scenarios"], &format!("{task} scenarios"))?;
        let expected_for_task: BTreeSet<String> = expected_scenarios[task.as_str()]
            .iter()
            .map(|value| value.to_string())
            .collect();
        if actual_scenarios != expected_for_task {
            return Err(format!("{task} scenario responsibility differs from v4.1"));
        }
        let remaining = entry["remaining"]
            .as_array()
            .ok_or_else(|| format!("{task} remaining must be an array"))?;
        if status == "verified" && !remaining.is_empty() {
            return Err(format!(
                "{task} cannot be verified while remaining is non-empty"
            ));
        }
        if status != "verified" && remaining.is_empty() {
            return Err(format!(
                "{task} non-verified scope must state remaining work"
            ));
        }
        for item in remaining {
            nonempty_string(item, &format!("{task} remaining item"))?;
        }
        let evidence = entry["verified_subscopes"]
            .as_array()
            .ok_or_else(|| format!("{task} verified_subscopes must be an array"))?;
        if status == "verified" && evidence.is_empty() {
            return Err(format!("{task} verified status lacks execution evidence"));
        }
        for record in evidence {
            let id = validate_evidence_record(record, task, root)?;
            if !evidence_ids.insert(id.clone()) {
                return Err(format!("duplicate evidence id {id}"));
            }
        }
    }
    Ok(evidence_ids)
}

struct CommitmentExpected {
    id: &'static str,
    sections: &'static str,
    sources: &'static str,
    e_tasks: &'static str,
    scenarios: &'static str,
}

fn expected_commitments(kind: &str) -> Vec<CommitmentExpected> {
    let rows: &[(&str, &str, &str, &str, &str)] = match kind {
        "B" => &[
            ("B01", "§5.2", "RH01 RH02", "E02 E06", "V044 V048 V079"),
            ("B02", "§5.3", "RH03", "E02", "V043 V045 V046 V078"),
            (
                "B03",
                "§5.4 §6.5",
                "RH04",
                "E02 E06 E07",
                "V047 V049 V050 V059 V077",
            ),
            ("B04", "§5.5", "RH05", "E02 E16.4", "V060 V061 V062"),
            ("B05", "§6.3", "RH06", "E03", "V051 V052 V055 V056"),
            ("B06", "§6.4", "RH07", "E03 E14", "V054 V057 V058 V070"),
            (
                "B07",
                "§5.6 §6.3",
                "RH08",
                "E03 E16.1",
                "V052 V053 V054 V056 V076",
            ),
            ("B08", "§11.1", "RH09", "E06 E16.3", "V063 V064 V065 V078"),
            (
                "B09",
                "§6.5 §9",
                "RH01 RH07",
                "E06 E14 E15",
                "V015 V033 V036 V070",
            ),
            (
                "B10",
                "§11.2–§11.3",
                "RH01",
                "E08 E16.2 E16.6",
                "V066 V067 V068 V069 V073 V075",
            ),
        ],
        "U" => &[
            ("U01", "§7.1 §7.5", "", "E02 E09 E10 E11", "V025 V028 V081"),
            ("U02", "§7.5.1–§7.5.3", "", "E10 E11", "V021 V023 V027 V081"),
            ("U03", "§8.1", "", "E12 E13", "V029 V034 V082"),
            ("U04", "§8.2–§8.4", "", "E04 E12", "V007 V030 V031 V083"),
            ("U05", "§3.2 §10", "", "E01 E05 E06", "V010 V011 V084"),
            ("U06", "§3.4.1 §6.6", "", "E01 E04 E05", "V012 V013 V085"),
            ("U07", "§7.1.1 §7.2.1", "", "E09", "V020 V024 V086"),
            (
                "U08",
                "§1.4 §12 §14.2",
                "",
                "E00 E05 E09 E10 E12 E16.6",
                "V042 V062 V080 V081 V082 V083 V084 V085 V086",
            ),
        ],
        "K" => &[
            (
                "K01",
                "§6.4.1 §5.8",
                "SO01 SO06",
                "E02 E03 E04 E07 E13",
                "V087",
            ),
            ("K02", "§6.7.1–§6.7.2", "SO02 SO03", "E03 E09", "V088"),
            ("K03", "§5.3.1 §5.8", "SO04 SO05", "E02 E03 E06 E09", "V089"),
            (
                "K04",
                "§6.7.3 §5.8 §11.5",
                "SO01 SO02",
                "E03 E05 E08 E09 E13",
                "V090 V094",
            ),
            (
                "K05",
                "§11.5 §10.2",
                "SO01 SO07 SO10",
                "E05 E06 E09 E13",
                "V091",
            ),
            ("K06", "§9.1 §3.1.1", "SO01 SO08", "E14 E15", "V092 V097"),
            (
                "K07",
                "§8.5 §7.4.1",
                "SO12 SO13",
                "E04 E09 E10 E12",
                "V093 V094",
            ),
            ("K08", "§7.7", "SO11 SO14", "E06 E09", "V095"),
            (
                "K09",
                "§3.1.1 §3.6 §6.7.4",
                "SO13 SO15 SO16",
                "E01 E04 E07 E11 E15",
                "V096 V097",
            ),
            (
                "K10",
                "§2.7–§2.8 §16.5–§16.6 §18.7",
                "SO09 SO10 SO18",
                "E00 E05 E06 E16",
                "V091 V098",
            ),
        ],
        _ => unreachable!(),
    };
    rows.iter()
        .map(|row| CommitmentExpected {
            id: row.0,
            sections: row.1,
            sources: row.2,
            e_tasks: row.3,
            scenarios: row.4,
        })
        .collect()
}

fn split_set(values: &str) -> BTreeSet<String> {
    values.split_whitespace().map(str::to_string).collect()
}

fn expected_commitment_implementation_status(id: &str) -> &'static str {
    match id {
        "B09" | "K06" => "planned",
        "B04" | "B06" | "B07" | "B10" | "U01" | "U02" | "U03" | "U04" | "U07" | "U08" | "K01"
        | "K05" | "K07" | "K08" | "K09" | "K10" => "in_progress",
        _ => "implemented_not_verified",
    }
}

fn validate_commitments(
    kind: &str,
    value: &Value,
    root: &Path,
    e_ids: &BTreeSet<String>,
    v_ids: &BTreeSet<String>,
    so_ids: &BTreeSet<String>,
    evidence_ids: &BTreeSet<String>,
) -> Result<(), String> {
    let array = value
        .as_array()
        .ok_or_else(|| format!("{kind} commitments must be an array"))?;
    let expected = expected_commitments(kind);
    if array.len() != expected.len() {
        return Err(format!("{kind} commitments have the wrong count"));
    }
    let mut by_id = BTreeMap::new();
    for entry in array {
        let id = nonempty_string(&entry["id"], &format!("{kind} commitment id"))?;
        if by_id.insert(id, entry).is_some() {
            return Err(format!("duplicate commitment id {id}"));
        }
    }
    for expected in expected {
        let entry = by_id
            .get(expected.id)
            .ok_or_else(|| format!("missing commitment {}", expected.id))?;
        if entry["specification_status"] != "included" || entry["effect_status"] != "not_claimed" {
            return Err(format!(
                "{} conflates specification or effect status",
                expected.id
            ));
        }
        for field in ["implementation_status", "verification_status"] {
            let status = nonempty_string(&entry[field], &format!("{} {field}", expected.id))?;
            if !ALLOWED_STATUSES.contains(&status) {
                return Err(format!("{} has invalid {field}", expected.id));
            }
        }
        if entry["implementation_status"] != expected_commitment_implementation_status(expected.id)
        {
            return Err(format!(
                "{} implementation status hides a known consumer gap or completed subset",
                expected.id
            ));
        }
        let sections = string_set(&entry["sections"], &format!("{} sections", expected.id))?;
        let sources = string_set(&entry["sources"], &format!("{} sources", expected.id))?;
        let tasks = string_set(&entry["e_tasks"], &format!("{} e_tasks", expected.id))?;
        let scenarios = string_set(
            &entry["v_scenarios"],
            &format!("{} v_scenarios", expected.id),
        )?;
        if sections != split_set(expected.sections)
            || sources != split_set(expected.sources)
            || tasks != split_set(expected.e_tasks)
            || scenarios != split_set(expected.scenarios)
        {
            return Err(format!("{} trace chain differs from v4.1", expected.id));
        }
        if !tasks.is_subset(e_ids) || !scenarios.is_subset(v_ids) {
            return Err(format!("{} points to an unknown E or V id", expected.id));
        }
        if kind == "K" && !sources.is_subset(so_ids) {
            return Err(format!("{} points to an unknown SO id", expected.id));
        }
        if kind == "B"
            && !sources.iter().all(|source| {
                matches!(
                    source.as_str(),
                    "RH01" | "RH02" | "RH03" | "RH04" | "RH05" | "RH06" | "RH07" | "RH08" | "RH09"
                )
            })
        {
            return Err(format!("{} points to an unknown RH id", expected.id));
        }
        let artifacts = entry["local_artifacts"]
            .as_array()
            .ok_or_else(|| format!("{} local_artifacts must be an array", expected.id))?;
        if artifacts.is_empty() {
            return Err(format!("{} has no local consumer/artifact", expected.id));
        }
        for artifact in artifacts {
            let artifact = nonempty_string(artifact, "local artifact")?;
            validate_repo_file(root, artifact, &format!("{} local artifact", expected.id))?;
        }
        let refs = string_set(
            &entry["related_evidence_refs"],
            &format!("{} related_evidence_refs", expected.id),
        )?;
        if !refs.is_subset(evidence_ids) {
            return Err(format!("{} references unknown evidence", expected.id));
        }
        nonempty_string(&entry["remaining"], &format!("{} remaining", expected.id))?;
    }
    Ok(())
}

struct SoExpected {
    id: &'static str,
    path: &'static str,
    blob: &'static str,
    range: &'static str,
    tasks: &'static str,
}

fn expected_so() -> Vec<SoExpected> {
    [
        (
            "SO01",
            "skillopt/engine/trainer.py",
            "520a90eb8af422368c8e498b8487504bb8f6c279",
            "1–240、1270–2110",
            "E03 E05 E09 E13 E14",
        ),
        (
            "SO02",
            "skillopt/gradient/reflect.py",
            "df68c9b76de83b2465b267b04310399ec5f3cec8",
            "300–700",
            "E03 E09",
        ),
        (
            "SO03",
            "skillopt/gradient/aggregate.py",
            "14f0cd978f35afe579913c9b5677a803bbce9e70",
            "全文",
            "E03 E09",
        ),
        (
            "SO04",
            "skillopt/optimizer/clip.py",
            "a2ed965f7c81c18426e82e2fd49e589b0952a92f",
            "全文",
            "E02 E03 E09",
        ),
        (
            "SO05",
            "skillopt/optimizer/skill.py",
            "65d574157f47e9b3f415d4db94b0e5a9c09e4826",
            "全文",
            "E02 E03 E06 E09",
        ),
        (
            "SO06",
            "skillopt/optimizer/skill_aware.py",
            "de39427ed0cc85fa1e716363a22140c91090b2d6",
            "全文",
            "E03 E07 E13",
        ),
        (
            "SO07",
            "skillopt/optimizer/slow_update.py",
            "9f8dd52606c3ada4fea1e6c0f72ec38f8a5139e8",
            "1–260",
            "E09 E13",
        ),
        (
            "SO08",
            "skillopt/optimizer/meta_skill.py",
            "6e34ff10d90655bb620d3e63b1d2041338616a5c",
            "全文",
            "E14 E15",
        ),
        (
            "SO09",
            "skillopt/evaluation/gate.py",
            "10461a98756ebacbf7d5a8bbdc67657950e2c199",
            "全文",
            "E05 E06",
        ),
        (
            "SO10",
            "configs/_base_/default.yaml",
            "4fe64088dc8011a2121335155d4a79ae7f90e9f5",
            "全文；本轮重读90–150",
            "E00 E03 E05 E13",
        ),
        (
            "SO11",
            "skillopt_sleep/consolidate.py",
            "5b80b4a9825fb5b5a79b04fdad4c655d5f3228f6",
            "全文",
            "E05 E06 E09",
        ),
        (
            "SO12",
            "skillopt_sleep/rollout.py",
            "f889333b3eecb83693a7005154dedadccbdc07f2",
            "全文",
            "E09 E12",
        ),
        (
            "SO13",
            "skillopt_sleep/replay.py",
            "1502a71cf9069b8ea23293acbb080225cdc51de2",
            "全文",
            "E04 E09 E10 E12",
        ),
        (
            "SO14",
            "skillopt_sleep/multi_skill.py",
            "32247639849e2c327ab7a1b7b3258dc7c35f53d4",
            "全文",
            "E06 E09",
        ),
        (
            "SO15",
            "skillopt_sleep/budget.py",
            "48875ca0f01cf099967ca2b5a45891e903645e36",
            "全文",
            "E01 E04 E11",
        ),
        (
            "SO16",
            "docs/sleep/RESULTS.md",
            "441ffa28f8644631c275456c7ccf4e75fd1d8c99",
            "全文",
            "E01 E07 E15",
        ),
        (
            "SO17",
            "docs/guide/training-loop.md",
            "30393dca9533de90e0cd0ff95f26eb327bfec0ff",
            "全文",
            "E03 E09",
        ),
        (
            "SO18",
            "LICENSE",
            "cf7bcb2ba475e33648180599c0f83c425f1705d9",
            "全文；本轮重读及固定字节校验",
            "E00 E16.2 E16.5",
        ),
    ]
    .into_iter()
    .map(|row| SoExpected {
        id: row.0,
        path: row.1,
        blob: row.2,
        range: row.3,
        tasks: row.4,
    })
    .collect()
}

fn validate_so_sources(
    value: &Value,
    root: &Path,
    e_ids: &BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    let array = value.as_array().ok_or("so_sources must be an array")?;
    let mut by_id = BTreeMap::new();
    for entry in array {
        let id = nonempty_string(&entry["id"], "SO id")?;
        if by_id.insert(id, entry).is_some() {
            return Err(format!("duplicate SO id {id}"));
        }
    }
    let expected = expected_so();
    if by_id.len() != expected.len() {
        return Err("SO01-SO18 exact set is incomplete".into());
    }
    for expected in &expected {
        let entry = by_id
            .get(expected.id)
            .ok_or_else(|| format!("missing {}", expected.id))?;
        if entry["repository"] != SKILLOPT_REPO
            || entry["commit"] != SKILLOPT_COMMIT
            || entry["path"] != expected.path
            || entry["blob_sha"] != expected.blob
            || entry["read_range"] != expected.range
            || entry["license"] != "MIT"
            || entry["license_status"] != "reference_pin_from_spec_not_reverified_by_cp001"
            || entry["verification_status"] != "reference_pins_only"
        {
            return Err(format!("{} pin/provenance differs from v4.1", expected.id));
        }
        let tasks = string_set(&entry["e_tasks"], &format!("{} e_tasks", expected.id))?;
        if tasks != split_set(expected.tasks) || !tasks.is_subset(e_ids) {
            return Err(format!("{} task mapping differs from v4.1", expected.id));
        }
        if entry["local_use"]["mode"] != "reference_only_no_upstream_runtime_import" {
            return Err(format!(
                "{} must state reference-only local use",
                expected.id
            ));
        }
        let artifacts = entry["local_use"]["artifacts"]
            .as_array()
            .ok_or_else(|| format!("{} local artifacts missing", expected.id))?;
        if artifacts.is_empty() {
            return Err(format!("{} has no explicit local landing", expected.id));
        }
        for artifact in artifacts {
            validate_repo_file(
                root,
                nonempty_string(artifact, "SO local artifact")?,
                &format!("{} local artifact", expected.id),
            )?;
        }
    }
    Ok(expected
        .into_iter()
        .map(|entry| entry.id.to_string())
        .collect())
}

fn expected_v_to_e() -> BTreeMap<String, BTreeSet<String>> {
    let mut result: BTreeMap<String, BTreeSet<String>> = (1..=98)
        .map(|i| (format!("V{i:03}"), BTreeSet::new()))
        .collect();
    for (task, scenarios) in expected_e_scenarios() {
        for scenario in scenarios {
            result.get_mut(scenario).unwrap().insert(task.to_string());
        }
    }
    result
}

fn validate_v_scenarios(
    value: &Value,
    evidence_ids: &BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    let array = value.as_array().ok_or("v_scenarios must be an array")?;
    let mut by_id = BTreeMap::new();
    for entry in array {
        let id = nonempty_string(&entry["id"], "V id")?;
        if by_id.insert(id, entry).is_some() {
            return Err(format!("duplicate V id {id}"));
        }
    }
    let expected = expected_v_to_e();
    let actual_ids: BTreeSet<String> = by_id.keys().map(|id| (*id).to_string()).collect();
    let expected_ids: BTreeSet<String> = expected.keys().cloned().collect();
    if actual_ids != expected_ids {
        return Err("V001-V098 exact set is incomplete".into());
    }
    for (id, expected_tasks) in &expected {
        let entry = by_id[id.as_str()];
        if entry["specification_status"] != "included" || entry["effect_status"] != "not_claimed" {
            return Err(format!("{id} conflates specification or effect status"));
        }
        for field in ["status", "verification_status"] {
            let status = nonempty_string(&entry[field], &format!("{id} {field}"))?;
            if !ALLOWED_STATUSES.contains(&status) {
                return Err(format!("{id} has unknown {field}"));
            }
        }
        let tasks = string_set(&entry["e_tasks"], &format!("{id} e_tasks"))?;
        if &tasks != expected_tasks {
            return Err(format!("{id} E responsibility mapping differs from v4.1"));
        }
        let refs = string_set(
            &entry["related_evidence_refs"],
            &format!("{id} evidence refs"),
        )?;
        if !refs.is_subset(evidence_ids) {
            return Err(format!("{id} references unknown evidence"));
        }
        nonempty_string(&entry["remaining"], &format!("{id} remaining"))?;
    }
    Ok(actual_ids)
}

fn validate_trace_index_coverage(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("trace_index_coverage must be an object")?;
    if object.keys().cloned().collect::<BTreeSet<_>>()
        != ["E00".to_string(), "E16.6".to_string()]
            .into_iter()
            .collect()
    {
        return Err("trace index coverage must be recorded separately for E00 and E16.6".into());
    }
    if object["E00"]["purpose"] != "index_consistency_only_not_scenario_execution"
        || string_set(&object["E00"]["ranges"], "E00 trace ranges")?
            != split_set("K01–K10 SO01–SO18 V087–V098")
    {
        return Err("E00 trace-only responsibility differs from v4.1".into());
    }
    if object["E16.6"]["purpose"] != "complete_derived_index_coverage_not_scenario_execution"
        || string_set(&object["E16.6"]["ranges"], "E16.6 trace ranges")?
            != split_set("B01–B10 U01–U08 K01–K10 SO01–SO18 E00–E18 E16.1–E16.6 V001–V098")
    {
        return Err("E16.6 complete index responsibility differs from v4.1".into());
    }
    Ok(())
}

fn validate_manifest(value: &Value, root: &Path) -> Result<(), String> {
    if value["schema_version"] != "rsia.support_scope.v2"
        || value["plan_version"] != "v4.1"
        || value["plan_sha256"] != PLAN_SHA
    {
        return Err("support index is not bound to the v4.1 plan".into());
    }
    if value["declaration"] != "subset_only"
        || value["not_full_route_complete"] != true
        || value["legacy_equivalence_unverified"] != true
        || value["dimensions"]["effect"] != "not_claimed"
    {
        return Err("support index overstates completion or effect".into());
    }
    let optional = string_set(&value["optional_disabled"], "optional_disabled")?;
    if optional != split_set("E17 E18") {
        return Err("E17 and E18 must remain explicitly disabled".into());
    }
    validate_trace_index_coverage(&value["trace_index_coverage"])?;
    let evidence_ids = validate_e_scopes(&value["e_scopes"], root)?;
    let e_ids = expected_e_ids();
    let so_ids = validate_so_sources(&value["traceability"]["so_sources"], root, &e_ids)?;
    let v_ids = validate_v_scenarios(&value["traceability"]["v_scenarios"], &evidence_ids)?;
    validate_commitments(
        "B",
        &value["traceability"]["b_commitments"],
        root,
        &e_ids,
        &v_ids,
        &so_ids,
        &evidence_ids,
    )?;
    validate_commitments(
        "U",
        &value["traceability"]["u_commitments"],
        root,
        &e_ids,
        &v_ids,
        &so_ids,
        &evidence_ids,
    )?;
    validate_commitments(
        "K",
        &value["traceability"]["k_commitments"],
        root,
        &e_ids,
        &v_ids,
        &so_ids,
        &evidence_ids,
    )?;
    Ok(())
}

#[test]
fn support_scope_is_a_complete_but_non_normative_v41_derivative() {
    let root = repo_root();
    let value = load_manifest(&root);
    validate_manifest(&value, &root).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(value["e_scopes"].as_object().unwrap().len(), 25);
    assert!(
        value["e_scopes"]
            .as_object()
            .unwrap()
            .values()
            .all(|entry| entry["status"] != "verified")
    );
}

#[test]
fn rejects_missing_e16_parent_and_optional_extensions() {
    let root = repo_root();
    for missing in ["E16", "E17", "E18"] {
        let mut value = load_manifest(&root);
        value["e_scopes"].as_object_mut().unwrap().remove(missing);
        assert!(
            validate_manifest(&value, &root).is_err(),
            "missing {missing} was accepted"
        );
    }
}

#[test]
fn rejects_unknown_status_and_verified_without_execution_evidence() {
    let root = repo_root();
    let mut unknown = load_manifest(&root);
    unknown["e_scopes"]["E17"]["status"] = Value::String("verified_optional".into());
    assert!(validate_manifest(&unknown, &root).is_err());

    let mut unsupported_verified = load_manifest(&root);
    unsupported_verified["e_scopes"]["E17"]["status"] = Value::String("verified".into());
    unsupported_verified["e_scopes"]["E17"]["remaining"] = Value::Array(vec![]);
    assert!(validate_manifest(&unsupported_verified, &root).is_err());
}

#[test]
fn rejects_verified_record_missing_input_or_actual_result() {
    let root = repo_root();
    for field in ["input_digest", "input_ref", "actual_result"] {
        let mut value = load_manifest(&root);
        value["e_scopes"]["E01"]["verified_subscopes"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            validate_manifest(&value, &root).is_err(),
            "missing {field} was accepted"
        );
    }
}

#[test]
fn rejects_failed_execution_and_wrong_per_pr_merge_sha() {
    let root = repo_root();
    let mut failed = load_manifest(&root);
    failed["e_scopes"]["E01"]["verified_subscopes"][0]["exit_code"] = serde_json::json!(101);
    assert!(validate_manifest(&failed, &root).is_err());

    let mut wrong_merge = load_manifest(&root);
    wrong_merge["e_scopes"]["E03"]["verified_subscopes"][0]["merged_sha"] =
        Value::String("59afd1663a0d5f5212077dd92beaaf7920cde0ff".into());
    assert!(validate_manifest(&wrong_merge, &root).is_err());
}

#[test]
fn rejects_well_formed_but_wrong_so_pin_and_mapping() {
    let root = repo_root();
    for field in ["blob_sha", "commit"] {
        let mut value = load_manifest(&root);
        value["traceability"]["so_sources"][0][field] =
            Value::String("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
        assert!(
            validate_manifest(&value, &root).is_err(),
            "wrong SO {field} was accepted"
        );
    }
    let mut mapping = load_manifest(&root);
    mapping["traceability"]["so_sources"][0]["e_tasks"] = serde_json::json!(["E99"]);
    assert!(validate_manifest(&mapping, &root).is_err());
}

#[test]
fn rejects_broken_commitment_and_v_trace_links() {
    let root = repo_root();
    let mut b = load_manifest(&root);
    b["traceability"]["b_commitments"][0]["e_tasks"] = serde_json::json!(["E18"]);
    assert!(validate_manifest(&b, &root).is_err());

    let mut v = load_manifest(&root);
    v["traceability"]["v_scenarios"][0]["e_tasks"] = serde_json::json!(["E18"]);
    assert!(validate_manifest(&v, &root).is_err());

    let mut hidden_gap = load_manifest(&root);
    hidden_gap["traceability"]["k_commitments"][4]["implementation_status"] =
        Value::String("implemented_not_verified".into());
    assert!(
        validate_manifest(&hidden_gap, &root).is_err(),
        "K05 cannot hide its missing consolidation consumer as implementation-complete"
    );
}

#[test]
fn rejects_fake_target_extra_command_and_path_escape() {
    let root = repo_root();
    let mut fake = load_manifest(&root);
    fake["e_scopes"]["E01"]["verified_subscopes"][0]["test_entry"] =
        Value::String("crates/evo-core/tests/not_real.rs".into());
    fake["e_scopes"]["E01"]["verified_subscopes"][0]["command"] =
        Value::String("cargo test --locked --offline -p evo-core --test not_real".into());
    assert!(validate_manifest(&fake, &root).is_err());

    let mut extra = load_manifest(&root);
    extra["e_scopes"]["E01"]["verified_subscopes"][0]["command"] = Value::String(
        "cargo test --locked --offline -p evo-core --test evaluation_v41 && echo pass".into(),
    );
    assert!(validate_manifest(&extra, &root).is_err());

    let mut escape = load_manifest(&root);
    escape["e_scopes"]["E01"]["implementation_files"][0] = Value::String("../outside.rs".into());
    assert!(validate_manifest(&escape, &root).is_err());
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_evidence_file() {
    use std::os::unix::fs::symlink;
    let unique = format!("rsia-support-scope-{}", std::process::id());
    let temp = std::env::temp_dir().join(unique);
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(temp.join("inside")).unwrap();
    std::fs::write(temp.join("outside"), b"evidence").unwrap();
    symlink(temp.join("outside"), temp.join("inside/link")).unwrap();
    let error = validate_repo_file(&temp, "inside/link", "fixture")
        .expect_err("a symlinked evidence file must be rejected");
    assert!(error.contains("symlink"));

    std::fs::create_dir_all(temp.join("real-directory")).unwrap();
    std::fs::write(temp.join("real-directory/evidence"), b"evidence").unwrap();
    symlink(temp.join("real-directory"), temp.join("directory-link")).unwrap();
    let error = validate_repo_file(&temp, "directory-link/evidence", "fixture")
        .expect_err("a parent directory symlink must be rejected even inside the root");
    assert!(error.contains("symlink component"));
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn planned_scope_can_omit_files_only_with_explicit_remaining_reason() {
    let root = repo_root();
    let value = load_manifest(&root);
    assert_eq!(value["e_scopes"]["E18"]["status"], "planned");
    assert!(
        value["e_scopes"]["E18"]["implementation_files"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !value["e_scopes"]["E18"]["remaining"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    validate_manifest(&value, &root).unwrap();
}
