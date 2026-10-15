//! AG-136 MCP isolation controls (plan V100.a L2215, §6.1.2 L726).
//!
//! The actual MCP router must keep exposing exactly the four model tools, the
//! full 73-name management catalog must be disjoint from them, and every
//! registered management name must be rejected as a model tool while the four
//! model tools stay accepted.
use evo_core::contract::{MODEL_TOOLS, admin_ops};
use evo_core::management_registry;
use evo_mcp::{TOOL_COUNT, model_tools, reject_if_admin, tool_descriptors};

/// C: the real router output is exactly the four model tools.
#[test]
fn actual_router_output_is_exactly_the_four_model_tools() {
    assert_eq!(TOOL_COUNT, 4);
    let descriptors = tool_descriptors().unwrap();
    assert_eq!(descriptors.len(), 4);
    let mut names: Vec<&str> = descriptors
        .iter()
        .map(|descriptor| descriptor["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    let mut expected: Vec<&str> = MODEL_TOOLS.to_vec();
    expected.sort_unstable();
    assert_eq!(names, expected);
    assert_eq!(model_tools(), &MODEL_TOOLS[..]);
}

/// C/V100.a: the management catalog (73) and the model tools never intersect;
/// management names never appear in any tool descriptor.
#[test]
fn management_catalog_is_disjoint_from_model_tools() {
    let all = management_registry::all_names();
    assert_eq!(all.len(), 73);
    for &name in all {
        assert!(
            !MODEL_TOOLS.contains(&name),
            "{name} leaked into model tools"
        );
    }
    for tool in MODEL_TOOLS {
        assert!(
            management_registry::lookup(tool).is_none(),
            "{tool} must not be a registered management name"
        );
    }
    let serialized = serde_json::to_string(&tool_descriptors().unwrap()).unwrap();
    for &name in all {
        assert!(
            !serialized.contains(name),
            "{name} appeared in the tool descriptor payload"
        );
    }
}

/// V100.a closure: every registered management name on every plane is
/// rejected as a model tool; the four model tools are accepted; anything
/// else — including near misses and control-family wildcards — is an unknown
/// model tool, never silently allowed.
#[test]
fn reject_if_admin_refuses_all_73_registered_names_only() {
    for &name in management_registry::all_names() {
        let error = reject_if_admin(name).unwrap_err();
        assert!(
            matches!(error, evo_core::Error::Forbidden),
            "{name}: {error:?}"
        );
    }
    for tool in MODEL_TOOLS {
        assert!(reject_if_admin(tool).is_ok(), "{tool}");
    }
    for name in [
        "budget.grant",
        "evaluation.startt",
        "evo_prepare_admin",
        "control.revie",
        "control.review2",
        "",
    ] {
        let error = reject_if_admin(name).unwrap_err();
        assert!(
            matches!(error, evo_core::Error::Invalid(ref message) if message == "unknown model tool"),
            "{name:?}: {error:?}"
        );
    }
}

/// The dispatch projection served to the CLI/dispatcher gates is the registry
/// projection, and the formal/offline planes are outside it.
#[test]
fn admin_ops_is_the_dispatch_projection_and_excludes_formal_and_offline() {
    assert_eq!(admin_ops(), management_registry::dispatch_names());
    assert_eq!(admin_ops().len(), 59);
    for &name in management_registry::formal_names()
        .iter()
        .chain(management_registry::offline_names().iter())
    {
        assert!(!admin_ops().contains(&name), "{name}");
    }
}
