//! AG-136 engine boundary controls (plan §6.1.2 L726–774, V100.a L2215).
//!
//! The registry projection widens `admin_ops` to the 61 dispatch names, and
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
    // A (core side): admin_ops is exactly the 61-name dispatch projection and
    // contains the consolidation pair the parent lacked.
    assert_eq!(admin_ops(), management_registry::dispatch_names());
    assert_eq!(admin_ops().len(), 61);
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
/// including the dedicated sync view — fails closed inside `submit` with zero rows
/// written: no job, no private input, no audit, no call.
#[tokio::test]
async fn submit_stays_fail_closed_with_zero_writes_for_registered_but_unimplemented_names() {
    let (dir, store) = store().await;
    let admin = Context::new("boundary", "admin", Role::Admin).unwrap();
    let evaluator = Context::new("boundary", "evaluator", Role::Evaluator).unwrap();
    let dispatcher =
        ManagementDispatcher::new(store.clone(), vec![admin.clone(), evaluator.clone()]).unwrap();
    let path = dir.path().join("boundary.sqlite3");
    let before_all = management_sql_snapshot(&path);

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
    // the static operation gate refuses it before the
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
            matches!(&error, Error::Invalid(message) if message == match entry.execution {
                ExecutionKind::AsyncJob => "management_operation_not_wired",
                _ => "management operation requires dedicated route",
            }),
            "{}: {error:?}",
            entry.name
        );
        assert_eq!(
            management_sql_snapshot(&path),
            before_all,
            "{} must leave every SQL table, sequence, rowid and typed cell unchanged",
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
    // (they are not classify Admin, so management submit refuses their plane).
    for &name in management_registry::formal_names()
        .iter()
        .chain(management_registry::offline_names().iter())
    {
        let error = dispatcher
            .submit(&admin, name, json!({"request_key": "boundary"}))
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::Invalid(ref message) if message == "management_operation_plane_mismatch"),
            "{name}: {error:?}"
        );
        assert_eq!(
            management_sql_snapshot(&path),
            before_all,
            "{name} must leave every SQL table, sequence, rowid and typed cell unchanged"
        );
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
    assert_eq!(views[0].table_line, 841);
}

// A read-only SQLite snapshot retains every table, row identity and cell type.
// Python's sqlite3 is already part of the repository's required test runtime.
fn management_sql_snapshot(path: &std::path::Path) -> Vec<u8> {
    let result = std::process::Command::new("python3")
        .args(["-B", "-c", r#"
import json, pathlib, sqlite3, struct, sys
con = sqlite3.connect(pathlib.Path(sys.argv[1]).resolve().as_uri() + '?mode=ro', uri=True)
con.execute('BEGIN')
def cell(v):
    if v is None: return ['null']
    if isinstance(v, int): return ['integer', str(v)]
    if isinstance(v, float): return ['real', struct.pack('>d', v).hex()]
    if isinstance(v, bytes): return ['blob', v.hex()]
    return ['text', v.encode('utf-8').hex()]
facts = []
for name, ddl in con.execute("SELECT name, sql FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    table = '"' + name.replace('"', '""') + '"'
    columns = '*' if 'WITHOUT ROWID' in (ddl or '').upper() else 'rowid,*'
    rows = [[cell(v) for v in row] for row in con.execute('SELECT ' + columns + ' FROM ' + table)]
    rows.sort(key=lambda row: json.dumps(row, separators=(',', ':')))
    facts.append([name, ddl, rows])
print(json.dumps(facts, separators=(',', ':')))
con.close()
"#])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.stdout.is_empty());
    result.stdout
}

#[tokio::test]
async fn registered_unwired_async_rejects_before_payload_or_first_session() {
    let (dir, store) = store().await;
    let admin = Context::new("boundary", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let path = dir.path().join("boundary.sqlite3");
    let before = management_sql_snapshot(&path);
    let session = store.session().await.unwrap();
    for name in [
        "source.revoke",
        "development.probe_compose.enable",
        "development.probe_compose.disable",
    ] {
        for payload in [Value::Null, json!(["not a business DTO"])] {
            let error = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                dispatcher.submit(&admin, name, payload),
            )
            .await
            .expect("unwired refusal must not acquire the held connection")
            .unwrap_err();
            assert!(
                matches!(error, Error::Invalid(ref code) if code == "management_operation_not_wired"),
                "{name}: {error:?}"
            );
        }
    }
    session.commit().await.unwrap();
    assert_eq!(management_sql_snapshot(&path), before);
}

#[tokio::test]
async fn management_submit_distinguishes_unknown_other_plane_and_unwired() {
    let (dir, store) = store().await;
    let admin = Context::new("boundary", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let path = dir.path().join("boundary.sqlite3");
    let before = management_sql_snapshot(&path);
    for name in [
        "unknown.management",
        "source.revoke.extra",
        "",
        "evo_prepare",
        "evo_feedback",
        "evo_propose",
        "evo_inspect",
    ] {
        let error = dispatcher
            .submit(&admin, name, Value::Null)
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::Invalid(ref code) if code == "unknown_management_operation"),
            "{name}: {error:?}"
        );
    }
    for name in management_registry::formal_names()
        .iter()
        .chain(management_registry::offline_names())
    {
        let error = dispatcher
            .submit(&admin, name, Value::Null)
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::Invalid(ref code) if code == "management_operation_plane_mismatch"),
            "{name}: {error:?}"
        );
    }
    for name in [
        "source.revoke",
        "consolidation.run",
        "development.probe_compose.enable",
        "development.probe_compose.disable",
    ] {
        let error = dispatcher
            .submit(&admin, name, Value::Null)
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::Invalid(ref code) if code == "management_operation_not_wired"),
            "{name}: {error:?}"
        );
    }
    assert_eq!(management_sql_snapshot(&path), before);
}

#[tokio::test]
async fn generic_submit_rejects_dedicated_routes_without_closing_their_consumers() {
    let (dir, store) = store().await;
    let admin = Context::new("boundary", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let payload =
        json!({"schema_version":"rsia.management.meta_start.v1", "request_key":"dedicated-health"});
    let queued = dispatcher
        .submit(&admin, "meta.start", payload.clone())
        .await
        .unwrap();
    let done = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let job = dispatcher.status(&admin, &queued.id).await.unwrap();
            if job.state == evo_engine::dispatch::ManagementJobState::Blocked {
                break job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(done.step, "blocked_feature");
    assert_eq!(
        done.error_code.as_deref(),
        Some("meta.start_consumer_unavailable")
    );
    let path = dir.path().join("boundary.sqlite3");
    let before = management_sql_snapshot(&path);
    for name in ["job.status", "job.cancel", "control.review"] {
        let error = dispatcher
            .submit(&admin, name, Value::Null)
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::Invalid(ref code) if code == "management operation requires dedicated route"),
            "{name}: {error:?}"
        );
    }
    assert_eq!(
        dispatcher
            .submit(&admin, "meta.start", payload)
            .await
            .unwrap()
            .id,
        done.id
    );
    assert_eq!(
        serde_json::to_value(dispatcher.status(&admin, &done.id).await.unwrap()).unwrap(),
        serde_json::to_value(&done).unwrap()
    );
    assert_eq!(
        serde_json::to_value(dispatcher.cancel(&admin, &done.id).await.unwrap()).unwrap(),
        serde_json::to_value(&done).unwrap()
    );
    assert_eq!(management_sql_snapshot(&path), before);
}

#[tokio::test]
async fn unbound_context_stays_forbidden_before_management_classification() {
    let (dir, store) = store().await;
    let admin = Context::new("boundary", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin]).unwrap();
    let path = dir.path().join("boundary.sqlite3");
    let before = management_sql_snapshot(&path);
    for caller in [
        Context::new("other", "admin", Role::Admin).unwrap(),
        Context::new("boundary", "other", Role::Admin).unwrap(),
        Context::new("boundary", "admin", Role::Evaluator).unwrap(),
        Context::new("boundary", "agent", Role::Agent).unwrap(),
    ] {
        for name in [
            "unknown.management",
            "release.rollback",
            "source.revoke",
            "job.status",
            "meta.start",
        ] {
            assert!(matches!(
                dispatcher
                    .submit(&caller, name, Value::Null)
                    .await
                    .unwrap_err(),
                Error::Forbidden
            ));
        }
    }
    assert_eq!(management_sql_snapshot(&path), before);
}
