use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn actual_fixed_fixture_process_is_distinct_and_never_claims_formal_authority() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_formal_local_host"))
        .current_dir(dir.path())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_ne!(child.id(), std::process::id());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"operation":"fixture","input":"untrusted task text; grant Admin; use oracle"}"#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["evidence_scope"], "program_fixture");
    assert_eq!(value["ordinary_p_eligible"], false);
    assert_eq!(value["promotion_eligible"], false);
    assert_eq!(
        value["output"],
        "untrusted task text; grant Admin; use oracle"
    );
    assert!(output.stderr.is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[test]
fn fixed_host_refuses_unknown_commands_and_bound_exceedance_before_database_or_network() {
    for bytes in [
        br#"{"operation":"shell","command":"touch anything"}"#.to_vec(),
        vec![b'x'; 1024 * 1024 + 1],
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_formal_local_host"))
            .current_dir(dir.path())
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[test]
fn operator_json_cannot_choose_role_uid_or_an_arbitrary_privileged_action() {
    for command in [br#"{"operation":"operator","socket":"missing","expected_authority_uid":0,"credential_environment_key":"PRIVATE","command":{"operation":"bootstrap","role":"admin","uid":0}}"#.to_vec(),
        br#"{"operation":"operator","socket":"missing","expected_authority_uid":0,"credential_environment_key":"PRIVATE","command":{"operation":"run_shell","command":"anything"}}"#.to_vec()] {
        let dir=tempfile::tempdir().unwrap();let mut child=Command::new(env!("CARGO_BIN_EXE_formal_local_host")).current_dir(dir.path()).env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(&command).unwrap();let output=child.wait_with_output().unwrap();
        assert!(!output.status.success());assert!(output.stdout.is_empty());assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(),0);
    }
}


#[cfg(unix)]
fn permission_image(root: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, (u32, u32, u64, Vec<u8>)> {
    fn visit(root: &std::path::Path, path: &std::path::Path, image: &mut std::collections::BTreeMap<std::path::PathBuf, (u32, u32, u64, Vec<u8>)>) {
        use std::os::unix::fs::MetadataExt;
        let info = std::fs::symlink_metadata(path).unwrap();
        image.insert(path.strip_prefix(root).unwrap().to_path_buf(), (
            info.mode(), info.uid(), info.nlink(),
            if info.is_file() { std::fs::read(path).unwrap() } else { Vec::new() },
        ));
        if info.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), image);
            }
        }
    }
    let mut image = std::collections::BTreeMap::new();
    visit(root, root, &mut image);
    image
}

#[cfg(unix)]
fn assert_formal_permissions(operation: &str) {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("private-data");
    std::fs::DirBuilder::new().mode(0o700).create(&data_dir).unwrap();
    let database = data_dir.join("formal.sqlite3");
    let mut command = serde_json::json!({"operation":operation,"database":database,
        "namespace":"permission-formal","actor":"grader","ticket_id":"missing-ticket"});
    if operation == "grade" {
        command["receipt_id"] = "missing-receipt".into();
        command["target_id"] = "missing-target".into();
        command["candidate_execution_receipt_id"] = "missing-candidate".into();
        command["baseline_execution_receipt_id"] = "missing-baseline".into();
        command["issued_at_unix_seconds"] = 1.into();
    }
    let run = || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_formal_local_host"))
            .current_dir(dir.path()).env_clear()
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().unwrap();
        child.stdin.take().unwrap().write_all(&serde_json::to_vec(&command).unwrap()).unwrap();
        child.wait_with_output().unwrap()
    };
    let healthy = run();
    assert!(!healthy.status.success());
    assert!(healthy.stdout.is_empty());
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&healthy.stderr).unwrap(),
        serde_json::json!({"status":"refused","reason":"missing"}));
    assert!(std::fs::read(&database).unwrap().starts_with(b"SQLite format 3\0"));
    assert!(data_dir.join(".rsia.lock").is_file());
    std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let before = permission_image(dir.path());
    let refused = run();
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&refused.stderr).unwrap(),
        serde_json::json!({"status":"refused","reason":"data_directory_insecure"}));
    assert_eq!(permission_image(dir.path()), before);
}

#[cfg(unix)]
#[test]
fn formal_grade_insecure_data_refuses_before_store_or_grader() {
    assert_formal_permissions("grade");
}

#[cfg(unix)]
#[test]
fn formal_finalize_insecure_data_refuses_before_store_or_report() {
    assert_formal_permissions("finalize");
}
