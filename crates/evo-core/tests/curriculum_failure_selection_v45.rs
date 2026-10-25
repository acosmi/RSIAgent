//! AG-170 (plan §8.1.1/§8.4.1, V109): the failure-record state machine, the
//! learner-asset and retest-mark transitions and the v3 plateau trigger of
//! the curriculum core.
//!
//! The environment/grading drift transition is only reachable through this
//! reducer: the engine's `record_cycle` refuses a drifted cycle before it
//! reduces anything, and no engine entry changes a learner state's
//! environment digest (disclosed boundary of the AG-170 rework).

use evo_core::curriculum::{
    AppliedLearnerAssetRef, CoverageBucketState, CurriculumControlProfileV1, DRIFT_NOTE,
    FailureCycleOutcome, FailureRecordStatus, FailureRecordType, FailureResolutionV1,
    LEGACY_V2_UNTYPED_NOTE, LearnerStateV2, LearnerStateV3, PlateauSignalV1,
    UNSUPPORTED_SCOPE_REASON, apply_cycle_to_failure_records, detect_plateau_signal_v3,
    failure_record_id, learner_skill_snapshot_after_asset, legacy_failure_record_id,
};
use evo_core::{Error, hash};

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

fn v2_state(failure_clusters: &[&str]) -> LearnerStateV2 {
    LearnerStateV2 {
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
        failure_clusters: failure_clusters.iter().map(|c| (*c).into()).collect(),
        coverage_buckets: vec![],
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    }
}

fn base_state() -> LearnerStateV3 {
    LearnerStateV3::from_v2(v2_state(&[]))
}

fn with_buckets(mut state: LearnerStateV3, buckets: &[&str]) -> LearnerStateV3 {
    state.coverage_buckets = buckets
        .iter()
        .map(|bucket| CoverageBucketState {
            bucket_id: (*bucket).into(),
            task_space_source_id: "task-space-source".into(),
            observed_independent_clusters: 0,
            required_independent_clusters: 1,
        })
        .collect();
    state.validate().unwrap();
    state
}

/// One verified outcome; `tag` names its per-task candidate execution
/// receipt and grader receipt.
fn outcome(cluster: &str, passed: bool, gain_micros: i64, tag: &str) -> FailureCycleOutcome {
    FailureCycleOutcome {
        cluster_id: cluster.into(),
        task_id: format!("task-{cluster}"),
        passed,
        paired_gain_micros: gain_micros,
        candidate_execution_id: format!("devexec-{tag}"),
        grader_receipt_digest: d(&format!("grader-receipt-{tag}")),
    }
}

/// Reduces one cycle in the state's own environment, grading version and
/// Skill snapshot, then re-derives the v3 cluster list.
fn reduce(state: &mut LearnerStateV3, cycle_id: &str, outcomes: &[FailureCycleOutcome]) {
    let (environment, grader, skill) = (
        state.environment_digest.clone(),
        state.grader_digest.clone(),
        state.skill_snapshot_digest.clone(),
    );
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        cycle_id,
        &environment,
        &grader,
        &skill,
        outcomes,
    )
    .unwrap();
    state.failure_clusters = state.active_absolute_failure_clusters();
    state.validate().unwrap();
}

fn asset(release: &str) -> AppliedLearnerAssetRef {
    AppliedLearnerAssetRef {
        release_id: release.into(),
        bundle_digest: d(&format!("bundle-{release}")),
        run_application_id: format!("run-snapshot-{release}"),
        host_execution_receipt_id: format!("host-execution-{release}"),
    }
}

fn families(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).into()).collect()
}

fn record_of<'a>(
    state: &'a LearnerStateV3,
    cluster: &str,
    cycle: &str,
) -> &'a evo_core::curriculum::FailureRecordV1 {
    state
        .failure_records
        .iter()
        .find(|record| record.cluster_id == cluster && record.cycle_id.as_deref() == Some(cycle))
        .unwrap_or_else(|| panic!("no record of {cluster} from {cycle}"))
}

#[test]
fn absolute_failure_and_regression_reduce_into_separate_typed_records_with_per_task_receipts() {
    let mut state = base_state();
    reduce(
        &mut state,
        "cycle-1",
        &[
            outcome("family-abs", false, -100, "abs-1"),
            outcome("family-reg", true, -5_000, "reg-1"),
            outcome("family-ok", true, 4_000, "ok-1"),
        ],
    );
    // Two typed records; only the absolute failure is a current failure gap
    // and the clean pass creates nothing.
    assert_eq!(state.failure_records.len(), 2);
    assert_eq!(
        state.active_absolute_failure_clusters(),
        vec!["family-abs".to_string()]
    );
    assert_eq!(state.failure_clusters, vec!["family-abs".to_string()]);
    let absolute = record_of(&state, "family-abs", "cycle-1");
    assert_eq!(
        absolute.record_type,
        Some(FailureRecordType::AbsoluteFailure)
    );
    assert_eq!(absolute.status, FailureRecordStatus::Active);
    // B1: the record keeps the verified task and its own receipts.
    assert_eq!(absolute.task_id.as_deref(), Some("task-family-abs"));
    assert_eq!(
        absolute.candidate_execution_id.as_deref(),
        Some("devexec-abs-1")
    );
    assert_eq!(
        absolute.grader_receipt_digest.as_deref(),
        Some(d("grader-receipt-abs-1").as_str())
    );
    assert_eq!(absolute.skill_snapshot_digest, state.skill_snapshot_digest);
    assert_eq!(
        absolute.record_id,
        failure_record_id("family-abs", FailureRecordType::AbsoluteFailure, "cycle-1")
    );
    let regression = record_of(&state, "family-reg", "cycle-1");
    assert_eq!(regression.record_type, Some(FailureRecordType::Regression));
    assert_eq!(regression.status, FailureRecordStatus::Active);
    // Receipts are per task, not one digest shared by the whole cycle.
    assert_ne!(
        regression.candidate_execution_id,
        absolute.candidate_execution_id
    );
    assert_ne!(
        regression.grader_receipt_digest,
        absolute.grader_receipt_digest
    );
}

#[test]
fn a_verified_pass_is_stored_as_resolution_and_never_rewrites_the_failure_evidence() {
    let mut state = base_state();
    reduce(
        &mut state,
        "cycle-1",
        &[outcome("family-a", false, -100, "c1")],
    );
    let failed = record_of(&state, "family-a", "cycle-1").clone();
    reduce(
        &mut state,
        "cycle-2",
        &[outcome("family-a", true, 6_000, "c2")],
    );
    let resolved = record_of(&state, "family-a", "cycle-1").clone();
    assert_eq!(resolved.status, FailureRecordStatus::Resolved);
    // The original failure evidence is untouched (§8.1.1 原记录保留)...
    assert_eq!(resolved.cycle_id, failed.cycle_id);
    assert_eq!(resolved.task_id, failed.task_id);
    assert_eq!(
        resolved.candidate_execution_id,
        failed.candidate_execution_id
    );
    assert_eq!(resolved.grader_receipt_digest, failed.grader_receipt_digest);
    assert_eq!(resolved.skill_snapshot_digest, failed.skill_snapshot_digest);
    assert_eq!(resolved.environment_digest, failed.environment_digest);
    assert_eq!(resolved.grader_version_digest, failed.grader_version_digest);
    // ...and the pass that resolved it is stored next to it.
    assert_eq!(
        resolved.resolution,
        Some(FailureResolutionV1 {
            resolved_cycle_id: "cycle-2".into(),
            resolved_task_id: "task-family-a".into(),
            resolved_candidate_execution_id: "devexec-c2".into(),
            resolved_grader_receipt_digest: d("grader-receipt-c2"),
            resolved_skill_snapshot_digest: state.skill_snapshot_digest.clone(),
        })
    );
    assert!(state.active_absolute_failure_clusters().is_empty());
    // A new failure of the cluster makes it active again as a new record; the
    // resolved record and its resolution stay exactly as they were.
    reduce(
        &mut state,
        "cycle-3",
        &[outcome("family-a", false, -100, "c3")],
    );
    assert_eq!(state.failure_records.len(), 2);
    assert_eq!(record_of(&state, "family-a", "cycle-1"), &resolved);
    let reopened = record_of(&state, "family-a", "cycle-3");
    assert_eq!(reopened.status, FailureRecordStatus::Active);
    assert_eq!(
        reopened.candidate_execution_id.as_deref(),
        Some("devexec-c3")
    );
    assert_eq!(
        state.active_absolute_failure_clusters(),
        vec!["family-a".to_string()]
    );
    // One cycle reduces once.
    let (environment, grader, skill) = (
        state.environment_digest.clone(),
        state.grader_digest.clone(),
        state.skill_snapshot_digest.clone(),
    );
    assert!(matches!(
        apply_cycle_to_failure_records(
            &mut state.failure_records,
            "cycle-2",
            &environment,
            &grader,
            &skill,
            &[outcome("family-a", true, 1, "c2-again")],
        ),
        Err(Error::Conflict(_))
    ));
}

#[test]
fn a_regression_pass_resolves_the_absolute_failure_and_opens_a_regression_record() {
    let mut state = with_buckets(base_state(), &["family-a"]);
    reduce(
        &mut state,
        "cycle-1",
        &[outcome("family-a", false, -100, "c1")],
    );
    // I1: a pass resolves the absolute failure whatever its gain; the negative
    // gain only opens a regression record, which is no failure gap.
    reduce(
        &mut state,
        "cycle-2",
        &[outcome("family-a", true, -2_000, "c2")],
    );
    let absolute = record_of(&state, "family-a", "cycle-1");
    assert_eq!(absolute.status, FailureRecordStatus::Resolved);
    assert_eq!(
        absolute
            .resolution
            .as_ref()
            .map(|resolution| resolution.resolved_cycle_id.as_str()),
        Some("cycle-2")
    );
    let regression = record_of(&state, "family-a", "cycle-2");
    assert_eq!(regression.record_type, Some(FailureRecordType::Regression));
    assert_eq!(regression.status, FailureRecordStatus::Active);
    assert!(state.active_absolute_failure_clusters().is_empty());
    let signal = detect_plateau_signal_v3(&profile(), &state).unwrap();
    assert!(
        !matches!(signal, PlateauSignalV1::FailureGap { .. }),
        "a regression must never trigger failure_gap: {signal:?}"
    );
    // A later pass without a regression resolves the regression record too.
    reduce(
        &mut state,
        "cycle-3",
        &[outcome("family-a", true, 1_000, "c3")],
    );
    assert_eq!(
        record_of(&state, "family-a", "cycle-2").status,
        FailureRecordStatus::Resolved
    );
}

#[test]
fn drift_moves_open_records_to_diagnostic_without_touching_their_evidence() {
    let mut state = with_buckets(base_state(), &["family-b", "family-c"]);
    reduce(
        &mut state,
        "cycle-1",
        &[
            outcome("family-a", false, -100, "a1"),
            outcome("family-b", false, -100, "b1"),
        ],
    );
    reduce(&mut state, "cycle-2", &[outcome("family-a", true, 1, "a2")]);
    let open_before = record_of(&state, "family-b", "cycle-1").clone();
    let resolved_before = record_of(&state, "family-a", "cycle-1").clone();
    // A later cycle under another grading version (core reducer only; see the
    // module note) demotes the open record and keeps the resolved one.
    let (environment, skill) = (
        state.environment_digest.clone(),
        state.skill_snapshot_digest.clone(),
    );
    apply_cycle_to_failure_records(
        &mut state.failure_records,
        "cycle-3",
        &environment,
        &d("grader-v2"),
        &skill,
        &[outcome("family-c", false, -100, "c3")],
    )
    .unwrap();
    state.failure_clusters = state.active_absolute_failure_clusters();
    state.validate().unwrap();
    let drifted = record_of(&state, "family-b", "cycle-1");
    assert_eq!(drifted.status, FailureRecordStatus::Diagnostic);
    assert_eq!(drifted.transition_note.as_deref(), Some(DRIFT_NOTE));
    assert_eq!(drifted.cycle_id, open_before.cycle_id);
    assert_eq!(
        drifted.candidate_execution_id,
        open_before.candidate_execution_id
    );
    assert_eq!(
        drifted.grader_version_digest,
        open_before.grader_version_digest
    );
    assert_eq!(record_of(&state, "family-a", "cycle-1"), &resolved_before);
    // The drifted record no longer counts: the trigger answers with the live
    // cluster only (V109.a: 环境漂移后旧失败不计当前).
    assert_eq!(
        state.active_absolute_failure_clusters(),
        vec!["family-c".to_string()]
    );
    let PlateauSignalV1::FailureGap {
        failure_cluster_ids,
    } = detect_plateau_signal_v3(&profile(), &state).unwrap()
    else {
        panic!("expected a failure gap for the live cluster");
    };
    assert_eq!(failure_cluster_ids, vec!["family-c".to_string()]);
}

#[test]
fn regression_only_states_never_trigger_failure_gap_v3() {
    let mut state = with_buckets(base_state(), &["family-reg"]);
    reduce(
        &mut state,
        "cycle-1",
        &[outcome("family-reg", true, -5_000, "reg-1")],
    );
    assert_eq!(state.failure_records[0].status, FailureRecordStatus::Active);
    assert!(state.active_absolute_failure_clusters().is_empty());
    // The cluster is a registered bucket, so only the record type keeps the
    // regression out of the failure gap (V109.a: 回归不触发failure_gap).
    let signal = detect_plateau_signal_v3(&profile(), &state).unwrap();
    assert!(
        !matches!(signal, PlateauSignalV1::FailureGap { .. }),
        "regression records must never trigger failure_gap: {signal:?}"
    );
}

#[test]
fn a_failure_retires_only_by_a_verified_pass_of_its_own_cluster() {
    let mut state = base_state();
    reduce(
        &mut state,
        "cycle-1",
        &[outcome("family-a", false, -100, "a1")],
    );
    let failed = record_of(&state, "family-a", "cycle-1").clone();
    // V109.a: neither the same template under another seed nor a copy of the
    // template, each registered as its own cluster, retires the failure, and
    // no number of rounds does either.
    for round in 2..=6 {
        reduce(
            &mut state,
            &format!("cycle-{round}"),
            &[
                outcome("family-a-seed-2", true, 1_000, &format!("seed-{round}")),
                outcome(
                    "family-a-template-copy",
                    true,
                    1_000,
                    &format!("copy-{round}"),
                ),
            ],
        );
        assert_eq!(record_of(&state, "family-a", "cycle-1"), &failed);
    }
    // An applied asset only moves it to retest_due; it neither resolves nor
    // removes it.
    assert!(
        state
            .apply_learner_asset(asset("release-1"), &families(&["family-a"]))
            .unwrap()
    );
    let retest = record_of(&state, "family-a", "cycle-1");
    assert_eq!(retest.status, FailureRecordStatus::RetestDue);
    assert!(retest.resolution.is_none());
    assert_eq!(retest.candidate_execution_id, failed.candidate_execution_id);
    // Only a verified pass of the cluster itself resolves it.
    reduce(
        &mut state,
        "cycle-7",
        &[outcome("family-a", true, 1_000, "a7")],
    );
    let resolved = record_of(&state, "family-a", "cycle-1");
    assert_eq!(resolved.status, FailureRecordStatus::Resolved);
    assert_eq!(
        resolved
            .resolution
            .as_ref()
            .map(|resolution| resolution.resolved_cycle_id.as_str()),
        Some("cycle-7")
    );
}

#[test]
fn an_applied_asset_marks_every_bound_family_and_moves_only_active_records() {
    // One legacy diagnostic family (projected from v2), one active, one
    // resolved and one family without any record.
    let mut state = LearnerStateV3::from_v2(v2_state(&["family-diagnostic"]));
    reduce(
        &mut state,
        "cycle-1",
        &[
            outcome("family-active", false, -100, "act-1"),
            outcome("family-resolved", false, -100, "res-1"),
        ],
    );
    reduce(
        &mut state,
        "cycle-2",
        &[outcome("family-resolved", true, 1, "res-2")],
    );
    let resolved_before = record_of(&state, "family-resolved", "cycle-1").clone();
    let diagnostic_before = state
        .failure_records
        .iter()
        .find(|record| record.cluster_id == "family-diagnostic")
        .unwrap()
        .clone();
    let bound = families(&[
        "family-active",
        "family-resolved",
        "family-diagnostic",
        "family-clean",
    ]);
    assert!(
        state
            .apply_learner_asset(asset("release-1"), &bound)
            .unwrap()
    );
    // I8: every bound family gets one unconsumed mark, whatever its records.
    let mut marked: Vec<(&str, &str, bool)> = state
        .retest_priority_marks
        .iter()
        .map(|mark| {
            (
                mark.family.as_str(),
                mark.release_id.as_str(),
                mark.consumed,
            )
        })
        .collect();
    marked.sort();
    assert_eq!(
        marked,
        vec![
            ("family-active", "release-1", false),
            ("family-clean", "release-1", false),
            ("family-diagnostic", "release-1", false),
            ("family-resolved", "release-1", false),
        ]
    );
    let retest = record_of(&state, "family-active", "cycle-1");
    assert_eq!(retest.status, FailureRecordStatus::RetestDue);
    assert_eq!(retest.retest_release_id.as_deref(), Some("release-1"));
    assert_eq!(
        retest.transition_note.as_deref(),
        Some("retest_after:release-1")
    );
    assert_eq!(
        record_of(&state, "family-resolved", "cycle-1"),
        &resolved_before
    );
    assert!(state.failure_records.contains(&diagnostic_before));
    // The same release again changes nothing; a changed identity or an asset
    // bound to no family is refused.
    let before = serde_json::to_value(&state).unwrap();
    assert!(
        !state
            .apply_learner_asset(asset("release-1"), &bound)
            .unwrap()
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    let mut changed = asset("release-1");
    changed.bundle_digest = d("another-bundle");
    assert!(matches!(
        state.apply_learner_asset(changed, &bound),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        state.apply_learner_asset(asset("release-2"), &[]),
        Err(Error::Conflict(message)) if message.contains("release") && message.contains("family")
    ));
}

#[test]
fn consuming_a_retest_mark_is_one_shot_and_a_void_release_reopens_its_records() {
    let mut state = base_state();
    reduce(
        &mut state,
        "cycle-1",
        &[
            outcome("family-a", false, -100, "a1"),
            outcome("family-b", false, -100, "b1"),
        ],
    );
    state
        .apply_learner_asset(asset("release-1"), &families(&["family-a"]))
        .unwrap();
    state
        .apply_learner_asset(asset("release-2"), &families(&["family-b"]))
        .unwrap();
    assert!(state.failure_clusters.is_empty());
    // I7: the selection consumes the mark and the retest-due record is a
    // current gap again; the mark cannot be consumed twice.
    state
        .consume_retest_priority_mark("family-a", "release-1")
        .unwrap();
    let consumed = state
        .retest_priority_marks
        .iter()
        .find(|mark| mark.family == "family-a")
        .unwrap();
    assert!(consumed.consumed);
    let reopened = record_of(&state, "family-a", "cycle-1");
    assert_eq!(reopened.status, FailureRecordStatus::Active);
    assert_eq!(reopened.retest_release_id, None);
    assert_eq!(
        reopened.transition_note.as_deref(),
        Some("retest_selected:release-1")
    );
    assert_eq!(state.failure_clusters, vec!["family-a".to_string()]);
    assert!(matches!(
        state.consume_retest_priority_mark("family-a", "release-1"),
        Err(Error::Conflict(_))
    ));
    // A rolled-back or revoked release voids its influence: its retest-due
    // records are active again and its mark stays unconsumed.
    state.void_retest_due_of_release("release-2").unwrap();
    let voided = record_of(&state, "family-b", "cycle-1");
    assert_eq!(voided.status, FailureRecordStatus::Active);
    assert_eq!(
        voided.transition_note.as_deref(),
        Some("retest_void:release-2")
    );
    assert!(
        state
            .retest_priority_marks
            .iter()
            .any(|mark| mark.family == "family-b" && !mark.consumed)
    );
    assert_eq!(
        state.failure_clusters,
        vec!["family-a".to_string(), "family-b".to_string()]
    );
}

#[test]
fn an_applied_asset_moves_the_skill_snapshot_of_later_failure_records() {
    let mut state = base_state();
    let before_asset = state.skill_snapshot_digest.clone();
    reduce(
        &mut state,
        "cycle-1",
        &[outcome("family-a", false, -100, "a1")],
    );
    state
        .apply_learner_asset(asset("release-1"), &families(&["family-a"]))
        .unwrap();
    // B1(d): the trusted application moves the learner Skill snapshot.
    let after_asset = learner_skill_snapshot_after_asset(&before_asset, &asset("release-1"));
    assert_ne!(after_asset, before_asset);
    assert_eq!(state.skill_snapshot_digest, after_asset);
    reduce(
        &mut state,
        "cycle-2",
        &[
            outcome("family-a", false, -100, "a2"),
            outcome("family-b", false, -100, "b2"),
        ],
    );
    // The earlier record keeps the snapshot it failed under; its retest
    // failed, so it is active again. Records written later carry the new
    // snapshot.
    let original = record_of(&state, "family-a", "cycle-1");
    assert_eq!(original.skill_snapshot_digest, before_asset);
    assert_eq!(original.status, FailureRecordStatus::Active);
    assert_eq!(
        original.transition_note.as_deref(),
        Some("retest_failed:cycle-2")
    );
    assert_eq!(
        record_of(&state, "family-a", "cycle-2").skill_snapshot_digest,
        after_asset
    );
    assert_eq!(
        record_of(&state, "family-b", "cycle-2").skill_snapshot_digest,
        after_asset
    );
}

#[test]
fn legacy_v2_failure_clusters_survive_as_untyped_diagnostics() {
    let mut state = with_buckets(
        LearnerStateV3::from_v2(v2_state(&["legacy-a", "legacy-b"])),
        &["legacy-a"],
    );
    state.validate().unwrap();
    assert_eq!(state.schema_version, "rsia.learner_state.v3");
    // I3: the bare v2 clusters are neither dropped nor counted as a gap.
    assert!(state.failure_clusters.is_empty());
    assert_eq!(state.failure_records.len(), 2);
    for record in &state.failure_records {
        assert_eq!(record.status, FailureRecordStatus::Diagnostic);
        assert_eq!(
            record.transition_note.as_deref(),
            Some(LEGACY_V2_UNTYPED_NOTE)
        );
        assert_eq!(record.record_type, None);
        assert_eq!(record.cycle_id, None);
        assert_eq!(record.candidate_execution_id, None);
        assert_eq!(
            record.record_id,
            legacy_failure_record_id(&record.cluster_id)
        );
    }
    assert!(!matches!(
        detect_plateau_signal_v3(&profile(), &state).unwrap(),
        PlateauSignalV1::FailureGap { .. }
    ));
    // The next cycle does not overwrite them, even when the cluster passes.
    let legacy = state.failure_records.clone();
    reduce(&mut state, "cycle-1", &[outcome("legacy-a", true, 1, "l1")]);
    assert_eq!(state.failure_records, legacy);
    // A legacy record cannot pose as a typed or current one, and the v3
    // cluster list has exactly one meaning.
    let mut forged = state.clone();
    forged.failure_records[0].status = FailureRecordStatus::Active;
    assert!(forged.validate().is_err());
    let mut forged = state.clone();
    forged.failure_records[0].record_type = Some(FailureRecordType::AbsoluteFailure);
    assert!(forged.validate().is_err());
    let mut forged = state.clone();
    forged.failure_clusters = vec!["legacy-a".into()];
    assert!(forged.validate().is_err());
}

#[test]
fn unsupported_scope_keeps_only_registered_failure_clusters() {
    let mut state = with_buckets(base_state(), &["family-a"]);
    reduce(
        &mut state,
        "cycle-1",
        &[
            outcome("family-a", false, -100, "a1"),
            outcome("family-orphan", false, -100, "o1"),
        ],
    );
    // I4: the orphan cluster is dropped, the registered one still triggers.
    let PlateauSignalV1::FailureGap {
        failure_cluster_ids,
    } = detect_plateau_signal_v3(&profile(), &state).unwrap()
    else {
        panic!("expected a failure gap for the registered cluster");
    };
    assert_eq!(failure_cluster_ids, vec!["family-a".to_string()]);
    // With only orphan failures there is no executable direction: no
    // failure gap, the unsupported-scope reason instead.
    let mut orphan_only = with_buckets(base_state(), &["family-a"]);
    reduce(
        &mut orphan_only,
        "cycle-1",
        &[outcome("family-orphan", false, -100, "o1")],
    );
    let PlateauSignalV1::NotTriggered { reasons } =
        detect_plateau_signal_v3(&profile(), &orphan_only).unwrap()
    else {
        panic!("expected NotTriggered for an orphan-only failure gap");
    };
    assert_eq!(reasons, vec![UNSUPPORTED_SCOPE_REASON.to_string()]);
}

#[test]
fn record_status_invariants_are_enforced() {
    let mut state = base_state();
    reduce(
        &mut state,
        "cycle-1",
        &[outcome("family-a", false, -100, "a1")],
    );
    // retest_release_id only on a retest_due record.
    let mut forged = state.clone();
    forged.failure_records[0].retest_release_id = Some("release-1".into());
    assert!(forged.failure_records[0].validate().is_err());
    assert!(forged.validate().is_err());
    // A resolution only on a resolved record.
    let mut forged = state.clone();
    forged.failure_records[0].resolution = Some(FailureResolutionV1 {
        resolved_cycle_id: "cycle-2".into(),
        resolved_task_id: "task-family-a".into(),
        resolved_candidate_execution_id: "devexec-x".into(),
        resolved_grader_receipt_digest: d("x"),
        resolved_skill_snapshot_digest: d("skill"),
    });
    assert!(forged.validate().is_err());
    // A typed record needs its receipts and an id derived from them.
    let mut forged = state.clone();
    forged.failure_records[0].candidate_execution_id = None;
    assert!(forged.validate().is_err());
    let mut forged = state.clone();
    forged.failure_records[0].cycle_id = Some("cycle-9".into());
    assert!(forged.validate().is_err());
    // A mark must name an applied release.
    let mut forged = state.clone();
    forged
        .retest_priority_marks
        .push(evo_core::curriculum::RetestPriorityMarkV1 {
            family: "family-a".into(),
            release_id: "release-never-applied".into(),
            consumed: false,
        });
    assert!(forged.validate().is_err());
}

#[test]
fn a_profile_without_a_seed_commitment_keeps_its_exact_v1_bytes() {
    let profile = CurriculumControlProfileV1::offline_default(
        "pure_function_test_proposal.v1",
        "a".repeat(64),
        "b".repeat(64),
        "c".repeat(64),
    )
    .unwrap();
    // Golden: the v1 field order and values as they were before the
    // commitment field existed (I9: the absent field is not serialized).
    let golden = format!(
        concat!(
            r#"{{"schema_version":"rsia.curriculum_control_profile.v1","#,
            r#""profile_id":"pure_function_test_proposal.v1","#,
            r#""task_space_digest":"{a}","oracle_digest":"{b}","runner_digest":"{c}","#,
            r#""max_total_proposals":4,"plateau_window_cycles":3,"#,
            r#""min_independent_clusters_per_cycle":10,"min_unique_window_clusters":30,"#,
            r#""min_success_rate_micros":950000,"max_abs_mean_gain_micros":5000,"#,
            r#""max_gain_variance_micros_squared":1000000000,"cooldown_completed_cycles":3,"#,
            r#""curriculum_budget_share_bps":2000,"max_concurrent_probe_jobs":1,"#,
            r#""monetary_limit_micros":0,"runtime_status":"disabled"}}"#
        ),
        a = "a".repeat(64),
        b = "b".repeat(64),
        c = "c".repeat(64),
    );
    assert_eq!(serde_json::to_string(&profile).unwrap(), golden);
    let decoded: CurriculumControlProfileV1 = serde_json::from_str(&golden).unwrap();
    decoded.validate().unwrap();
    assert!(decoded.fixed_random_seed_commitment.is_none());
    assert_eq!(serde_json::to_string(&decoded).unwrap(), golden);
    // With a commitment the key appears and round-trips.
    let mut committed = profile.clone();
    committed.fixed_random_seed_commitment = Some(d("seed-commitment"));
    let wire = serde_json::to_string(&committed).unwrap();
    assert!(wire.contains(r#""fixed_random_seed_commitment":"#));
    let decoded: CurriculumControlProfileV1 = serde_json::from_str(&wire).unwrap();
    assert_eq!(
        decoded.fixed_random_seed_commitment,
        Some(d("seed-commitment"))
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
}
