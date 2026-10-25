//! AG-170 (plan §8.1.1/§8.4.1, V109): the failure-record state machine, the
//! three-arm selection bookkeeping types and the v3 plateau trigger of the
//! curriculum core.

use evo_core::curriculum::{
    CurriculumControlProfileV1, FailureCycleOutcome, FailureRecordStatus, FailureRecordType,
    LearnerStateV2, LearnerStateV3, PlateauSignalV1, apply_cycle_to_failure_records,
    detect_plateau_signal_v3, failure_record_id, mark_retest_due_after_asset,
};
use evo_core::hash;

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn profile() -> CurriculumControlProfileV1 {
    CurriculumControlProfileV1::offline_default(
        "pure_function_test_proposal.v1",
        d("task-space"),
        d("oracle"),
        d("runner"),
    )
    .unwrap()
}

fn base_state() -> LearnerStateV3 {
    LearnerStateV3::from_v2(LearnerStateV2 {
        schema_version: LearnerStateV2::SCHEMA.into(),
        id: "state-1".into(),
        profile_id: "pure_function_test_proposal.v1".into(),
        skill_snapshot_digest: d("skill"),
        improver_snapshot_digest: d("improver"),
        environment_digest: d("env"),
        grader_digest: d("grader"),
        model_tools_digest: d("model-tools"),
        runner_digest: d("runner"),
        rules_digest: d("rules"),
        source_watermark: 1,
        source_artifact_ids: vec!["task-space-source".into()],
        development_fact_ids: vec![],
        completed_cycles: vec![],
        failure_clusters: vec![],
        coverage_buckets: vec![],
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    })
}

fn outcome(cluster: &str, passed: bool, gain_micros: i64) -> FailureCycleOutcome {
    FailureCycleOutcome {
        cluster_id: cluster.into(),
        passed,
        paired_gain_micros: gain_micros,
        execution_receipt_digest: d(&format!("exec-{cluster}")),
        grader_receipt_digest: d(&format!("grader-{cluster}")),
    }
}

#[test]
fn absolute_failure_and_regression_reduce_into_separate_typed_records() {
    let mut state = base_state();
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        "cycle-1",
        &state.environment_digest,
        &state.grader_digest,
        &state.skill_snapshot_digest,
        &[
            outcome("family-abs", false, -100),
            outcome("family-reg", true, -5_000),
            outcome("family-ok", true, 4_000),
        ],
    )
    .unwrap();
    // Three typed records exist, but only the absolute failure constitutes a
    // current failure gap: the regression stays out of the active gap list and
    // the clean pass creates nothing.
    assert_eq!(state.failure_records.len(), 2);
    assert_eq!(
        state.active_absolute_failure_clusters(),
        vec!["family-abs".to_string()]
    );
    let regression = state
        .failure_records
        .iter()
        .find(|record| record.record_type == FailureRecordType::Regression)
        .unwrap();
    assert_eq!(regression.status, FailureRecordStatus::Active);
    assert_eq!(
        regression.record_id,
        failure_record_id("family-reg", FailureRecordType::Regression)
    );
}

#[test]
fn verified_pass_resolves_records_and_keeps_them_reactivating_on_next_failure() {
    let mut state = base_state();
    let digests = (
        state.environment_digest.clone(),
        state.grader_digest.clone(),
        state.skill_snapshot_digest.clone(),
    );
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        "cycle-1",
        &digests.0,
        &digests.1,
        &digests.2,
        &[outcome("family-a", false, -100)],
    )
    .unwrap();
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        "cycle-2",
        &digests.0,
        &digests.1,
        &digests.2,
        &[outcome("family-a", true, 6_000)],
    )
    .unwrap();
    let resolved = &state.failure_records[0];
    assert_eq!(resolved.status, FailureRecordStatus::Resolved);
    assert!(state.active_absolute_failure_clusters().is_empty());
    // The record itself survives the resolution.
    assert_eq!(state.failure_records.len(), 1);
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        "cycle-3",
        &digests.0,
        &digests.1,
        &digests.2,
        &[outcome("family-a", false, -100)],
    )
    .unwrap();
    assert_eq!(state.failure_records[0].status, FailureRecordStatus::Active);
    assert_eq!(
        state.active_absolute_failure_clusters(),
        vec!["family-a".to_string()]
    );
}

#[test]
fn drift_moves_records_to_diagnostic_and_out_of_the_current_gap() {
    let mut state = base_state();
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        "cycle-1",
        &state.environment_digest,
        &state.grader_digest,
        &state.skill_snapshot_digest,
        &[outcome("family-a", false, -100)],
    )
    .unwrap();
    // A later cycle under a different grading version demotes the old record.
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        "cycle-2",
        &state.environment_digest,
        &d("grader-v2"),
        &state.skill_snapshot_digest,
        &[outcome("family-b", false, -100)],
    )
    .unwrap();
    let drifted = state
        .failure_records
        .iter()
        .find(|record| record.cluster_id == "family-a")
        .unwrap();
    assert_eq!(drifted.status, FailureRecordStatus::Diagnostic);
    assert_eq!(
        drifted.transition_note.as_deref(),
        Some("environment_or_grader_drift")
    );
    assert_eq!(
        state.active_absolute_failure_clusters(),
        vec!["family-b".to_string()]
    );
    // The drifted record no longer triggers a v3 failure gap: the trigger
    // answers with the live cluster only.
    state.failure_clusters = state.active_absolute_failure_clusters();
    let signal = detect_plateau_signal_v3(&profile(), &state).unwrap();
    let PlateauSignalV1::FailureGap {
        failure_cluster_ids,
    } = signal
    else {
        panic!("expected failure gap for the live cluster");
    };
    assert_eq!(failure_cluster_ids, vec!["family-b".to_string()]);
}

#[test]
fn regression_only_states_never_trigger_failure_gap_v3() {
    let mut state = base_state();
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        "cycle-1",
        &state.environment_digest,
        &state.grader_digest,
        &state.skill_snapshot_digest,
        &[outcome("family-reg", true, -5_000)],
    )
    .unwrap();
    // A v2 trigger would have seen the regression as a failure cluster; the
    // v3 trigger must not (V109.a: 回归不触发 failure_gap).
    assert!(state.failure_records[0].status == FailureRecordStatus::Active);
    assert!(state.active_absolute_failure_clusters().is_empty());
    // With no completed-cycle window yet the v3 trigger answers ColdStart;
    // either way it is never FailureGap (V109.a).
    let signal = detect_plateau_signal_v3(&profile(), &state).unwrap();
    assert!(
        !matches!(signal, PlateauSignalV1::FailureGap { .. }),
        "regression records must never trigger failure_gap: {signal:?}"
    );
}

#[test]
fn applied_asset_moves_active_records_to_retest_due_and_marks_new_families() {
    let mut state = base_state();
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        "cycle-1",
        &state.environment_digest,
        &state.grader_digest,
        &state.skill_snapshot_digest,
        &[outcome("family-with-failure", false, -100)],
    )
    .unwrap();
    mark_retest_due_after_asset(
        &mut state.failure_records,
        &mut state.retest_priority_marks,
        &["family-with-failure".into(), "family-clean".into()],
        "release-1",
    )
    .unwrap();
    let retest = &state.failure_records[0];
    assert_eq!(retest.status, FailureRecordStatus::RetestDue);
    assert_eq!(retest.retest_release_id.as_deref(), Some("release-1"));
    assert_eq!(
        retest.transition_note.as_deref(),
        Some("retest_after:release-1")
    );
    // The family without any failure record gets the one-shot mark instead.
    assert_eq!(state.retest_priority_marks.len(), 1);
    assert_eq!(state.retest_priority_marks[0].family, "family-clean");
    assert!(!state.retest_priority_marks[0].consumed);
    // The same release applied again is idempotent, and applying never
    // clears the record.
    mark_retest_due_after_asset(
        &mut state.failure_records,
        &mut state.retest_priority_marks,
        &["family-with-failure".into(), "family-clean".into()],
        "release-1",
    )
    .unwrap();
    assert_eq!(state.failure_records.len(), 1);
    assert_eq!(state.retest_priority_marks.len(), 1);
    assert_eq!(
        state.failure_records[0].status,
        FailureRecordStatus::RetestDue
    );
}

#[test]
fn retest_release_id_only_valid_on_retest_due_records() {
    let mut state = base_state();
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        "cycle-1",
        &state.environment_digest,
        &state.grader_digest,
        &state.skill_snapshot_digest,
        &[outcome("family-a", false, -100)],
    )
    .unwrap();
    state.failure_records[0].retest_release_id = Some("release-1".into());
    assert!(state.failure_records[0].validate().is_err());
    assert!(state.validate().is_err());
}

#[test]
fn v2_projection_starts_with_empty_failure_records_and_keeps_identity() {
    let v2 = LearnerStateV2 {
        schema_version: LearnerStateV2::SCHEMA.into(),
        id: "state-1".into(),
        profile_id: "pure_function_test_proposal.v1".into(),
        skill_snapshot_digest: d("skill"),
        improver_snapshot_digest: d("improver"),
        environment_digest: d("env"),
        grader_digest: d("grader"),
        model_tools_digest: d("model-tools"),
        runner_digest: d("runner"),
        rules_digest: d("rules"),
        source_watermark: 1,
        source_artifact_ids: vec!["task-space-source".into()],
        development_fact_ids: vec![],
        completed_cycles: vec![],
        failure_clusters: vec!["legacy-mixed-cluster".into()],
        coverage_buckets: vec![],
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    };
    let v3 = LearnerStateV3::from_v2(v2);
    v3.validate().unwrap();
    assert_eq!(v3.schema_version, "rsia.learner_state.v3");
    assert!(v3.failure_records.is_empty());
    assert!(v3.retest_priority_marks.is_empty());
    // The legacy mixed list is carried as-is for read-side continuity.
    assert_eq!(
        v3.failure_clusters,
        vec!["legacy-mixed-cluster".to_string()]
    );
}

#[test]
fn fixed_random_seed_commitment_must_be_a_digest() {
    let mut registered = profile();
    assert!(registered.fixed_random_seed_commitment.is_none());
    registered.fixed_random_seed_commitment = Some(d("seed-commitment"));
    registered.validate().unwrap();
    registered.fixed_random_seed_commitment = Some("seed-commitment".into());
    assert!(registered.validate().is_err());
    // A pre-field profile payload (no fixed_random_seed_commitment key) still
    // decodes and reads the field as None.
    let mut legacy = serde_json::to_value(&registered).unwrap();
    let Some(object) = legacy.as_object_mut() else {
        panic!("profile serializes to an object");
    };
    assert!(object.remove("fixed_random_seed_commitment").is_some());
    let decoded: CurriculumControlProfileV1 = serde_json::from_value(legacy).unwrap();
    assert!(decoded.fixed_random_seed_commitment.is_none());
    decoded.validate().unwrap();
}
