//! AG-132 (v4.5 §7.3.1 L1150–1152, V110.d L2470, Q-41/Q-54): the clock floor is
//! the maximum of the registered occurrence-time entries only. `Store::open`
//! reads it before mkdir, staging cleanup or migration, and again after
//! migration:
//!
//! * `_sqlx_migrations.installed_on` after the clock refuses `clock_regressed`
//!   and writes nothing (T1);
//! * only `execution_time`, lease or deadline fields after the clock do not
//!   refuse (T2);
//! * an unparsable, pre-epoch or out-of-range registered value refuses
//!   `occurrence_time_column_invalid` instead of counting as 0 (T3);
//! * unregistered columns do not move the floor (T4).
//!
//! A clock behind a floor already admitted at run time is the
//! `a_sample_behind_the_floor_refuses_and_keeps_the_floor` unit test in
//! `evo_core::clock`: simulating it here would need a fixed clock.

use evo_core::Error;
use evo_storage::Store;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{ConnectOptions, Connection};
use std::path::{Path, PathBuf};

/// Far after any test run, still inside the millisecond range.
const FUTURE_SECONDS: i64 = 4_000_000_000;

async fn created(dir: &Path) -> PathBuf {
    let path = dir.join("rsia.sqlite3");
    Store::open(&path).await.unwrap().close().await;
    path
}

async fn raw(path: &Path) -> SqliteConnection {
    SqliteConnectOptions::new()
        .filename(path)
        .foreign_keys(false)
        .connect()
        .await
        .unwrap()
}

async fn execute(path: &Path, statements: &[&str]) {
    let mut conn = raw(path).await;
    for statement in statements {
        sqlx::query(statement).execute(&mut conn).await.unwrap();
    }
    conn.close().await.unwrap();
}

fn staging_orphan(dir: &Path) -> PathBuf {
    let namespace = dir.join(".blob-staging").join("ns");
    std::fs::create_dir_all(&namespace).unwrap();
    let orphan = namespace.join("orphan");
    std::fs::write(&orphan, b"left by a crash").unwrap();
    orphan
}

#[tokio::test]
async fn t1_installed_on_after_the_clock_refuses_open_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = created(dir.path()).await;
    execute(
        &path,
        &["UPDATE _sqlx_migrations SET installed_on='9999-12-31 23:59:59' WHERE version=1"],
    )
    .await;
    let orphan = staging_orphan(dir.path());

    let refused = Store::open(&path).await.err().unwrap();
    assert!(matches!(refused, Error::ClockRegressed), "{refused:?}");
    assert_eq!(refused.to_string(), "clock_regressed");
    assert!(
        orphan.exists(),
        "staging cleanup ran before the floor check"
    );
    let mut conn = raw(&path).await;
    let installed: String =
        sqlx::query_scalar("SELECT installed_on FROM _sqlx_migrations WHERE version=1")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(installed, "9999-12-31 23:59:59");
}

#[tokio::test]
async fn t2_only_durations_leases_and_deadlines_after_the_clock_do_not_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let path = created(dir.path()).await;
    let job = serde_json::json!({
        "id": "job-1", "owner": "agent", "run_id": "run-1", "state": "running",
        "lease_token": "lease", "lease_until": FUTURE_SECONDS, "deadline": FUTURE_SECONDS,
        "attempts": 1, "improver_version": "v", "task_snapshot": "s", "result_ids": [],
        "error_code": null
    });
    let management_job = serde_json::json!({
        "id": "mj-1", "schema_version": "rsia.management_job.v2",
        "lease_until": FUTURE_SECONDS, "created_at": 1
    });
    execute(
        &path,
        &[
            &format!("UPDATE _sqlx_migrations SET execution_time={}", i64::MAX),
            "INSERT INTO root_budgets(billing_scope,root_budget_id,authorizing_namespace,currency,
                 pricing_version,payment_subject,authorization_receipt_digest,per_call_cap_micros,
                 total_limit_micros,created_at)
             VALUES('scope','root','ns','USD','p','payer','d',1,1,1)",
            "INSERT INTO root_budget_dispatch_groups(billing_scope,dispatch_group_id,owner_namespace,created_at)
             VALUES('scope','group','ns',1)",
            &format!(
                "INSERT INTO root_budget_calls(billing_scope,call_id,dispatch_group_id,namespace,stage,
                     actual_input_digest,reserved_micros,state,lease_token,lease_epoch,lease_until,created_at)
                 VALUES('scope','call','group','ns','stage','d',1,'reserved','lease',1,{FUTURE_SECONDS},1)"
            ),
            &format!("INSERT INTO objects(namespace,kind,id,owner,body) VALUES('ns','job','job-1','agent','{job}')"),
            &format!(
                "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('ns','job','mj-1','admin','{management_job}')"
            ),
        ],
    )
    .await;

    Store::open(&path).await.unwrap().close().await;
}

#[tokio::test]
async fn t3_invalid_registered_values_refuse_with_the_fixed_code_instead_of_zero() {
    let tombstone = |created_at: &str| {
        format!(
            "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('ns','tombstone','t-1','admin',
             '{{\"id\":\"t-1\",\"schema_version\":\"rsia.revoke_tombstone.v1\",\"created_at\":{created_at}}}')"
        )
    };
    let cases = [
        ("installed_on unparsable", "UPDATE _sqlx_migrations SET installed_on='not-a-clock'".to_owned()),
        ("installed_on impossible date", "UPDATE _sqlx_migrations SET installed_on='2026-02-30 00:00:00'".to_owned()),
        ("installed_on before the epoch", "UPDATE _sqlx_migrations SET installed_on='1969-12-31 23:59:59'".to_owned()),
        ("installed_on an integer", "UPDATE _sqlx_migrations SET installed_on=1".to_owned()),
        (
            "audit timestamp negative",
            "INSERT INTO audit(namespace,payload,previous_hash,digest) VALUES('ns','{\"timestamp\":-1}','','d')".to_owned(),
        ),
        (
            "audit timestamp absent",
            "INSERT INTO audit(namespace,payload,previous_hash,digest) VALUES('ns','{}','','d')".to_owned(),
        ),
        ("object created_at text", tombstone("\"x\"")),
        ("object created_at fractional", tombstone("1.5")),
        ("object created_at null", tombstone("null")),
        ("object created_at milliseconds overflow", tombstone(&i64::MAX.to_string())),
        (
            "management job created_at milliseconds overflow",
            format!(
                "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('ns','job','mj-1','admin',
                 '{{\"id\":\"mj-1\",\"schema_version\":\"rsia.management_job.v1\",\"created_at\":{}}}')",
                i64::MAX / 1000 + 1
            ),
        ),
        (
            "column text",
            "INSERT INTO revoke_cleanup_jobs(namespace,job_id,source_kind,source_id,watermark_seq,
                 watermark_digest,state,created_at,updated_at)
             VALUES('ns','job','run','r',1,'d','pending',1,'x')"
                .to_owned(),
        ),
    ];
    for (case, statement) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = created(dir.path()).await;
        execute(&path, &[&statement]).await;
        let orphan = staging_orphan(dir.path());

        let refused = Store::open(&path).await.err().unwrap();
        assert!(
            matches!(refused, Error::OccurrenceTimeColumnInvalid),
            "{case}: {refused:?}"
        );
        assert_eq!(
            refused.to_string(),
            "occurrence_time_column_invalid",
            "{case}"
        );
        assert!(
            orphan.exists(),
            "{case}: staging cleanup ran before the check"
        );
    }
}

#[tokio::test]
async fn t4_unregistered_columns_do_not_move_the_floor() {
    let dir = tempfile::tempdir().unwrap();
    let path = created(dir.path()).await;
    let unregistered = serde_json::json!({
        "id": "a-1", "schema_version": "rsia.not_registered.v1", "created_at": FUTURE_SECONDS
    });
    execute(
        &path,
        &[
            &format!("INSERT INTO revoke_watermark(namespace,seq,digest) VALUES('ns',{},'d')", i64::MAX),
            &format!(
                "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('ns','artifact','a-1','agent','{unregistered}')"
            ),
            &format!(
                "INSERT INTO idempotency(namespace,actor,operation,request_key,payload_hash,subject_id,response)
                 VALUES('ns','agent','op','key','h','a-1','{{\"created_at\":{FUTURE_SECONDS}}}')"
            ),
            // A registered schema's unregistered field does not count either.
            &format!(
                "INSERT INTO objects(namespace,kind,id,owner,body) VALUES('ns','tombstone','t-1','admin',
                 '{{\"id\":\"t-1\",\"schema_version\":\"rsia.revoke_tombstone.v1\",\"created_at\":1,\"other_at\":{FUTURE_SECONDS}}}')"
            ),
        ],
    )
    .await;

    Store::open(&path).await.unwrap().close().await;
}
