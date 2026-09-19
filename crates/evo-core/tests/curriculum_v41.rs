use evo_core::curriculum::{
    CoverageBucketState, CurriculumControlProfileV1, DevelopmentCycleObservation, LearnerStateV2,
    PlateauSignalV1, ProbeJobV1, ProbeTerminal, ProposalAttemptOutcome, RegisteredProperty,
    StructuredTestProposalV1, detect_plateau_signal,
};
use evo_core::evaluation::DataUse;
use evo_core::{fingerprint, hash};

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

fn cycle(index: u32) -> DevelopmentCycleObservation {
    DevelopmentCycleObservation {
        cycle_id: format!("cycle-{index}"),
        environment_digest: d("env"),
        grader_digest: d("grader"),
        cluster_ids: (0..10)
            .map(|cluster| format!("cluster-{index}-{cluster}"))
            .collect(),
        successful_clusters: 10,
        paired_gain_micros: vec![0; 10],
        source_artifact_ids: vec![format!("cycle-receipt-{index}")],
    }
}

fn state(cycles: Vec<DevelopmentCycleObservation>) -> LearnerStateV2 {
    let development_facts = cycles
        .iter()
        .flat_map(|cycle| cycle.source_artifact_ids.iter().cloned())
        .collect();
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
        development_fact_ids: development_facts,
        completed_cycles: cycles,
        failure_clusters: vec![],
        coverage_buckets: vec![],
        applied_assets: vec![],
        active_probe_job_id: None,
        last_trigger_window_digest: None,
        cooldown_remaining_cycles: 0,
    }
}

#[test]
fn empty_failures_with_three_complete_cycles_trigger_plateau_once() {
    let signal =
        detect_plateau_signal(&profile(), &state(vec![cycle(1), cycle(2), cycle(3)])).unwrap();
    let PlateauSignalV1::PlateauProbe {
        unique_cluster_ids,
        mean_gain_micros,
        sample_variance_micros_squared,
        ..
    } = signal
    else {
        panic!("expected plateau probe");
    };
    assert_eq!(unique_cluster_ids.len(), 30);
    assert_eq!(mean_gain_micros, vec![0, 0, 0]);
    assert_eq!(sample_variance_micros_squared, 0);
}

#[test]
fn duplicate_clusters_environment_drift_and_negative_gain_do_not_fake_plateau() {
    let first = cycle(1);
    let mut duplicate = cycle(2);
    duplicate.cluster_ids[0] = first.cluster_ids[0].clone();
    let mut drift = cycle(3);
    drift.paired_gain_micros = vec![-20_000; 10];
    assert!(matches!(
        detect_plateau_signal(
            &profile(),
            &state(vec![first.clone(), duplicate, drift.clone()])
        )
        .unwrap(),
        PlateauSignalV1::NotTriggered { .. }
    ));
    drift.environment_digest = d("other-env");
    assert!(detect_plateau_signal(&profile(), &state(vec![first, cycle(2), drift])).is_err());

    let mut fractional_boundary = vec![cycle(4), cycle(5), cycle(6)];
    for cycle in &mut fractional_boundary {
        cycle.paired_gain_micros = vec![5_001; 10];
        cycle.paired_gain_micros[9] = 5_000;
    }
    assert!(matches!(
        detect_plateau_signal(&profile(), &state(fractional_boundary)).unwrap(),
        PlateauSignalV1::NotTriggered { .. }
    ));
    let mut out_of_range = cycle(7);
    out_of_range.paired_gain_micros[0] = 1_000_001;
    assert!(out_of_range.validate().is_err());
}

#[test]
fn trigger_priority_and_cooldown_are_typed() {
    let mut learner = state(vec![]);
    learner.failure_clusters = vec!["failure-a".into()];
    learner.coverage_buckets = vec![CoverageBucketState {
        bucket_id: "bucket-a".into(),
        task_space_source_id: "task-space-source".into(),
        observed_independent_clusters: 0,
        required_independent_clusters: 1,
    }];
    assert!(matches!(
        detect_plateau_signal(&profile(), &learner).unwrap(),
        PlateauSignalV1::FailureGap { .. }
    ));
    learner.cooldown_remaining_cycles = 1;
    assert!(matches!(
        detect_plateau_signal(&profile(), &learner).unwrap(),
        PlateauSignalV1::NotTriggered { .. }
    ));
}

#[test]
fn all_four_proposal_attempts_count_and_budget_share_is_exact() {
    let profile = profile();
    assert_eq!(profile.curriculum_limit(1_000_000).unwrap(), 200_000);
    assert!(profile.curriculum_limit(u64::MAX).is_err());
    let mut job = ProbeJobV1 {
        schema_version: "rsia.coverage_probe_job.v1".into(),
        id: "job-1".into(),
        profile_id: profile.profile_id.clone(),
        state_id: "state-1".into(),
        state_digest: d("state"),
        trigger_digest: d("trigger"),
        root_budget_limit_micros: 1_000_000,
        curriculum_share_limit_micros: 200_000,
        effective_monetary_limit_micros: 0,
        provider_dispatch_count: 0,
        attempts: vec![],
        terminal: None,
    };
    for outcome in [
        ProposalAttemptOutcome::Duplicate,
        ProposalAttemptOutcome::Malformed,
        ProposalAttemptOutcome::TimedOut,
        ProposalAttemptOutcome::InvalidOracle,
    ] {
        job.record_attempt(&profile, outcome).unwrap();
    }
    assert_eq!(job.attempts.len(), 4);
    assert_eq!(job.terminal, Some(ProbeTerminal::InvalidOracle));
    assert!(
        job.record_attempt(&profile, ProposalAttemptOutcome::Valid)
            .is_err()
    );
}

#[test]
fn structured_proposals_are_development_only_and_have_no_oracle_boolean() {
    let proposal = StructuredTestProposalV1 {
        schema_version: "rsia.structured_test_proposal.v1".into(),
        id: "proposal-1".into(),
        probe_job_id: "probe-1".into(),
        target_id: "reference_host.clamp_i64.v1".into(),
        property: RegisteredProperty::BelowMapsToMin,
        value: -2,
        min: -1,
        max: 1,
        parent_family: "family-1".into(),
        data_use: DataUse::Development,
        source_artifact_ids: vec!["source-1".into()],
        reason: "exercise a registered lower boundary".into(),
    };
    proposal.validate().unwrap();
    for field in ["oracle_ok", "correct", "expected", "shell", "network"] {
        let mut value = serde_json::to_value(&proposal).unwrap();
        value[field] = serde_json::json!(true);
        assert!(serde_json::from_value::<StructuredTestProposalV1>(value).is_err());
    }
    let encoded = serde_json::to_string(&proposal).unwrap();
    let duplicate = encoded.replacen(
        "\"id\":\"proposal-1\"",
        "\"id\":\"proposal-1\",\"id\":\"proposal-2\"",
        1,
    );
    assert!(serde_json::from_str::<StructuredTestProposalV1>(&duplicate).is_err());
    let mut holdout = proposal;
    holdout.data_use = DataUse::AcceptanceEpoch;
    assert!(holdout.validate().is_err());
    assert_ne!(fingerprint(&holdout).unwrap(), d("untrusted"));
}
