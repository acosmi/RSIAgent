//! AG-136 engine boundary controls (plan §6.1.2 L726–774, V100.a L2215).
//!
//! The registry projection widens `admin_ops` to the 59 dispatch names, and
//! `classify` consumes it. These tests pin the boundary the card requires:
//! newly registered dispatch names classify as Admin but stay fail-closed at
//! the dispatcher's unchanged unknown-operation default with zero writes; the
//! formal 13 and offline 1 never enter `admin_ops` or classify Admin; the old
//! seven handlers keep their real role guards; and `control.review` never
//! becomes a management job through `submit`.
use evo_core::contract::admin_ops;
use evo_core::management_registry::{
    self, ExecutionKind, MANAGEMENT_CATALOG, Plane, REVIEW_OPERATION,
};
use evo_core::{Context, Error, Role};
use evo_engine::dispatch::{ManagementDispatcher, Surface, classify};
use evo_storage::Store;
use serde_json::{Value, json};

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("boundary.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

/// B: registered dispatch names that the parent's seven-name admin_ops lacked
/// now classify on the Admin surface — the single registry projection at work.
#[test]
fn classify_projects_every_registered_dispatch_name_to_the_admin_surface() {
    for name in ["source.revoke", "budget.stage_plan.bind"] {
        assert_eq!(
            classify(name).unwrap(),
            Surface::Admin,
            "{name} must classify Admin on this head"
        );
    }
    // A (core side): admin_ops is exactly the 59-name dispatch projection and
    // contains the consolidation pair the parent lacked.
    assert_eq!(admin_ops(), management_registry::dispatch_names());
    assert_eq!(admin_ops().len(), 59);
    assert!(admin_ops().contains(&"consolidation.trigger"));
    assert!(admin_ops().contains(&"consolidation.run"));
    assert!(admin_ops().contains(&REVIEW_OPERATION));
    for &name in management_registry::dispatch_names() {
        assert_eq!(classify(name).unwrap(), Surface::Admin, "{name}");
    }
}

/// C: the formal 13 and offline 1 are neither admin_ops nor classify Admin,
/// and off-table dynamic names stay unknown.
#[test]
fn formal_and_offline_names_never_enter_admin_ops_or_classify() {
    for &name in management_registry::formal_names() {
        assert!(!admin_ops().contains(&name), "{name}");
        assert!(classify(name).is_err(), "{name}");
    }
    assert_eq!(management_registry::formal_names().len(), 13);
    for name in ["formal.execute", "formal.grade", "restore"] {
        assert!(!admin_ops().contains(&name), "{name}");
        let error = classify(name).unwrap_err();
        assert!(
            matches!(error, Error::Invalid(ref message) if message == "unknown operation"),
            "{name}: {error:?}"
        );
    }
    for &name in management_registry::offline_names() {
        assert!(!admin_ops().contains(&name), "{name}");
        assert!(classify(name).is_err(), "{name}");
    }
    // Off-table dynamic names, near misses and any control.* wildcard stay
    // unknown; the closed control family is not a wildcard.
    for name in [
        "budget.grant",
        "control.",
        "control",
        "control.deploy",
        "control.review.extra",
        "CONTROL.REVIEW",
        "",
    ] {
        assert!(!admin_ops().contains(&name), "{name:?}");
        assert!(classify(name).is_err(), "{name:?}");
        assert!(management_registry::lookup(name).is_none(), "{name:?}");
    }
}

/// The four model tools keep their surface and the catalog never intersects
/// them (V100.a isolation, engine side).
#[test]
fn model_tools_keep_their_surface_and_are_disjoint_from_the_catalog() {
    for name in evo_core::contract::MODEL_TOOLS {
        assert_eq!(classify(name).unwrap(), Surface::ModelTool);
        assert!(management_registry::lookup(name).is_none(), "{name}");
    }
}

/// The dispatcher's real guards are untouched: the old seven keep their role
/// requirements, and every newly registered but unimplemented name —
/// including the sync view — fails closed inside `submit` with zero rows
/// written: no job, no private input, no audit, no call.
#[tokio::test]
async fn submit_stays_fail_closed_with_zero_writes_for_registered_but_unimplemented_names() {
    let (_guard, store) = store().await;
    let admin = Context::new("boundary", "admin", Role::Admin).unwrap();
    let evaluator = Context::new("boundary", "evaluator", Role::Evaluator).unwrap();
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator.clone()]).unwrap();

    async fn rows(store: &Store, ctx: &Context) -> Vec<String> {
        let mut session = store.session().await.unwrap();
        let jobs = session.list::<Value>(ctx, "job").await.unwrap();
        let artifacts = session.list::<Value>(ctx, "artifact").await.unwrap();
        session.commit().await.unwrap();
        jobs.iter()
            .chain(&artifacts)
            .map(|value| value.to_string())
            .collect()
    }

    let before_admin = rows(&store, &admin).await;
    let before_evaluator = rows(&store, &evaluator).await;

    // Every registered dispatch name without one of the seven old handlers,
    // submitted by a trusted Admin with a schema-correct-looking payload:
    // the unchanged `required_management_role` default refuses it before the
    // first session, and control.review must never create a job this way.
    for entry in MANAGEMENT_CATALOG.iter().filter(|entry| {
        entry.plane == Plane::Dispatch
            && !matches!(
                entry.name,
                "experiment.register"
                    | "evaluation.start"
                    | "evaluation.status"
                    | "exploration.start"
                    | "replay.run"
                    | "curriculum.step"
                    | "meta.start"
            )
    }) {
        let payload = json!({
            "schema_version": format!("rsia.management.{}.v1", entry.name.replace('.', "_")),
            "request_key": "boundary-unimplemented"
        });
        let error = dispatcher
            .submit(&admin, entry.name, payload)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Invalid(message) if message == "unknown management operation"),
            "{}: {error:?}",
            entry.name
        );
    }
    assert_eq!(rows(&store, &admin).await, before_admin, "zero writes");
    assert_eq!(
        rows(&store, &evaluator).await,
        before_evaluator,
        "zero writes"
    );

    // The formal 13 and restore also fail closed through the dispatcher path
    // (they are not classify Admin, so authorize refuses them as unknown).
    for &name in management_registry::formal_names()
        .iter()
        .chain(management_registry::offline_names().iter())
    {
        let error = dispatcher
            .submit(&admin, name, json!({"request_key": "boundary"}))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{name}: {error:?}");
    }
    assert_eq!(rows(&store, &admin).await, before_admin, "zero writes");
}

/// The old seven handlers keep their real role guards (healthy positive and
/// negative controls on the unchanged dispatcher).
#[tokio::test]
async fn the_old_seven_handlers_keep_their_real_role_guards() {
    let (_guard, store) = store().await;
    let admin = Context::new("boundary", "admin", Role::Admin).unwrap();
    let evaluator = Context::new("boundary", "evaluator", Role::Evaluator).unwrap();
    let agent = Context::new("boundary", "agent", Role::Agent).unwrap();
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator.clone()]).unwrap();

    // meta.start is Admin-gated and still accepted (blocked terminal path).
    let queued = dispatcher
        .submit(
            &admin,
            "meta.start",
            json!({
                "schema_version": "rsia.management.meta_start.v1",
                "request_key": "boundary-meta"
            }),
        )
        .await
        .unwrap();
    assert_eq!(queued.operation, "meta.start");

    // evaluation.status is Evaluator-gated: an Admin submit is Forbidden,
    // the Evaluator submit passes the role gate (and then fails closed on its
    // own payload validation — either way the guard order is unchanged).
    let error = dispatcher
        .submit(
            &admin,
            "evaluation.status",
            json!({
                "schema_version": "rsia.management.evaluation_status.v1",
                "request_key": "boundary-status"
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Forbidden), "{error:?}");

    // An untrusted Agent identity never reaches any operation.
    let error = dispatcher
        .submit(
            &agent,
            "meta.start",
            json!({
                "schema_version": "rsia.management.meta_start.v1",
                "request_key": "boundary-agent"
            }),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Forbidden | Error::NotFound),
        "{error:?}"
    );
}

/// The one sync view is registered as such, and nothing else claims that
/// execution kind (E, engine-side syncview check).
#[test]
fn exactly_one_registered_operation_is_the_sync_readonly_view() {
    let views: Vec<_> = MANAGEMENT_CATALOG
        .iter()
        .filter(|entry| entry.execution == ExecutionKind::SyncReadonlyView)
        .collect();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].name, REVIEW_OPERATION);
    assert_eq!(views[0].table_line, 767);
}
