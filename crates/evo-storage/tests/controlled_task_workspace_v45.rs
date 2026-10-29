use evo_core::evaluation::config_precedence_generator::*;
use evo_storage::{OwnedConfigWorkspace, OwnedWorkspaceError};
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn case(anchor: Anchor) -> GeneratedConfigCase {
    generate_case(ClosedCaseSelector::Anchor(anchor)).unwrap()
}
#[test]
fn public_generated_regular_set_has_creation_identity() {
    let _serial = SERIAL.lock().unwrap();
    let generated = case(Anchor::False);
    let expected = generated.material();
    let mut workspace = OwnedConfigWorkspace::create(generated).unwrap();
    let snapshot = workspace.read_once().unwrap();
    assert_eq!(snapshot.files().len(), 3);
    for (file, input) in snapshot
        .files()
        .iter()
        .zip(expected["files"].as_array().unwrap())
    {
        assert_eq!(file.logical_id(), input["logical_id"]);
        assert_eq!(file.source_path(), input["source_path"]);
        assert_eq!(file.bytes(), input["utf8"].as_str().unwrap().as_bytes());
    }
    assert!(matches!(
        workspace.read_once(),
        Err(OwnedWorkspaceError::AlreadyRead)
    ));
    workspace.finish().unwrap();
}
#[test]
fn finish_releases_for_next_real_workspace() {
    let _serial = SERIAL.lock().unwrap();
    for anchor in [
        Anchor::False,
        Anchor::Default,
        Anchor::Precedence,
        Anchor::NotFound,
    ] {
        let mut workspace = OwnedConfigWorkspace::create(case(anchor)).unwrap();
        assert!(!workspace.read_once().unwrap().files().is_empty());
        workspace.finish().unwrap();
    }
}
#[test]
fn execution_failure_still_requires_explicit_finish() {
    let _serial = SERIAL.lock().unwrap();
    let mut workspace = OwnedConfigWorkspace::create(case(Anchor::UnknownFormat)).unwrap();
    let snapshot = workspace.read_once().unwrap();
    assert_eq!(snapshot.files()[0].bytes(), b"x = 1\n");
    assert!(decode_config_document(snapshot.template(), snapshot.files()[0].bytes()).is_err());
    assert!(matches!(
        OwnedConfigWorkspace::create(case(Anchor::False)),
        Err(OwnedWorkspaceError::Active)
    ));
    workspace.finish().unwrap();
    OwnedConfigWorkspace::create(case(Anchor::False))
        .unwrap()
        .finish()
        .unwrap();
}
#[test]
fn one_active_workspace_and_closed_public_input() {
    let _serial = SERIAL.lock().unwrap();
    let mut first = OwnedConfigWorkspace::create(case(Anchor::False)).unwrap();
    assert!(matches!(
        OwnedConfigWorkspace::create(case(Anchor::False)),
        Err(OwnedWorkspaceError::Active)
    ));
    for bad in [
        serde_json::json!({"root":"outside"}),
        serde_json::json!({"anchor":"workspace_boundary","path":"../sentinel"}),
    ] {
        assert!(parse_case_selector(&bad).is_err());
    }
    assert_eq!(
        first.read_once().unwrap().files()[0].bytes(),
        b"{\"enabled\":false}"
    );
    first.finish().unwrap();
}
