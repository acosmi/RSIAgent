//! Public budget transitions create all monetary/dispatch facts. Artificial
//! faults affect only dependency edges, historical refs or artifact bodies.
//! The registered S -> R edges are local closure fixtures, not an E16 learning chain.
use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallReservation,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use evo_storage::typed_budget::{
    E16_BUDGET_REF_SCHEMA, E16_BUDGET_REQUEST_SCHEMA, E16BudgetRequest, e16_budget_ref_id,
};
use serde_json::{Value, json};
use sqlx::Connection;

fn ctx(ns: &str, role: Role) -> Context {
    Context::new(
        ns,
        if role == Role::Host {
            "host"
        } else {
            "import-admin"
        },
        role,
    )
    .unwrap()
}
fn admin() -> Context {
    ctx("n", Role::Admin)
}
fn host() -> Context {
    ctx("n", Role::Host)
}
fn source(ns: &str, id: &str) -> Value {
    json!({"schema_version":"rsia.e16.import_source.v1","id":id,"namespace":ns,"owner_actor":"import-admin",
    "request_key":format!("request-{id}"),"input_digest":hash(id.as_bytes()),"created_at":1,"updated_at":1,"source_refs":[],"revoke_watermark":0,
    "payload":{"status":"prepared","raw_blob_digest":null}})
}
async fn put(store: &Store, context: &Context, kind: &str, id: &str, value: &Value) {
    let mut s = store.session().await.unwrap();
    s.put(context, kind, id, context.actor(), value)
        .await
        .unwrap();
    s.commit().await.unwrap();
}
async fn database() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    store
        .authorize_root_budget(
            &admin(),
            &RootBudgetAuthorization {
                root_budget_id: "root".into(),
                billing_scope: "scope".into(),
                allowed_namespaces: vec!["n".into(), "other".into()],
                currency: "USD".into(),
                pricing_version: "price".into(),
                payment_subject: "payer".into(),
                authorization_receipt_digest: hash(b"authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    put(
        &store,
        &admin(),
        "run",
        "R",
        &json!({"id":"R","schema_version":"test.root_run.v1"}),
    )
    .await;
    for id in ["S", "T"] {
        put(&store, &admin(), "artifact", id, &source("n", id)).await;
    }
    let mut s = store.session().await.unwrap();
    s.put_edge(&admin(), "artifact", "S", "run", "R")
        .await
        .unwrap();
    s.commit().await.unwrap();
    (dir, store)
}
fn fence(call: &BudgetCallRecord, now: i64) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    }
}
fn charge(id: &str) -> UsageCharge {
    UsageCharge {
        amount_micros: 9,
        currency: "USD".into(),
        pricing_version: "price".into(),
        provider_request_id: format!("provider-{id}"),
        usage_record_id: format!("usage-{id}"),
        output_digest: hash(b"output"),
    }
}
fn evidence() -> ModelCallSettlementEvidence {
    ModelCallSettlementEvidence {
        provenance: BudgetExecutionProvenance::Fixture,
        actual_model_digest: Some(hash(b"model")),
        transport_artifact: BudgetArtifact::from_serializable(
            "test.transport.v1",
            &json!({"content":"TRANSPORT-MARKER"}),
        )
        .unwrap(),
        usable_response: Some(
            BudgetArtifact::from_serializable(
                "test.response.v1",
                &json!({"content":"RESPONSE-MARKER"}),
            )
            .unwrap(),
        ),
        blocked_response: BudgetArtifact::from_serializable(
            "test.response.v1",
            &json!({"content":"BLOCKED-MARKER"}),
        )
        .unwrap(),
        forced_block_reason: None,
    }
}
async fn reserve(store: &Store, context: &Context, id: &str, source_id: &str) -> BudgetCallRecord {
    let refs = store
        .snapshot_e16_budget_sources(
            context,
            &[TypedObjectRef {
                kind: "artifact".into(),
                id: source_id.into(),
            }],
        )
        .await
        .unwrap();
    let input =
        BudgetArtifact::from_serializable("test.input.v1", &json!({"content":"REQUEST-MARKER"}))
            .unwrap();
    let request = E16BudgetRequest {
        schema_version: E16_BUDGET_REQUEST_SCHEMA.into(),
        namespace: context.namespace().into(),
        billing_scope: "scope".into(),
        call_id: id.into(),
        object_refs: refs,
        input_artifact: input.clone(),
    }
    .artifact()
    .unwrap();
    store
        .reserve_e16_budget_call(
            context,
            &BudgetCallReservation {
                billing_scope: "scope".into(),
                call_id: id.into(),
                dispatch_group_id: format!("group-{id}"),
                stage: BudgetStage::Reflection,
                actual_input_digest: input.digest,
                request_artifact: Some(request),
                max_cost_micros: 20,
                lease_token: format!("lease-{id}"),
                lease_until: 1000,
                now: 2,
            },
        )
        .await
        .unwrap()
}
async fn settled(store: &Store, id: &str, source_id: &str) -> BudgetCallRecord {
    let call = reserve(store, &host(), id, source_id).await;
    let call = store
        .begin_budget_dispatch(&host(), &fence(&call, 3))
        .await
        .unwrap()
        .call;
    let result = store
        .settle_model_budget_call(&host(), &fence(&call, 4), &charge(id), &evidence())
        .await
        .unwrap();
    assert!(result.response_usable);
    assert!(!result.call.execution_closed);
    result.call
}
async fn begin(store: &Store, kind: &str, id: &str) -> CleanupStatus {
    LifecycleStore::begin_revoke(
        &admin(),
        store,
        TypedObjectRef {
            kind: kind.into(),
            id: id.into(),
        },
        "test revoke",
        5,
    )
    .await
    .unwrap()
}
async fn drive(store: &Store, status: &CleanupStatus, start: i64) -> CleanupStatus {
    for now in start..start + 2000 {
        let next = LifecycleStore::cleanup_step(&admin(), store, &status.job_id, 1, now)
            .await
            .unwrap();
        if matches!(next.state, CleanupState::Complete | CleanupState::Failed) {
            return next;
        }
    }
    panic!("cleanup did not reach a terminal state")
}
fn irreversible(call: &BudgetCallRecord) -> Value {
    let mut value = serde_json::to_value(call).unwrap();
    let fields = value.as_object_mut().unwrap();
    for field in [
        "request_artifact",
        "transport_artifact",
        "response_artifact",
        "response_usable",
        "response_block_reason",
    ] {
        fields.remove(field);
    }
    value
}
fn redacted(call: &BudgetCallRecord) {
    for artifact in [
        &call.request_artifact,
        &call.transport_artifact,
        &call.response_artifact,
    ] {
        let artifact = artifact.as_ref().unwrap();
        assert_eq!(artifact.schema_version, "rsia.redacted.v1");
        for marker in [
            "REQUEST-MARKER",
            "TRANSPORT-MARKER",
            "RESPONSE-MARKER",
            "BLOCKED-MARKER",
        ] {
            assert!(!artifact.body.contains(marker));
        }
    }
    assert_eq!(call.response_usable, Some(false));
}
async fn assert_cleaned(store: &Store, call: &BudgetCallRecord, status: &CleanupStatus) {
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    let actual = store
        .budget_call(&host(), "scope", &call.call_id)
        .await
        .unwrap()
        .unwrap();
    redacted(&actual);
    assert_eq!(irreversible(&actual), irreversible(call));
    assert_eq!(
        store
            .root_budget(&admin(), "scope")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        9
    );
    for result in [
        store
            .consume_e16_budget_response(&host(), "scope", &call.call_id)
            .await
            .map(|_| ()),
        store
            .settle_model_budget_call(
                &host(),
                &fence(call, 100),
                &charge(&call.call_id),
                &evidence(),
            )
            .await
            .map(|_| ()),
    ] {
        assert!(
            matches!(result,Err(Error::Conflict(code)) if code=="e16_budget_sources_unavailable")
        );
    }
    let mut s = store.session().await.unwrap();
    let reference: Value = s
        .need(
            &admin(),
            "artifact",
            &e16_budget_ref_id("n", "scope", &call.call_id).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reference["schema_version"], E16_BUDGET_REF_SCHEMA);
    assert_eq!(reference["object_refs"][0]["id"], "S");
    s.commit().await.unwrap();
}
async fn fault_delete_ref(store: &Store, id: &str) {
    let mut s = store.session().await.unwrap();
    s.delete(
        &admin(),
        "artifact",
        &e16_budget_ref_id("n", "scope", id).unwrap(),
    )
    .await
    .unwrap();
    s.commit().await.unwrap();
}

async fn unused_id_boundary(missing: bool) {
    let (_dir, store) = database().await;
    let id = e16_budget_ref_id("n", "scope", "orphan").unwrap();
    let mut s = store.session().await.unwrap();
    if !missing {
        // No new ref or budget call was ever created for this string.
        // Its explicit old schema, not the name, determines classification.
        s.put(
            &admin(),
            "artifact",
            &id,
            admin().actor(),
            &json!({"schema_version":"rsia.budget_call_ref.v1","id":id}),
        )
        .await
        .unwrap();
    }
    s.put_edge(&admin(), "artifact", &id, "run", "R")
        .await
        .unwrap();
    s.commit().await.unwrap();
    let status = begin(&store, "run", "R").await;
    let terminal = drive(&store, &status, 6).await;
    assert!(
        store
            .budget_call(&host(), "scope", "orphan")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        terminal.state,
        CleanupState::Complete,
        "missing={missing}: {terminal:?}"
    );
    let persisted = LifecycleStore::cleanup_status(&admin(), &store, &status.job_id)
        .await
        .unwrap();
    assert_eq!(persisted.state, CleanupState::Complete);
    let mut s = store.session().await.unwrap();
    let body: Option<Value> = s.get(&admin(), "artifact", &id).await.unwrap();
    assert_eq!(
        body,
        if missing {
            None
        } else {
            Some(json!({"schema_version":"rsia.budget_call_ref.v1","id":id}))
        }
    );
    s.commit().await.unwrap();
    assert_eq!(
        store
            .root_budget(&admin(), "scope")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        0
    );
}

#[tokio::test]
async fn frontier_unused_ref_id_without_a_body_is_not_a_new_mode_call() {
    unused_id_boundary(true).await;
}

#[tokio::test]
async fn frontier_unused_ref_id_with_legacy_schema_keeps_that_explicit_schema() {
    unused_id_boundary(false).await;
}

#[tokio::test]
async fn typed_new_ref_with_valid_shape_but_no_real_call_persists_failed() {
    let (_dir, store) = database().await;
    let reference = evo_storage::typed_budget::E16BudgetCallRef {
        id: e16_budget_ref_id("n", "scope", "no-call").unwrap(),
        schema_version: E16_BUDGET_REF_SCHEMA.into(),
        namespace: "n".into(),
        billing_scope: "scope".into(),
        call_id: "no-call".into(),
        request_schema: E16_BUDGET_REQUEST_SCHEMA.into(),
        request_digest: hash(b"request never reserved"),
        actual_input_digest: hash(b"input never dispatched"),
        object_refs: store
            .snapshot_e16_budget_sources(
                &host(),
                &[TypedObjectRef {
                    kind: "artifact".into(),
                    id: "S".into(),
                }],
            )
            .await
            .unwrap(),
    };
    let mut s = store.session().await.unwrap();
    s.put(
        &admin(),
        "artifact",
        &reference.id,
        host().actor(),
        &reference,
    )
    .await
    .unwrap();
    s.put_edge(&admin(), "artifact", &reference.id, "artifact", "S")
        .await
        .unwrap();
    s.commit().await.unwrap();
    assert!(
        store
            .budget_call(&host(), "scope", "no-call")
            .await
            .unwrap()
            .is_none()
    );
    let status = begin(&store, "run", "R").await;
    let terminal = drive(&store, &status, 6).await;
    assert_eq!(terminal.state, CleanupState::Failed, "{terminal:?}");
    assert_eq!(
        LifecycleStore::cleanup_status(&admin(), &store, &status.job_id)
            .await
            .unwrap()
            .state,
        CleanupState::Failed
    );
    assert_eq!(
        store
            .root_budget(&admin(), "scope")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        0
    );
}

#[tokio::test]
async fn same_new_ref_prefix_with_a_legitimate_e16_source_uses_its_actual_schema() {
    let (_dir, store) = database().await;
    let id = e16_budget_ref_id("n", "scope", "legal-source-name").unwrap();
    put(&store, &admin(), "artifact", &id, &source("n", &id)).await;
    let mut s = store.session().await.unwrap();
    s.put_edge(&admin(), "artifact", &id, "run", "R")
        .await
        .unwrap();
    s.commit().await.unwrap();
    let status = begin(&store, "run", "R").await;
    let terminal = drive(&store, &status, 6).await;
    assert_eq!(terminal.state, CleanupState::Complete, "{terminal:?}");
    let mut s = store.session().await.unwrap();
    let body: Value = s.need(&admin(), "artifact", &id).await.unwrap();
    assert_eq!(body["schema_version"], "rsia.redacted.v1");
    s.commit().await.unwrap();
    assert_eq!(
        store
            .root_budget(&admin(), "scope")
            .await
            .unwrap()
            .unwrap()
            .spent_micros,
        0
    );
}
async fn fault_delete_edge(dir: &tempfile::TempDir, id: &str) {
    let mut db = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("rsia.sqlite3")),
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM dependencies WHERE namespace='n' AND src_kind='artifact' AND src_id=? AND dst_kind='artifact' AND dst_id='S'")
        .bind(e16_budget_ref_id("n","scope",id).unwrap()).execute(&mut db).await.unwrap();
}
async fn expanded(dir: &tempfile::TempDir, job: &str, kind: &str, id: &str) -> bool {
    let mut db = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(dir.path().join("rsia.sqlite3"))
            .read_only(true),
    )
    .await
    .unwrap();
    let state:Option<i64>=sqlx::query_scalar("SELECT expanded FROM revoke_cleanup_frontier WHERE namespace='n' AND job_id=? AND node_kind=? AND node_id=?")
        .bind(job).bind(kind).bind(id).fetch_optional(&mut db).await.unwrap();
    state == Some(1)
}

#[tokio::test]
async fn indirect_upstream_revoke_clears_all_ledger_bodies_before_preserving_the_ref() {
    let (_dir, store) = database().await;
    let call = settled(&store, "indirect", "S").await;
    let status = begin(&store, "run", "R").await;
    let complete = drive(&store, &status, 6).await;
    assert_cleaned(&store, &call, &complete).await;
    let repeated = LifecycleStore::cleanup_step(&admin(), &store, &complete.job_id, 1, 1000)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(repeated).unwrap(),
        serde_json::to_value(complete).unwrap()
    );
}

#[tokio::test]
async fn missing_historical_ref_is_rebuilt_before_the_indirect_source_filter() {
    let (_dir, store) = database().await;
    let call = settled(&store, "missing-ref", "S").await;
    fault_delete_ref(&store, &call.call_id).await;
    let status = begin(&store, "run", "R").await;
    let first = LifecycleStore::cleanup_step(&admin(), &store, &status.job_id, 1, 6)
        .await
        .unwrap();
    assert_eq!(first.state, CleanupState::Running);
    let mut s = store.session().await.unwrap();
    let repaired: Value = s
        .need(
            &admin(),
            "artifact",
            &e16_budget_ref_id("n", "scope", &call.call_id).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repaired["object_refs"][0]["owner_actor"], "import-admin");
    s.commit().await.unwrap();
    let complete = drive(&store, &first, 7).await;
    assert_cleaned(&store, &call, &complete).await;
}

#[tokio::test]
async fn missing_declared_edge_is_repaired_without_overwriting_the_historical_ref() {
    let (dir, store) = database().await;
    let call = settled(&store, "missing-edge", "S").await;
    let id = e16_budget_ref_id("n", "scope", &call.call_id).unwrap();
    let mut s = store.session().await.unwrap();
    let before: Value = s.need(&admin(), "artifact", &id).await.unwrap();
    s.commit().await.unwrap();
    fault_delete_edge(&dir, &call.call_id).await;
    let status = begin(&store, "run", "R").await;
    let complete = drive(&store, &status, 6).await;
    assert_cleaned(&store, &call, &complete).await;
    let mut s = store.session().await.unwrap();
    let after: Value = s.need(&admin(), "artifact", &id).await.unwrap();
    assert_eq!(before, after);
    s.commit().await.unwrap();
}

#[tokio::test]
async fn late_history_repair_after_s_expands_reports_progress_then_propagates_on_restart() {
    let (dir, store) = database().await;
    let call = settled(&store, "late-index", "S").await;
    let status = begin(&store, "run", "R").await;
    let first = LifecycleStore::cleanup_step(&admin(), &store, &status.job_id, 1, 6)
        .await
        .unwrap();
    assert!(expanded(&dir, &status.job_id, "__budget_scan", "R").await);
    // A real, already settled call loses its ref/index only after the initial
    // scan. No new reserve is attempted against a revoked source.
    fault_delete_ref(&store, &call.call_id).await;
    fault_delete_edge(&dir, &call.call_id).await;
    let mut repair = None;
    for now in 7..100 {
        let step = LifecycleStore::cleanup_step(&admin(), &store, &first.job_id, 1, now)
            .await
            .unwrap();
        let mut s = store.session().await.unwrap();
        let reference = s
            .get::<Value>(
                &admin(),
                "artifact",
                &e16_budget_ref_id("n", "scope", &call.call_id).unwrap(),
            )
            .await
            .unwrap();
        s.commit().await.unwrap();
        if reference.is_some() {
            repair = Some((step, now));
            break;
        }
        assert_eq!(step.state, CleanupState::Running);
    }
    let (repair, now) = repair.expect("late maintenance must restore the ref");
    assert!(expanded(&dir, &status.job_id, "artifact", "S").await);
    assert_eq!(
        repair.state,
        CleanupState::Running,
        "index insertion is real progress, not Complete"
    );
    assert_eq!(
        repair.pending_nodes, 0,
        "indirect ref is queued by the next persistent fixpoint"
    );
    let old = store
        .budget_call(&host(), "scope", &call.call_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        old.request_artifact
            .as_ref()
            .unwrap()
            .body
            .contains("REQUEST-MARKER")
    );
    drop(store);
    let reopened = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    let next = LifecycleStore::cleanup_step(&admin(), &reopened, &repair.job_id, 1, now + 1)
        .await
        .unwrap();
    assert_eq!(next.state, CleanupState::Running);
    assert!(next.pending_nodes > 0);
    let complete = drive(&reopened, &next, now + 2).await;
    assert_cleaned(&reopened, &call, &complete).await;
}

#[tokio::test]
async fn redacted_pending_and_running_calls_with_missing_or_conflicting_ref_persist_failed() {
    for running in [false, true] {
        for missing in [false, true] {
            let (dir, store) = database().await;
            let call = settled(&store, "bad-history", "S").await;
            let mut s = store.session().await.unwrap();
            s.redact_budget_call_content(
                &admin(),
                "scope",
                &call.call_id,
                "manual-history-redaction",
            )
            .await
            .unwrap();
            s.commit().await.unwrap();
            let mut status = begin(&store, "artifact", "S").await;
            if running {
                status = LifecycleStore::cleanup_step(&admin(), &store, &status.job_id, 1, 6)
                    .await
                    .unwrap();
                assert_eq!(status.state, CleanupState::Running);
            }
            if missing {
                fault_delete_ref(&store, &call.call_id).await;
            } else {
                let id = e16_budget_ref_id("n", "scope", &call.call_id).unwrap();
                let mut s = store.session().await.unwrap();
                let mut value: Value = s.need(&admin(), "artifact", &id).await.unwrap();
                value["request_digest"] = json!(hash(b"conflicting-history"));
                s.put(&admin(), "artifact", &id, admin().actor(), &value)
                    .await
                    .unwrap();
                s.commit().await.unwrap();
            }
            let failed = drive(&store, &status, 7).await;
            assert_eq!(
                failed.state,
                CleanupState::Failed,
                "running={running}, missing={missing}: {failed:?}"
            );
            assert!(failed.last_error.as_deref().unwrap().contains("e16_budget"));
            assert_eq!(
                store
                    .root_budget(&admin(), "scope")
                    .await
                    .unwrap()
                    .unwrap()
                    .spent_micros,
                9
            );
            drop(store);
            let reopened = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
            assert_eq!(
                LifecycleStore::cleanup_status(&admin(), &reopened, &failed.job_id)
                    .await
                    .unwrap()
                    .state,
                CleanupState::Failed
            );
        }
    }
}

#[tokio::test]
async fn same_id_other_namespace_and_other_kind_are_not_cleared_by_this_job() {
    let (_dir, store) = database().await;
    put(
        &store,
        &ctx("other", Role::Admin),
        "artifact",
        "S",
        &source("other", "S"),
    )
    .await;
    put(&store, &admin(), "run", "S", &json!({"id":"S"})).await;
    let other = reserve(&store, &ctx("other", Role::Host), "other-call", "S").await;
    let typed = reserve(&store, &host(), "typed-control", "T").await;
    let mut s = store.session().await.unwrap();
    let typed_ref = e16_budget_ref_id("n", "scope", &typed.call_id).unwrap();
    s.put_edge(&admin(), "artifact", &typed_ref, "run", "S")
        .await
        .unwrap();
    s.commit().await.unwrap();
    let status = begin(&store, "artifact", "S").await;
    let complete = drive(&store, &status, 6).await;
    assert_eq!(complete.state, CleanupState::Complete);
    assert_eq!(
        store
            .budget_call(&ctx("other", Role::Host), "scope", "other-call")
            .await
            .unwrap()
            .unwrap(),
        other
    );
    assert_eq!(
        store
            .budget_call(&host(), "scope", "typed-control")
            .await
            .unwrap()
            .unwrap(),
        typed
    );
    assert!(
        store
            .begin_budget_dispatch(&host(), &fence(&typed, 100))
            .await
            .unwrap()
            .new_dispatch
    );
}

#[tokio::test]
async fn a_complete_job_stays_complete_but_late_dependency_use_is_blocked_and_charged() {
    let (_dir, store) = database().await;
    let call = reserve(&store, &host(), "after-complete", "T").await;
    let dispatched = store
        .begin_budget_dispatch(&host(), &fence(&call, 3))
        .await
        .unwrap()
        .call;
    let status = begin(&store, "run", "R").await;
    let complete = drive(&store, &status, 6).await;
    assert_eq!(complete.state, CleanupState::Complete);
    let mut s = store.session().await.unwrap();
    s.put_edge(&admin(), "artifact", "T", "artifact", "S")
        .await
        .unwrap();
    s.commit().await.unwrap();
    assert!(
        matches!(store.begin_budget_dispatch(&host(),&fence(&call,100)).await,Err(Error::Conflict(code)) if code=="e16_budget_sources_unavailable")
    );
    let result = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 100),
            &charge(&call.call_id),
            &evidence(),
        )
        .await
        .unwrap();
    redacted(&result.call);
    assert_eq!(result.call.actual_cost_micros, Some(9));
    assert_eq!(result.call.dispatch_id, dispatched.dispatch_id);
    let after = LifecycleStore::cleanup_step(&admin(), &store, &complete.job_id, 1, 101)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(complete).unwrap()
    );
    assert!(
        matches!(store.consume_e16_budget_response(&host(),"scope",&call.call_id).await,Err(Error::Conflict(code)) if code=="e16_budget_sources_unavailable")
    );
}
