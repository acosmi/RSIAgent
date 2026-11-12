//! AG-139a (plan v4.5 L851, V100.b): no consolidation generation is skipped
//! silently. Claiming generation N writes every older generation that has no
//! claim yet as terminal (`no_contrast` without a contrast, `generation_expired`
//! with one) and expires an older `claimed` generation that never ran; running,
//! blocked and completed generations, and older `no_contrast` ones, keep their
//! exact bytes. The whole batch is validated and admitted against the claim
//! capacity before its first write. The refusal of `run_consolidation` on an
//! expired claim is pinned in `monitoring_trusted_cycle_v42.rs`, which holds
//! the optimization step material.
#[path = "support/consolidation_cycles.rs"]
mod consolidation_cycles;
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use consolidation_cycles::{CLAIM_SCHEMA, Fixture, RUN_SCHEMA};
use evo_core::capacity::{CapacityConflict, CapacityItem};
use evo_core::{Error, fingerprint};
use evo_engine::monitoring::{
    ClaimOutcome, ConsolidationClaim, ConsolidationClaimState, MonitoringCoordinator,
};
use serde_json::json;

async fn claimed(fixture: &Fixture, scope_id: &str) -> ConsolidationClaim {
    match fixture.claim(scope_id).await.unwrap() {
        ClaimOutcome::Claimed(claim) => claim,
        other => panic!("expected a new claim, got {other:?}"),
    }
}

async fn stored_claim(fixture: &Fixture, generation: u64) -> ConsolidationClaim {
    serde_json::from_value(fixture.value(&fixture.claim_id(generation).await).await).unwrap()
}

/// Overwrites a stored claim's state, as a run in progress or a zero-budget
/// run leaves it.
async fn set_claim_state(fixture: &Fixture, claim: &ConsolidationClaim, state: &str) {
    let mut value = serde_json::to_value(claim).unwrap();
    value["state"] = json!(state);
    value["execution_input_digest"] = json!(consolidation_cycles::d("execution-input"));
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.worker,
            "artifact",
            &claim.id,
            fixture.worker.actor(),
            &value,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

#[tokio::test]
async fn four_cycles_closed_without_a_claim_leave_no_generation_gap() {
    let fixture = Fixture::new().await;
    let mut cycles = Vec::new();
    for (cycle, contrast) in [(1, true), (2, false), (3, false), (4, false)] {
        cycles.push(fixture.close(cycle, contrast).await);
    }
    let scope_id = fixture.scope_id().await;
    let newest = claimed(&fixture, &scope_id).await;
    assert_eq!(newest.generation, 2);
    assert_eq!(newest.state, ConsolidationClaimState::NoContrast);

    let index = fixture.value(&scope_id).await;
    assert_eq!(
        index["claim_ids"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["1", "2"]
    );
    let first = stored_claim(&fixture, 1).await;
    // The gap generation is a full claim.v1 record with the derived id, its own
    // two cycles and sources; its contrast makes it expired, not dispatchable.
    assert_eq!(
        first.id,
        format!(
            "consolidation-claim-{}",
            &fingerprint(&(scope_id.as_str(), 1_u64)).unwrap()[..32]
        )
    );
    assert_eq!(first.schema_version, CLAIM_SCHEMA);
    assert_eq!(first.generation, 1);
    assert_eq!(
        first.cycle_ids,
        vec![cycles[0].id.clone(), cycles[1].id.clone()]
    );
    assert_eq!(first.sources, newest.sources);
    assert_eq!(first.revoke_watermark, 1);
    assert_eq!(first.state, ConsolidationClaimState::GenerationExpired);
    assert_eq!(first.execution_input_digest, None);
    assert_eq!(
        fixture.value(&first.id).await["state"],
        json!("generation_expired")
    );
    // Its edges reach both cycles and every source, like any claim.
    let mut session = fixture.store.session().await.unwrap();
    for (kind, id) in [
        ("artifact", cycles[0].id.as_str()),
        ("artifact", cycles[1].id.as_str()),
        ("run", "source-1"),
        ("run", "source-2"),
    ] {
        let dependents = session.dependents(&fixture.admin, kind, id).await.unwrap();
        assert!(
            dependents.contains(&("artifact".to_owned(), first.id.clone())),
            "{kind} {id}: {dependents:?}"
        );
    }
    session.commit().await.unwrap();
    let audits = fixture.audits();
    assert!(audits.contains(&(
        "monitoring.consolidation.expire".to_owned(),
        first.id.clone()
    )));
    assert!(audits.contains(&(
        "monitoring.consolidation.claim".to_owned(),
        newest.id.clone()
    )));
    // Neither claim ran: no run record exists.
    assert!(fixture.artifacts(RUN_SCHEMA).await.is_empty());

    // The same claim again is AlreadyClaimed and writes nothing.
    let rows = fixture.rows().await;
    assert!(matches!(
        fixture.claim(&scope_id).await.unwrap(),
        ClaimOutcome::AlreadyClaimed(claim) if claim.id == newest.id
    ));
    assert_eq!(fixture.rows().await, rows);
}

#[tokio::test]
async fn an_older_gap_generation_without_contrast_is_written_no_contrast() {
    let fixture = Fixture::new().await;
    for cycle in 1..=4 {
        fixture.close(cycle, cycle > 2).await;
    }
    let scope_id = fixture.scope_id().await;
    let newest = claimed(&fixture, &scope_id).await;
    assert_eq!(
        (newest.generation, newest.state),
        (2, ConsolidationClaimState::Claimed)
    );
    let first = stored_claim(&fixture, 1).await;
    assert_eq!(first.state, ConsolidationClaimState::NoContrast);
    // `no_contrast` is terminal by itself; finishing it stays optional.
    let record =
        MonitoringCoordinator::complete_no_contrast(&fixture.worker, &fixture.store, &first.id)
            .await
            .unwrap();
    assert_eq!(record.claim_id, first.id);
}

#[tokio::test]
async fn a_superseded_unrun_claimed_generation_expires_and_cannot_finish() {
    let fixture = Fixture::new().await;
    fixture.close(1, true).await;
    fixture.close(2, true).await;
    let scope_id = fixture.scope_id().await;
    let first = claimed(&fixture, &scope_id).await;
    assert_eq!(first.state, ConsolidationClaimState::Claimed);
    fixture.close(3, true).await;
    fixture.close(4, true).await;
    let second = claimed(&fixture, &scope_id).await;
    assert_eq!(
        (second.generation, second.state),
        (2, ConsolidationClaimState::Claimed)
    );
    // Only the state moved: identity, cycles, sources and watermark are kept.
    let mut expected = first.clone();
    expected.state = ConsolidationClaimState::GenerationExpired;
    assert_eq!(stored_claim(&fixture, 1).await, expected);
    assert!(fixture.audits().contains(&(
        "monitoring.consolidation.expire".to_owned(),
        first.id.clone()
    )));

    let rows = fixture.rows().await;
    let audits = fixture.audits();
    let refused =
        MonitoringCoordinator::complete_no_contrast(&fixture.worker, &fixture.store, &first.id)
            .await;
    assert!(
        matches!(&refused, Err(Error::Conflict(message)) if message == "only a no-contrast claim can finish without dispatch"),
        "{refused:?}"
    );
    assert_eq!(fixture.rows().await, rows);
    assert_eq!(fixture.audits(), audits);
    assert!(fixture.artifacts(RUN_SCHEMA).await.is_empty());
    assert_eq!(fixture.root().await, (0, 0));
}

#[tokio::test]
async fn running_blocked_and_completed_generations_never_expire() {
    for state in ["running", "blocked_budget", "completed_no_change"] {
        let fixture = Fixture::new().await;
        fixture.close(1, state != "completed_no_change").await;
        fixture.close(2, state != "completed_no_change").await;
        let scope_id = fixture.scope_id().await;
        let first = claimed(&fixture, &scope_id).await;
        if state == "completed_no_change" {
            MonitoringCoordinator::complete_no_contrast(&fixture.worker, &fixture.store, &first.id)
                .await
                .unwrap();
        } else {
            set_claim_state(&fixture, &first, state).await;
        }
        let before = fixture.raw(&first.id).await;
        assert_eq!(fixture.value(&first.id).await["state"], json!(state));
        fixture.close(3, true).await;
        fixture.close(4, true).await;
        let second = claimed(&fixture, &scope_id).await;
        assert_eq!(second.generation, 2);
        assert_eq!(fixture.raw(&first.id).await, before, "{state}");
        assert!(
            !fixture
                .audits()
                .iter()
                .any(|(operation, _)| operation == "monitoring.consolidation.expire"),
            "{state}"
        );
    }
}

#[tokio::test]
async fn an_older_no_contrast_claim_keeps_its_exact_v1_bytes() {
    let fixture = Fixture::new().await;
    fixture.close(1, false).await;
    fixture.close(2, false).await;
    let scope_id = fixture.scope_id().await;
    let first = claimed(&fixture, &scope_id).await;
    assert_eq!(first.state, ConsolidationClaimState::NoContrast);
    let before = fixture.raw(&first.id).await;
    fixture.close(3, true).await;
    fixture.close(4, true).await;
    claimed(&fixture, &scope_id).await;
    assert_eq!(fixture.raw(&first.id).await, before);
    assert_eq!(
        fixture.value(&first.id).await["state"],
        json!("no_contrast")
    );
}

#[tokio::test]
async fn the_generation_batch_is_refused_by_capacity_before_any_partial_record() {
    let fixture = Fixture::new().await;
    for cycle in 1..=4 {
        fixture.close(cycle, true).await;
    }
    // 249 occupied claim slots leave room for one claim, but generations one
    // and two both need a record.
    let mut session = fixture.store.session().await.unwrap();
    for slot in 0..249 {
        let id = format!("occupied-claim-slot-{slot}");
        session
            .put(
                &fixture.worker,
                "artifact",
                &id,
                fixture.worker.actor(),
                &json!({"id": id, "schema_version": CLAIM_SCHEMA}),
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let scope_id = fixture.scope_id().await;
    let rows = fixture.rows().await;
    let audits = fixture.audits();
    let refused = fixture.claim(&scope_id).await;
    assert!(
        matches!(
            &refused,
            Err(Error::Capacity(conflict)) if *conflict == CapacityConflict {
                item: CapacityItem::ConsolidationClaim,
                used: 249,
                limit: 250,
                delta: 2,
            }
        ),
        "{refused:?}"
    );
    assert_eq!(fixture.rows().await, rows);
    assert_eq!(fixture.audits(), audits);
    assert!(
        fixture.value(&scope_id).await["claim_ids"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}
