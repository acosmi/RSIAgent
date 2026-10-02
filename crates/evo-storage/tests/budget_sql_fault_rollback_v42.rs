//! A statement-error fixture in an owned database, using the existing budget API.
//! This covers root/member/event rollback and normal reopen, not disk or commit faults.

use evo_core::{Context, Error, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetCallState, BudgetStage,
    RootBudgetAuthorization, RootBudgetRecord, UsageCharge,
};
use serde_json::{Value, json};
use sqlx::{ConnectOptions, Connection, SqliteConnection};
use std::collections::BTreeMap;
use std::path::Path;

const TRIGGER_NAME: &str = "ag082_abort_root_authorized";
const FAULT_MARKER: &str = "AG082_TEST_AFTER_ROOT_AND_ALL_MEMBERS";
const CONTROL_SCOPE: &str = "ag082-control-scope";

type BudgetSnapshot = BTreeMap<&'static str, Vec<String>>;

// Every column from migration 0005, including nullable call artifacts and event details.
const BUDGET_PROJECTIONS: &[(&str, &str, &str)] = &[
    (
        "root_budgets",
        "billing_scope,root_budget_id,authorizing_namespace,currency,pricing_version,\
         payment_subject,authorization_receipt_digest,per_call_cap_micros,total_limit_micros,\
         spent_micros,reserved_micros,concurrency_limit,stopped,stop_reason,stop_committed_at,created_at",
        "billing_scope",
    ),
    (
        "root_budget_namespaces",
        "billing_scope,namespace",
        "billing_scope,namespace",
    ),
    (
        "root_budget_dispatch_groups",
        "billing_scope,dispatch_group_id,owner_namespace,stopped,stop_reason,stop_committed_at,created_at",
        "billing_scope,dispatch_group_id",
    ),
    (
        "root_budget_calls",
        "billing_scope,call_id,dispatch_group_id,namespace,stage,actual_input_digest,\
         request_artifact_schema,request_artifact_digest,request_artifact_body,reserved_micros,\
         state,lease_token,lease_epoch,lease_until,dispatch_id,provider_request_id,usage_record_id,\
         output_digest,actual_cost_micros,actual_currency,actual_pricing_version,execution_provenance,\
         actual_model_digest,transport_artifact_schema,transport_artifact_digest,transport_artifact_body,\
         response_artifact_schema,response_artifact_digest,response_artifact_body,response_usable,\
         response_block_reason,execution_closed,execution_closed_at,execution_close_reason,\
         terminal_reason,created_at,dispatched_at,finalized_at",
        "billing_scope,call_id",
    ),
    (
        "root_budget_events",
        "seq,billing_scope,call_id,event_kind,event_at,details",
        "seq",
    ),
];

fn ctx(namespace: &str, actor: &str, role: Role) -> Context {
    Context::new(namespace, actor, role).unwrap()
}

fn authorization(root: &str, scope: &str, namespaces: &[&str]) -> RootBudgetAuthorization {
    RootBudgetAuthorization {
        root_budget_id: root.into(),
        billing_scope: scope.into(),
        allowed_namespaces: namespaces
            .iter()
            .map(|namespace| (*namespace).into())
            .collect(),
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        payment_subject: "ag082-payer".into(),
        authorization_receipt_digest: hash(format!("ag082-admin-authorization-{root}").as_bytes()),
        per_call_cap_micros: 100,
        total_limit_micros: 1_000,
        created_at: 41,
    }
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

async fn control_ledger(store: &Store) -> (Context, RootBudgetRecord) {
    let admin = ctx("control-a", "ag082-control-admin", Role::Admin);
    let worker = ctx("control-a", "ag082-control-worker", Role::Worker);
    store
        .authorize_root_budget(
            &admin,
            &authorization(
                "ag082-control-root",
                CONTROL_SCOPE,
                &["control-b", "control-a"],
            ),
        )
        .await
        .unwrap();
    let call = store
        .reserve_budget_call(
            &worker,
            &BudgetCallReservation {
                billing_scope: CONTROL_SCOPE.into(),
                call_id: "ag082-control-call".into(),
                dispatch_group_id: "ag082-control-group".into(),
                stage: BudgetStage::Reflection,
                actual_input_digest: hash(b"ag082-control-input"),
                request_artifact: None,
                max_cost_micros: 30,
                lease_token: "ag082-control-lease".into(),
                lease_until: 200,
                now: 42,
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .begin_budget_dispatch(&worker, &fence(&call, 43))
            .await
            .unwrap()
            .new_dispatch
    );
    // This is the existing public fixture settlement API, with no provider call.
    let finalized = store
        .finalize_budget_call(
            &worker,
            &fence(&call, 44),
            &UsageCharge {
                amount_micros: 17,
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                provider_request_id: "ag082-fixture-provider-request".into(),
                usage_record_id: "ag082-fixture-usage".into(),
                output_digest: hash(b"ag082-fixture-output"),
            },
        )
        .await
        .unwrap();
    assert_eq!(finalized.state, BudgetCallState::Finalized);
    let closed = store
        .close_budget_call_execution(
            &worker,
            CONTROL_SCOPE,
            &finalized.call_id,
            finalized.dispatch_id.as_deref().unwrap(),
            "response_complete",
            45,
        )
        .await
        .unwrap();
    assert!(closed.execution_closed);
    let root = store
        .root_budget(&admin, CONTROL_SCOPE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root.allowed_namespaces, vec!["control-a", "control-b"]);
    assert_eq!((root.spent_micros, root.reserved_micros), (17, 0));
    (admin, root)
}

async fn observer(path: &Path) -> SqliteConnection {
    sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .disable_statement_logging()
        .connect()
        .await
        .unwrap()
}

async fn budget_snapshot(path: &Path, scope: Option<&str>) -> BudgetSnapshot {
    // Call only after the product future completes. Each observation opens and closes
    // its own read-only connection; no old read transaction spans a product write.
    let mut connection = observer(path).await;
    let actual_tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_schema WHERE type='table' \
         AND (name='root_budgets' OR name LIKE 'root_budget_%') ORDER BY name",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    let mut expected_tables: Vec<String> = BUDGET_PROJECTIONS
        .iter()
        .map(|(table, _, _)| (*table).into())
        .collect();
    expected_tables.sort();
    assert_eq!(actual_tables, expected_tables);
    let mut snapshot = BudgetSnapshot::new();
    for &(table, columns, order) in BUDGET_PROJECTIONS {
        let filter = if scope.is_some() {
            " WHERE billing_scope=?"
        } else {
            ""
        };
        let query = format!("SELECT json_array({columns}) FROM {table}{filter} ORDER BY {order}");
        let rows = if let Some(scope) = scope {
            sqlx::query_scalar::<_, String>(&query)
                .bind(scope)
                .fetch_all(&mut connection)
                .await
                .unwrap()
        } else {
            sqlx::query_scalar::<_, String>(&query)
                .fetch_all(&mut connection)
                .await
                .unwrap()
        };
        snapshot.insert(table, rows);
    }
    connection.close().await.unwrap();
    snapshot
}

fn assert_scope_empty(snapshot: &BudgetSnapshot) {
    for (table, rows) in snapshot {
        assert!(
            rows.is_empty(),
            "fault scope left rows in {table}: {rows:?}"
        );
    }
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn fault_trigger_sql(admin: &Context, request: &RootBudgetAuthorization) -> String {
    let members = request
        .allowed_namespaces
        .iter()
        .map(|namespace| {
            format!(
                "AND EXISTS (SELECT 1 FROM root_budget_namespaces \
                 WHERE billing_scope=NEW.billing_scope AND namespace={})",
                sql_literal(namespace)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "CREATE TRIGGER {TRIGGER_NAME}\n\
         BEFORE INSERT ON root_budget_events\n\
         WHEN NEW.billing_scope={scope} AND NEW.event_kind='root_authorized'\n\
         AND NEW.call_id IS NULL AND NEW.event_at={created_at}\n\
         AND EXISTS (SELECT 1 FROM root_budgets\n\
          WHERE billing_scope=NEW.billing_scope AND root_budget_id={root}\n\
          AND authorizing_namespace={namespace} AND currency={currency}\n\
          AND pricing_version={pricing_version} AND payment_subject={payment_subject}\n\
          AND authorization_receipt_digest={receipt}\n\
          AND per_call_cap_micros={per_call_cap} AND total_limit_micros={total_limit}\n\
          AND spent_micros=0 AND reserved_micros=0 AND concurrency_limit=1\n\
          AND stopped=0 AND stop_reason IS NULL AND stop_committed_at IS NULL\n\
          AND created_at={created_at})\n\
         AND (SELECT COUNT(*) FROM root_budget_namespaces\n\
              WHERE billing_scope=NEW.billing_scope)={member_count}\n\
         {members}\n\
         BEGIN SELECT RAISE(ABORT, '{FAULT_MARKER}'); END;",
        scope = sql_literal(&request.billing_scope),
        root = sql_literal(&request.root_budget_id),
        namespace = sql_literal(admin.namespace()),
        currency = sql_literal(&request.currency),
        pricing_version = sql_literal(&request.pricing_version),
        payment_subject = sql_literal(&request.payment_subject),
        receipt = sql_literal(&request.authorization_receipt_digest),
        per_call_cap = request.per_call_cap_micros,
        total_limit = request.total_limit_micros,
        created_at = request.created_at,
        member_count = request.allowed_namespaces.len(),
    )
}

async fn install_trigger(path: &Path, sql: &str) {
    // Store must be closed. This is an ordinary trigger in this owned database,
    // so it also applies to the separately reopened Store connection.
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .disable_statement_logging()
        .connect()
        .await
        .unwrap();
    let created = sqlx::query(sql).execute(&mut connection).await.unwrap();
    let stored_sql: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='trigger' AND name=?")
            .bind(TRIGGER_NAME)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(
        stored_sql.trim().trim_end_matches(';'),
        sql.trim().trim_end_matches(';')
    );
    eprintln!(
        "AG082 trigger created: rows_affected={}\n{stored_sql}",
        created.rows_affected()
    );
    connection.close().await.unwrap();
}

async fn remove_trigger(path: &Path) {
    // Store must again be closed; no raw SQL creates budget facts or fees.
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .disable_statement_logging()
        .connect()
        .await
        .unwrap();
    sqlx::query(&format!("DROP TRIGGER {TRIGGER_NAME}"))
        .execute(&mut connection)
        .await
        .unwrap();
    let remaining: Option<String> =
        sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='trigger' AND name=?")
            .bind(TRIGGER_NAME)
            .fetch_optional(&mut connection)
            .await
            .unwrap();
    assert!(remaining.is_none());
    eprintln!("AG082 trigger removed");
    connection.close().await.unwrap();
}

async fn assert_exact_authorization(
    store: &Store,
    path: &Path,
    admin: &Context,
    request: &RootBudgetAuthorization,
) -> RootBudgetRecord {
    let mut namespaces = request.allowed_namespaces.clone();
    namespaces.sort();
    let root = store
        .root_budget(admin, &request.billing_scope)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        root,
        RootBudgetRecord {
            root_budget_id: request.root_budget_id.clone(),
            billing_scope: request.billing_scope.clone(),
            authorizing_namespace: admin.namespace().into(),
            allowed_namespaces: namespaces.clone(),
            currency: request.currency.clone(),
            pricing_version: request.pricing_version.clone(),
            payment_subject: request.payment_subject.clone(),
            authorization_receipt_digest: request.authorization_receipt_digest.clone(),
            per_call_cap_micros: request.per_call_cap_micros,
            total_limit_micros: request.total_limit_micros,
            spent_micros: 0,
            reserved_micros: 0,
            stopped: false,
            stop_reason: None,
            stop_committed_at: None,
            created_at: request.created_at,
        }
    );
    let snapshot = budget_snapshot(path, Some(&request.billing_scope)).await;
    assert_eq!(
        snapshot["root_budgets"],
        vec![
            json!([
                request.billing_scope,
                request.root_budget_id,
                admin.namespace(),
                request.currency,
                request.pricing_version,
                request.payment_subject,
                request.authorization_receipt_digest,
                request.per_call_cap_micros,
                request.total_limit_micros,
                0,
                0,
                1,
                0,
                null,
                null,
                request.created_at
            ])
            .to_string()
        ]
    );
    let expected_members: Vec<String> = namespaces
        .iter()
        .map(|namespace| json!([request.billing_scope, namespace]).to_string())
        .collect();
    assert_eq!(snapshot["root_budget_namespaces"], expected_members);
    assert!(snapshot["root_budget_calls"].is_empty());
    assert!(snapshot["root_budget_dispatch_groups"].is_empty());
    assert_eq!(snapshot["root_budget_events"].len(), 1);
    let event: Vec<Value> = serde_json::from_str(&snapshot["root_budget_events"][0]).unwrap();
    assert_eq!(event.len(), 6);
    assert!(event[0].as_i64().unwrap() > 0);
    assert_eq!(event[1], json!(request.billing_scope));
    assert_eq!(event[2], Value::Null);
    assert_eq!(event[3], json!("root_authorized"));
    assert_eq!(event[4], json!(request.created_at));
    let details: Value = serde_json::from_str(event[5].as_str().unwrap()).unwrap();
    assert_eq!(
        details,
        json!({
            "root_budget_id": request.root_budget_id,
            "currency": request.currency,
            "pricing_version": request.pricing_version,
            "payment_subject": request.payment_subject,
            "authorization_receipt_digest": request.authorization_receipt_digest,
            "per_call_cap_micros": request.per_call_cap_micros,
            "total_limit_micros": request.total_limit_micros,
            "allowed_namespaces": namespaces,
        })
    );
    root
}

async fn exercise_rollback(namespaces: &[&str]) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ag082.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let (control_admin, control_root) = control_ledger(&store).await;
    let control_before = budget_snapshot(&path, Some(CONTROL_SCOPE)).await;
    assert_eq!(control_before["root_budgets"].len(), 1);
    assert_eq!(control_before["root_budget_namespaces"].len(), 2);
    assert_eq!(control_before["root_budget_calls"].len(), 1);
    assert_eq!(control_before["root_budget_dispatch_groups"].len(), 1);
    assert!(!control_before["root_budget_events"].is_empty());
    let before_trigger = budget_snapshot(&path, None).await;
    eprintln!("AG082 control snapshot before injection: {control_before:?}");

    let admin = ctx("fault-a", "ag082-fault-admin", Role::Admin);
    let request = authorization("ag082-fault-root", "ag082-fault-scope", namespaces);
    let trigger = fault_trigger_sql(&admin, &request);
    store.close().await;
    install_trigger(&path, &trigger).await;
    let store = Store::open(&path).await.unwrap();
    assert_eq!(budget_snapshot(&path, None).await, before_trigger);

    // A fresh, nonmatching authorization exercises the event INSERT while the
    // trigger exists. Reauthorizing the earlier control would skip that INSERT.
    let bypass_admin = ctx("bypass-a", "ag082-bypass-admin", Role::Admin);
    let bypass_request = authorization(
        "ag082-bypass-root",
        "ag082-bypass-scope",
        &["bypass-b", "bypass-a"],
    );
    store
        .authorize_root_budget(&bypass_admin, &bypass_request)
        .await
        .unwrap();
    assert_exact_authorization(&store, &path, &bypass_admin, &bypass_request).await;
    assert_eq!(
        budget_snapshot(&path, Some(CONTROL_SCOPE)).await,
        control_before
    );
    let baseline = budget_snapshot(&path, None).await;
    eprintln!("AG082 full baseline with nonmatching-scope control: {baseline:?}");

    // Duplicate namespaces are an input refusal, explicitly outside the two
    // legitimate SQL-fault attempts below.
    let mut duplicate_namespace = request.clone();
    duplicate_namespace
        .allowed_namespaces
        .push(namespaces[0].into());
    assert!(matches!(
        store.authorize_root_budget(&admin, &duplicate_namespace).await,
        Err(Error::Invalid(reason)) if reason == "duplicate allowed namespace"
    ));
    assert_eq!(budget_snapshot(&path, None).await, baseline);

    for attempt in 1..=2 {
        let error = store
            .authorize_root_budget(&admin, &request)
            .await
            .unwrap_err();
        assert!(matches!(&error, Error::Internal));
        assert!(!error.to_string().contains(FAULT_MARKER));
        // This awaits a new public pool operation after the failed transaction
        // was dropped; external observers are opened only after it completes.
        assert!(
            store
                .root_budget(&admin, &request.billing_scope)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .root_budget(&control_admin, CONTROL_SCOPE)
                .await
                .unwrap(),
            Some(control_root.clone())
        );
        assert_scope_empty(&budget_snapshot(&path, Some(&request.billing_scope)).await);
        assert_eq!(
            budget_snapshot(&path, Some(CONTROL_SCOPE)).await,
            control_before
        );
        assert_eq!(budget_snapshot(&path, None).await, baseline);
        eprintln!(
            "AG082 legitimate SQL-fault attempt {attempt}: {error}; all five tables unchanged"
        );
    }

    store.close().await;
    let store = Store::open(&path).await.unwrap();
    assert!(
        store
            .root_budget(&admin, &request.billing_scope)
            .await
            .unwrap()
            .is_none()
    );
    assert_scope_empty(&budget_snapshot(&path, Some(&request.billing_scope)).await);
    assert_eq!(budget_snapshot(&path, None).await, baseline);
    assert_eq!(store.integrity().await.unwrap(), "ok");
    eprintln!("AG082 normal reopen: fault scope empty, baseline unchanged, integrity=ok");

    store.close().await;
    remove_trigger(&path).await;
    let store = Store::open(&path).await.unwrap();
    // The identical request retains its original root ID and authorization
    // identity. Success cannot be rescued by switching to a fresh identity.
    let recovered = store.authorize_root_budget(&admin, &request).await.unwrap();
    assert_eq!(
        assert_exact_authorization(&store, &path, &admin, &request).await,
        recovered
    );
    assert_eq!(
        budget_snapshot(&path, Some(CONTROL_SCOPE)).await,
        control_before
    );
    let after_recovery = budget_snapshot(&path, None).await;
    eprintln!("AG082 identical-request recovery snapshot: {after_recovery:?}");

    let duplicate = store.authorize_root_budget(&admin, &request).await.unwrap();
    assert_eq!(duplicate, recovered);
    assert_eq!(budget_snapshot(&path, None).await, after_recovery);
    let mut conflicting = request.clone();
    conflicting.authorization_receipt_digest = hash(b"ag082-different-authorization");
    assert!(matches!(
        store.authorize_root_budget(&admin, &conflicting).await,
        Err(Error::Conflict(reason))
            if reason == "billing_scope_already_bound_to_different_root_or_authorization"
    ));
    assert_eq!(
        store
            .root_budget(&admin, &request.billing_scope)
            .await
            .unwrap(),
        Some(recovered.clone())
    );
    assert_eq!(budget_snapshot(&path, None).await, after_recovery);
    assert_eq!(
        store
            .root_budget(&control_admin, CONTROL_SCOPE)
            .await
            .unwrap(),
        Some(control_root)
    );
    store.close().await;

    let store = Store::open(&path).await.unwrap();
    assert_eq!(
        assert_exact_authorization(&store, &path, &admin, &request).await,
        recovered
    );
    assert_eq!(budget_snapshot(&path, None).await, after_recovery);
    assert_eq!(store.integrity().await.unwrap(), "ok");
    eprintln!(
        "AG082 recovery persisted; duplicate unchanged; original authorization conflict preserved"
    );
    store.close().await;
}

#[tokio::test]
async fn single_namespace_event_write_failure_rolls_back_and_same_request_recovers() {
    exercise_rollback(&["fault-a"]).await;
}

#[tokio::test]
async fn unordered_distinct_namespaces_event_write_failure_rolls_back_and_same_request_recovers() {
    exercise_rollback(&["fault-c", "fault-a", "fault-b"]).await;
}
