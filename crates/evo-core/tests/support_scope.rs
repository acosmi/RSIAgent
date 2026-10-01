//! Strict derived support-scope verification for E16.6 (V071/V072/V074/V080/V098).
//! The current plan (lineage v4.1→v4.2, plan §18.8) remains normative; this test rejects
//! incomplete or inflated indexes. Historical evidence records keep the lineage digest they
//! were verified against; only the top level must bind the current plan.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

const PLAN_VERSION: &str = "v4.2";
const PLAN_SHA: &str = "70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455";
/// Registered plan lineage (plan §18.8) as (version, file, sha256), oldest first; the last
/// entry is the current binding.
const PLAN_LINEAGE: &[(&str, &str, &str)] = &[
    (
        "v4.1",
        "RSIAgent-v4.1定稿-工程实施方案-2026-09-19.md",
        "45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150",
    ),
    (
        "v4.2",
        "RSIAgent-v4.2定稿-工程实施方案-2026-09-30.md",
        "70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455",
    ),
];
/// Optional per-record recheck marker; a missing key means not_affected and "required" never passes.
const PLAN_RECHECKED: &str = "rechecked_v4.2";
const PLAN_RECHECK_VALUES: &[&str] = &["not_affected", "required", PLAN_RECHECKED];
/// Tasks whose historical records are touched by the v4.2 revision (plan §19.4).
const PLAN_RECHECK_TASKS: &[&str] = &["E00", "E16.5", "E16.6"];
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
        ("E09", "V019 V020 V022 V024 V025 V081 V086 V088 V089 V090 V091 V093 V094 V095 V096 V097"),
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
        44 => Some("acda31895bb1cb42cf7985b907a4c600429573d0"),
        47 => Some("1c824e6614667da4dc7ea74e96b46adfd1d2c089"),
        48 => Some("4980aa498baa80fd82e83441839e606859fa4065"),
        50 => Some("e94042d3f2fd761455727a6c82ad13fd3d657d03"),
        51 => Some("bef7bd1763a01ccb677ada4acad71a063366539c"),
        54 => Some("16c2817bc192bf71535d2c99d867873e6e85bcf8"),
        60 => Some("dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b"),
        61 => Some("b220971c4e5ad44453c9d3e5787fbdfda6eb01ac"),
        62 => Some("9c3ec979fd2ee3c53dfbf4ddf96967e94808b11e"),
        63 => Some("3e2fc067190077b8cf8401875d6c627b8d9e13fc"),
        64 => Some("f8fa1a19d5e968f83a52385ecb43d07d56442610"),
        66 => Some("5a0ef4419c51687534baa3296dbbf7e310a5d810"),
        67 => Some("5c4f81dca79373c6871449aed287f61fb1f3cbc2"),
        69 => Some("31566ce87203b44c44b72986233af1c02fbe5cda"),
        70 => Some("c81b258b6f890ab78bc9c7111798f75560c2ea23"),
        71 => Some("9519c26f049afff63154fb2337893217b18b7e3c"),
        72 => Some("15b140ddef25c8e794d586fecae56f129226a5c6"),
        73 => Some("7ad4994c7a9208f523612d305660dc8ca9529620"),
        74 => Some("c72aae14c9f35c1bcc090b5a374d2696d57dacfc"),
        75 => Some("6a23cb3697f656e1f5132cbe62ae766b4c0383f0"),
        76 => Some("7120842a1f586ae394b76e146c8298f11bec52bc"),
        77 => Some("887b1d714581ae8a71d45243d38be1759697600d"),
        78 => Some("84d8a7054c2428db0f821fcd478c615b09ade76b"),
        79 => Some("b262bbb2a0ea9ead68bc33fe1e58235e85bcffea"),
        80 => Some("9e3e350ed5fbef2975117f12a7f8d3ca3b9cdcbf"),
        81 => Some("d9b3f6f5234ad14da18956a78c804e4c6818fa10"),
        82 => Some("8d959dee844fb16f0763b0684ba3adf48ad75f1f"),
        83 => Some("51db9b6c9a251def4e6566c96641ed8b63d43a85"),
        84 => Some("42f89bd602db0c2783de38d87d91e80cd65f220b"),
        85 => Some("f4b1ef66340412f5c0edf69a48549cd1386324cf"),
        86 => Some("16bdb08e1ac118e3e732a912358add468c49173b"),
        87 => Some("32a2f5090fe5136b21c7d3454f53964b36990247"),
        88 => Some("ca117c45d87986500b38bc5afee3d6e161fea4f5"),
        89 => Some("cc85b5c71cec06763909fb0efa33c4e296b6178d"),
        90 => Some("ecdfe31faabb21a368f08d341b2a6e15223b0849"),
        91 => Some("a38b1452ede8fc5755bcab65686edd2f2817b51a"),
        92 => Some("696252faf8ef2b2391102f4e6b0e9c0ee20c53d5"),
        93 => Some("81449ccd1a3a9f73b464438d8a6edd26a0ff9734"),
        94 => Some("4a8487413cb8fe51ee16b91a538a71816997adfb"),
        95 => Some("e2c9107142413c0ba304b5090692b56037958f40"),
        96 => Some("49d5b85ec4fa757144213573c585087798324e91"),
        97 => Some("804302602efe575d92973be20c532d264019f9aa"),
        99 => Some("3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360"),
        100 => Some("43f0022478d58dd8a8e6492cffb3aa24463b3fce"),
        101 => Some("296b5a07a7bf911d02454b683d5732e9d38e3ae5"),
        103 => Some("ba01a2e8489ceef6d1350f0babdac46d816071ec"),
        104 => Some("9b08c4b2fd882abab65f48ea0af8f23b13a3b50d"),
        105 => Some("5337f4b17558b4c95176885d39f2518142d965b9"),
        106 => Some("279ee7c5290fabbb20cbd16b7e53c5ac2b71c188"),
        108 => Some("6f0e5fa35b582f0f4cde4207ddf59d65a51b180a"),
        109 => Some("c6e13e906ea888916294e6deac77c37f4391225f"),
        110 => Some("eb275812f09dd8850ad4edac08a2282cb5e82112"),
        _ => return Err(format!("no reviewed merge status for PR {pr}")),
    })
}

fn validate_evidence_record(record: &Value, task: &str, root: &Path) -> Result<String, String> {
    let id = nonempty_string(&record["id"], &format!("{task} evidence id"))?;
    nonempty_string(&record["description"], &format!("{id} description"))?;
    let version = nonempty_string(&record["plan_version"], &format!("{id} plan_version"))?;
    let digest = nonempty_string(&record["plan_sha256"], &format!("{id} plan_sha256"))?;
    let Some((_, _, registered)) = PLAN_LINEAGE.iter().find(|(known, _, _)| *known == version)
    else {
        return Err(format!(
            "{id} plan_version {version} is outside the registered plan lineage"
        ));
    };
    if digest != *registered {
        return Err(format!(
            "{id} plan_sha256 is not the registered digest of plan {version}"
        ));
    }
    let recheck = match record.get("plan_recheck") {
        None => "not_affected",
        Some(value) => nonempty_string(value, &format!("{id} plan_recheck"))?,
    };
    if !PLAN_RECHECK_VALUES.contains(&recheck) {
        return Err(format!(
            "{id} plan_recheck {recheck} is not a registered marker"
        ));
    }
    if recheck == "required" {
        return Err(format!(
            "{id} still requires a recheck against plan {PLAN_VERSION}"
        ));
    }
    if version != PLAN_VERSION && PLAN_RECHECK_TASKS.contains(&task) && recheck != PLAN_RECHECKED {
        return Err(format!(
            "{id} belongs to {task}, which the {PLAN_VERSION} revision touches, but was not rechecked"
        ));
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
            return Err(format!(
                "{task} scenario responsibility differs from plan {PLAN_VERSION}"
            ));
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
            return Err(format!(
                "{} trace chain differs from plan {PLAN_VERSION}",
                expected.id
            ));
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
            return Err(format!(
                "{} pin/provenance differs from plan {PLAN_VERSION}",
                expected.id
            ));
        }
        let tasks = string_set(&entry["e_tasks"], &format!("{} e_tasks", expected.id))?;
        if tasks != split_set(expected.tasks) || !tasks.is_subset(e_ids) {
            return Err(format!(
                "{} task mapping differs from plan {PLAN_VERSION}",
                expected.id
            ));
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
            return Err(format!(
                "{id} E responsibility mapping differs from plan {PLAN_VERSION}"
            ));
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
        return Err(format!(
            "E00 trace-only responsibility differs from plan {PLAN_VERSION}"
        ));
    }
    if object["E16.6"]["purpose"] != "complete_derived_index_coverage_not_scenario_execution"
        || string_set(&object["E16.6"]["ranges"], "E16.6 trace ranges")?
            != split_set("B01–B10 U01–U08 K01–K10 SO01–SO18 E00–E18 E16.1–E16.6 V001–V098")
    {
        return Err(format!(
            "E16.6 complete index responsibility differs from plan {PLAN_VERSION}"
        ));
    }
    Ok(())
}

fn validate_plan_lineage(value: &Value) -> Result<(), String> {
    let array = value.as_array().ok_or("plan_lineage must be an array")?;
    if array.len() != PLAN_LINEAGE.len() {
        return Err(format!(
            "plan_lineage must list exactly the {} registered plan versions",
            PLAN_LINEAGE.len()
        ));
    }
    let allowed: BTreeSet<&str> = ["version", "file", "sha256"].into_iter().collect();
    for (entry, (version, file, sha256)) in array.iter().zip(PLAN_LINEAGE) {
        let object = entry
            .as_object()
            .ok_or("plan_lineage entries must be objects")?;
        if object.keys().map(String::as_str).collect::<BTreeSet<_>>() != allowed {
            return Err(format!(
                "plan_lineage entry for {version} carries unknown or missing keys"
            ));
        }
        if entry["version"] != *version || entry["file"] != *file || entry["sha256"] != *sha256 {
            return Err(format!(
                "plan_lineage entry for {version} differs from the registered lineage"
            ));
        }
    }
    let last = array.last().ok_or("plan_lineage must not be empty")?;
    if last["version"] != PLAN_VERSION || last["sha256"] != PLAN_SHA {
        return Err(format!(
            "plan_lineage must end with the current plan {PLAN_VERSION}"
        ));
    }
    Ok(())
}

fn validate_manifest(value: &Value, root: &Path) -> Result<(), String> {
    if value["schema_version"] != "rsia.support_scope.v2"
        || value["plan_version"] != PLAN_VERSION
        || value["plan_sha256"] != PLAN_SHA
    {
        return Err(format!(
            "support index top level is not bound to the current plan {PLAN_VERSION}"
        ));
    }
    validate_plan_lineage(&value["plan_lineage"])?;
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
fn support_scope_is_a_complete_but_non_normative_plan_derivative() {
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
fn historical_records_keep_lineage_digests_but_top_level_must_be_current() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    let record = &baseline["e_scopes"]["E01"]["verified_subscopes"][0];
    assert_eq!(record["plan_version"], "v4.1");
    assert_eq!(record["plan_sha256"], PLAN_LINEAGE[0].2);
    validate_manifest(&baseline, &root).unwrap();

    let mut current = load_manifest(&root);
    current["e_scopes"]["E01"]["verified_subscopes"][0]["plan_version"] =
        Value::String(PLAN_VERSION.into());
    current["e_scopes"]["E01"]["verified_subscopes"][0]["plan_sha256"] =
        Value::String(PLAN_SHA.into());
    validate_manifest(&current, &root).unwrap();

    let mut mismatched = load_manifest(&root);
    mismatched["e_scopes"]["E01"]["verified_subscopes"][0]["plan_version"] =
        Value::String(PLAN_VERSION.into());
    let error = validate_manifest(&mismatched, &root)
        .expect_err("a v4.2 version paired with the v4.1 digest was accepted");
    assert!(
        error.contains("not the registered digest of plan v4.2"),
        "{error}"
    );

    let mut outside = load_manifest(&root);
    outside["e_scopes"]["E01"]["verified_subscopes"][0]["plan_sha256"] =
        Value::String("a".repeat(64));
    assert!(
        validate_manifest(&outside, &root).is_err(),
        "a digest outside the lineage was accepted"
    );

    let mut unknown_version = load_manifest(&root);
    unknown_version["e_scopes"]["E01"]["verified_subscopes"][0]["plan_version"] =
        Value::String("v4.0".into());
    let error = validate_manifest(&unknown_version, &root)
        .expect_err("an unregistered plan version was accepted");
    assert!(
        error.contains("outside the registered plan lineage"),
        "{error}"
    );

    let mut stale_top = load_manifest(&root);
    stale_top["plan_version"] = Value::String("v4.1".into());
    stale_top["plan_sha256"] = Value::String(PLAN_LINEAGE[0].2.into());
    let error = validate_manifest(&stale_top, &root)
        .expect_err("a top level still bound to v4.1 was accepted");
    assert!(
        error.contains("not bound to the current plan v4.2"),
        "{error}"
    );
}

#[test]
fn rejects_pending_or_unknown_plan_recheck_markers() {
    let root = repo_root();
    let mut required = load_manifest(&root);
    required["e_scopes"]["E01"]["verified_subscopes"][0]["plan_recheck"] =
        Value::String("required".into());
    let error = validate_manifest(&required, &root)
        .expect_err("a record still requiring a recheck was accepted");
    assert!(
        error.contains("E01.controller_acceptance still requires a recheck"),
        "{error}"
    );

    let mut unknown = load_manifest(&root);
    unknown["e_scopes"]["E01"]["verified_subscopes"][0]["plan_recheck"] =
        Value::String("improved".into());
    assert!(validate_manifest(&unknown, &root).is_err());

    let mut untouched_without_marker = load_manifest(&root);
    untouched_without_marker["e_scopes"]["E01"]["verified_subscopes"][0]
        .as_object_mut()
        .unwrap()
        .remove("plan_recheck");
    validate_manifest(&untouched_without_marker, &root).unwrap();

    for task in ["E00", "E16.5"] {
        let mut downgraded = load_manifest(&root);
        let record = &mut downgraded["e_scopes"][task]["verified_subscopes"][0];
        assert_eq!(record["plan_recheck"], PLAN_RECHECKED);
        record["plan_recheck"] = Value::String("not_affected".into());
        assert!(
            validate_manifest(&downgraded, &root).is_err(),
            "{task}: a v4.1 record of a touched task without a recheck was accepted"
        );
        let mut missing = load_manifest(&root);
        missing["e_scopes"][task]["verified_subscopes"][0]
            .as_object_mut()
            .unwrap()
            .remove("plan_recheck");
        assert!(
            validate_manifest(&missing, &root).is_err(),
            "{task}: a v4.1 record of a touched task with no marker was accepted"
        );
    }
}

#[test]
fn rejects_missing_or_unregistered_plan_lineage() {
    let root = repo_root();
    let mut missing = load_manifest(&root);
    missing.as_object_mut().unwrap().remove("plan_lineage");
    assert!(validate_manifest(&missing, &root).is_err());

    let mut reversed = load_manifest(&root);
    reversed["plan_lineage"].as_array_mut().unwrap().reverse();
    assert!(validate_manifest(&reversed, &root).is_err());

    let mut truncated = load_manifest(&root);
    truncated["plan_lineage"].as_array_mut().unwrap().remove(0);
    assert!(validate_manifest(&truncated, &root).is_err());

    let mut wrong_digest = load_manifest(&root);
    wrong_digest["plan_lineage"][1]["sha256"] = Value::String(PLAN_LINEAGE[0].2.into());
    assert!(validate_manifest(&wrong_digest, &root).is_err());

    let mut extra_key = load_manifest(&root);
    extra_key["plan_lineage"][1]["effect"] = Value::String("improved".into());
    assert!(validate_manifest(&extra_key, &root).is_err());

    let mut extended = load_manifest(&root);
    extended["plan_lineage"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "version": "v4.3",
            "file": "future.md",
            "sha256": "b".repeat(64),
        }));
    assert!(validate_manifest(&extended, &root).is_err());
}

#[test]
fn e09_scenarios_include_the_v097_ablation_obligation() {
    assert!(expected_e_scenarios()["E09"].contains("V097"));
    assert!(expected_v_to_e()["V097"].contains("E09"));
    let root = repo_root();
    let mut without_scenario = load_manifest(&root);
    without_scenario["e_scopes"]["E09"]["scenarios"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item.as_str() != Some("V097"));
    assert!(validate_manifest(&without_scenario, &root).is_err());

    let mut without_task = load_manifest(&root);
    let v097 = without_task["traceability"]["v_scenarios"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["id"] == "V097")
        .unwrap();
    v097["e_tasks"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item.as_str() != Some("E09"));
    assert!(validate_manifest(&without_task, &root).is_err());
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
    assert!(
        unsupported_verified["e_scopes"]["E15"]["verified_subscopes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    unsupported_verified["e_scopes"]["E15"]["status"] = Value::String("verified".into());
    unsupported_verified["e_scopes"]["E15"]["remaining"] = Value::Array(vec![]);
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
    let mut value = load_manifest(&root);
    assert_eq!(value["e_scopes"]["E15"]["status"], "planned");
    value["e_scopes"]["E15"]["implementation_files"] = Value::Array(vec![]);
    assert!(
        !value["e_scopes"]["E15"]["remaining"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    validate_manifest(&value, &root).unwrap();

    let mut started = load_manifest(&root);
    assert_eq!(started["e_scopes"]["E18"]["status"], "in_progress");
    started["e_scopes"]["E18"]["implementation_files"] = Value::Array(vec![]);
    assert!(
        validate_manifest(&started, &root).is_err(),
        "a started scope without implementation files was accepted"
    );
}
