//! AG-184 (plan §6.1.1, §7.3.1): a budget call a crashed process left dispatched
//! is turned uncertain, with its execution closed as `process_restarted`, by the
//! real `rsia serve` startup under the data-directory lock. The call belongs to a
//! namespace the server does not serve: the startup step covers the whole data
//! directory. Only public store APIs seed and read the database.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use evo_core::{Context, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetCallState, BudgetStage,
    RootBudgetAuthorization,
};
use serde_json::json;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{Duration, timeout};

const ADMIN_TOKEN: &str = "interrupted-admin-fixture-0123456789";

#[tokio::test]
async fn serve_startup_closes_a_call_a_crashed_process_left_dispatched() {
    let root = std::env::temp_dir().join(format!(
        "rsia-interrupted-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let database = root.join("serve.sqlite3");
    let admin = Context::new("tenant", "admin", Role::Admin).unwrap();
    let host = Context::new("tenant", "broker", Role::Host).unwrap();
    let now = evo_core::now().unwrap();

    // What a crash in the middle of a model call leaves: a dispatched call whose
    // lease has not even ended.
    let store = Store::open(&database).await.unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-tenant".into(),
                billing_scope: "scope-tenant".into(),
                allowed_namespaces: vec!["tenant".into()],
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-1".into(),
                authorization_receipt_digest: hash(b"authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let reserved = store
        .reserve_budget_call(
            &host,
            &BudgetCallReservation {
                billing_scope: "scope-tenant".into(),
                call_id: "call-crashed".into(),
                dispatch_group_id: "group-crashed".into(),
                stage: BudgetStage::Reflection,
                actual_input_digest: hash(b"input-crashed"),
                request_artifact: Some(
                    BudgetArtifact::from_serializable(
                        "rsia.model_request.artifact.v1",
                        &json!({
                            "request_id": "call-crashed",
                            "source_closure": [],
                            "input": "input of call-crashed",
                        }),
                    )
                    .unwrap(),
                ),
                max_cost_micros: 20,
                lease_token: "lease-crashed".into(),
                lease_until: now + 3_600,
                now,
            },
        )
        .await
        .unwrap();
    let dispatched = store
        .begin_budget_dispatch(
            &host,
            &BudgetCallFence {
                billing_scope: "scope-tenant".into(),
                call_id: "call-crashed".into(),
                actual_input_digest: reserved.actual_input_digest.clone(),
                lease_token: reserved.lease_token.clone(),
                lease_epoch: reserved.lease_epoch,
                now,
            },
        )
        .await
        .unwrap()
        .call;
    assert_eq!(dispatched.state, BudgetCallState::Dispatched);
    assert!(!dispatched.execution_closed);
    store.close().await;

    let mut serve = Command::new(env!("CARGO_BIN_EXE_rsia"))
        .current_dir(&root)
        .env_clear()
        .env("RSIA_ADMIN_TOKEN", ADMIN_TOKEN)
        .args([
            "serve",
            "--namespace",
            "served",
            "--actor",
            "agent",
            "--bind",
            "127.0.0.1:0",
            "--data",
        ])
        .arg(&database)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(serve.stderr.take().unwrap()).lines();
    timeout(Duration::from_secs(30), async {
        loop {
            let line = lines
                .next_line()
                .await
                .unwrap()
                .expect("server exited before its listen point");
            if line.starts_with("rsia http listening on ") {
                break;
            }
        }
    })
    .await
    .unwrap();
    serve.kill().await.unwrap();
    serve.wait().await.unwrap();

    let store = Store::open(&database).await.unwrap();
    let call = store
        .budget_call(&admin, "scope-tenant", "call-crashed")
        .await
        .unwrap()
        .unwrap();
    store.close().await;
    fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        call.state,
        BudgetCallState::Uncertain,
        "PARENT-EXPECTED-FAILURE: serve startup left the crashed call {:?}",
        call.state
    );
    assert!(call.execution_closed);
    assert_eq!(
        call.execution_close_reason.as_deref(),
        Some("process_restarted")
    );
    assert_eq!(call.terminal_reason.as_deref(), Some("process_restarted"));
    assert_eq!(call.actual_cost_micros, None);
    assert_eq!(call.dispatch_id, dispatched.dispatch_id);
}
