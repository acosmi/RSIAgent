//! AG-139a two-tree probes. They use only API that predates AG-139a and read
//! every new fact as JSON strings, so they compile on the parent tree, where
//! each fails a business assertion marked `PARENT-EXPECTED-FAILURE`:
//! - CPP-A: `consolidation.trigger` was registered but not wired;
//! - CPP-B: a superseded generation that never ran stayed `claimed` (plan L851).
#[path = "support/consolidation_cycles.rs"]
mod consolidation_cycles;
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use consolidation_cycles::{CLAIM_SCHEMA, CYCLE_SCHEMA, Fixture, RUN_SCHEMA};
use evo_core::Error;
use evo_engine::dispatch::ManagementDispatcher;
use evo_engine::monitoring::ClaimOutcome;
use serde_json::{Value, json};

async fn finished(fixture: &Fixture, dispatcher: &ManagementDispatcher, job_id: &str) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        loop {
            let status = serde_json::to_value(
                dispatcher
                    .status_view(&fixture.admin, job_id)
                    .await
                    .unwrap(),
            )
            .unwrap();
            if matches!(
                status["state"].as_str(),
                Some("succeeded" | "failed" | "cancelled" | "blocked")
            ) {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("trigger job reaches a terminal state")
}

async fn submit(
    fixture: &Fixture,
    dispatcher: &ManagementDispatcher,
    key: &str,
    fact_id: &str,
) -> Value {
    let queued = match dispatcher
        .submit(
            &fixture.admin,
            "consolidation.trigger",
            fixture.payload(key, fact_id),
        )
        .await
    {
        Ok(queued) => queued,
        Err(Error::Invalid(code)) if code == "management_operation_not_wired" => panic!(
            "PARENT-EXPECTED-FAILURE: consolidation.trigger is registered but not wired ({code})"
        ),
        Err(error) => panic!("unexpected trigger refusal: {error:?}"),
    };
    finished(fixture, dispatcher, queued.identity().id).await
}

#[tokio::test]
async fn cpp_a_the_trigger_job_closes_the_cycle_and_claims_with_zero_spend() {
    let fixture = Fixture::new().await;
    let dispatcher =
        ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
    let first_fact = fixture.stage(1, false).await;
    let second_fact = fixture.stage(2, false).await;
    let first = submit(&fixture, &dispatcher, "cpp-a-1", &first_fact).await;
    assert_eq!(first["state"], json!("succeeded"), "{first}");
    assert_eq!(first["result"]["trigger_state"], json!("not_eligible"));
    let second = submit(&fixture, &dispatcher, "cpp-a-2", &second_fact).await;
    assert_eq!(second["state"], json!("succeeded"), "{second}");

    let mut cycles = fixture.artifacts(CYCLE_SCHEMA).await;
    cycles.sort_by_key(|cycle| cycle["ordinal"].as_u64());
    assert_eq!(cycles.len(), 2);
    assert_eq!(cycles[0]["report_fact_id"], json!(first_fact));
    assert_eq!(cycles[1]["report_fact_id"], json!(second_fact));
    assert_eq!(cycles[1]["ordinal"], json!(2));
    let scope_id = fixture.scope_id().await;
    let index = fixture.value(&scope_id).await;
    assert_eq!(
        index["cycle_ids"],
        json!([cycles[0]["id"].clone(), cycles[1]["id"].clone()])
    );
    let claims = fixture.artifacts(CLAIM_SCHEMA).await;
    assert_eq!(claims.len(), 1);
    let claim = &claims[0];
    assert_eq!(index["claim_ids"], json!({"1": claim["id"].clone()}));
    assert_eq!(claim["generation"], json!(1));
    assert_eq!(claim["state"], json!("no_contrast"));
    assert_eq!(claim["cycle_ids"], index["cycle_ids"]);

    let result = &second["result"];
    assert_eq!(result["result"], json!("consolidation_triggered"));
    assert_eq!(result["cycle_id"], cycles[1]["id"]);
    assert_eq!(result["scope_id"], json!(scope_id));
    assert_eq!(result["completed_cycles"], json!(2));
    assert_eq!(result["trigger_state"], json!("claimed"));
    assert_eq!(result["claim_id"], claim["id"]);
    assert_eq!(result["claim_state"], json!("no_contrast"));
    assert_eq!(result["terminated_generations"], json!([]));

    // A claim only: no run record and nothing reserved or spent.
    assert!(fixture.artifacts(RUN_SCHEMA).await.is_empty());
    assert_eq!(fixture.root().await, (0, 0));
    let job_id = second["id"].as_str().unwrap().to_owned();
    let audits = fixture.audits();
    for expected in [
        (
            "monitoring.development_cycle.complete",
            cycles[1]["id"].as_str().unwrap().to_owned(),
        ),
        (
            "monitoring.consolidation.claim",
            claim["id"].as_str().unwrap().to_owned(),
        ),
        ("management.job.finish", job_id),
    ] {
        assert!(
            audits.contains(&(expected.0.to_owned(), expected.1.clone())),
            "{expected:?} in {audits:?}"
        );
    }
}

#[tokio::test]
async fn cpp_b_a_superseded_generation_that_never_ran_is_expired() {
    let fixture = Fixture::new().await;
    fixture.close(1, true).await;
    fixture.close(2, true).await;
    let scope_id = fixture.scope_id().await;
    assert!(matches!(
        fixture.claim(&scope_id).await.unwrap(),
        ClaimOutcome::Claimed(claim) if claim.generation == 1
    ));
    fixture.close(3, true).await;
    fixture.close(4, true).await;
    assert!(matches!(
        fixture.claim(&scope_id).await.unwrap(),
        ClaimOutcome::Claimed(claim) if claim.generation == 2
    ));
    let first = fixture.value(&fixture.claim_id(1).await).await;
    let second = fixture.value(&fixture.claim_id(2).await).await;
    if first["state"] == json!("claimed") {
        panic!(
            "PARENT-EXPECTED-FAILURE: generation one never ran and stays claimed after generation two"
        );
    }
    assert_eq!(first["state"], json!("generation_expired"));
    assert_eq!(second["state"], json!("claimed"));
}
