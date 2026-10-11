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
