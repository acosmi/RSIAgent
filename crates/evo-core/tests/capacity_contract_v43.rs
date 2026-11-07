use evo_core::Error;
use evo_core::capacity::{CapacityConflict, CapacityItem as I, CapacityScope as S};

#[test]
fn registered_limits_pin_all_32_items_and_owner_scopes() {
    let expected = [
        (I::Run, 1000, S::Instance),
        (I::Event, 10000, S::Instance),
        (I::Skill, 1000, S::Instance),
        (I::InflightPrepare, 5, S::Instance),
        (I::ExplorationNode, 500, S::Instance),
        (I::ReplayWorld, 100, S::Instance),
        (I::ActiveLease, 10, S::Instance),
        (I::StagingPackage, 20, S::Instance),
        (I::ConcurrentManagementDispatch, 1, S::Instance),
        (I::DevelopmentCycle, 1000, S::Instance),
        (I::ConsolidationClaim, 250, S::Instance),
        (I::ProposalPerClaim, 1, S::Claim),
        (I::StagingBindingPerClaim, 1, S::Claim),
        (I::PracticeAttemptSet, 200, S::Instance),
        (I::MetaTrial, 10, S::Instance),
        (I::ExplorationWorld, 100, S::Instance),
        (I::OptimizationHistory, 1000, S::Instance),
        (I::ConcurrentOptimizationJob, 1, S::Instance),
        (I::SkillGroupPerJob, 2, S::OptimizationJob),
        (I::CurriculumProbeJob, 100, S::Instance),
        (I::ReplayEconomicExperiment, 100, S::Instance),
        (I::ManagementJobRecord, 10000, S::Instance),
        (I::FailureRecord, 10000, S::Instance),
        (I::SelectionRecord, 1000, S::Instance),
        (I::RepairTemplate, 100, S::Instance),
        (I::ResetReceiptPerNode, 1, S::ExplorationNode),
        (I::ExternalService, 50, S::Instance),
        (I::RootStagePlanVersionPerRoot, 100, S::Root),
        (I::HostSurface, 100, S::Instance),
        (I::DirectionHint, 100, S::Instance),
        (I::TraceExport, 1000, S::Instance),
        (I::ImproverRelease, 100, S::Instance),
    ];
    assert_eq!(I::ALL, expected.map(|(item, _, _)| item));
    for (item, limit, scope) in expected {
        assert_eq!(item.limit(), limit);
        assert_eq!(item.scope(), scope);
    }
}

#[test]
fn whole_intent_admission_checks_registered_deltas_and_real_boundaries() {
    for item in I::ALL {
        let limit = item.limit();
        item.admit(limit - 1, 1).unwrap();
        item.admit(limit, 0).unwrap();
        item.admit(limit + 1, 0).unwrap();
        for (used, delta) in [(limit, 1), (limit - 1, 2), (0, limit + 1)] {
            match item.admit(used, delta) {
                Err(Error::Capacity(detail)) => assert_eq!(
                    detail,
                    CapacityConflict {
                        item,
                        used,
                        limit,
                        delta
                    }
                ),
                other => panic!("{item} must refuse the entire delta: {other:?}"),
            }
        }
    }
}

#[test]
fn typed_capacity_error_roundtrips_item_used_limit_and_delta() {
    let detail = CapacityConflict {
        item: I::Skill,
        used: 1000,
        limit: 1000,
        delta: 1,
    };
    let raw = serde_json::to_value(&detail).unwrap();
    assert_eq!(
        raw,
        serde_json::json!({"item":"skills","used":1000,"limit":1000,"delta":1})
    );
    assert_eq!(
        serde_json::from_value::<CapacityConflict>(raw.clone()).unwrap(),
        detail
    );
    assert_eq!(
        Error::Capacity(detail).to_string(),
        "capacity_conflict item=skills used=1000 limit=1000 delta=1"
    );
    let mut unknown = raw;
    unknown["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<CapacityConflict>(unknown).is_err());
}

/// The engine and storage re-exports and the group bound are pinned to these
/// names by the engine's own capacity unit test; core cannot depend on them.
#[test]
fn legacy_constants_reexport_the_single_authority() {
    use evo_core::capacity::*;
    assert_eq!(
        evo_core::curriculum::MAX_FAILURE_RECORDS as u64,
        I::FailureRecord.limit()
    );
    for (item, constant) in [
        (I::Run, MAX_RUNS),
        (I::Event, MAX_EVENTS),
        (I::Skill, MAX_SKILLS),
        (I::InflightPrepare, MAX_PREPARE),
        (I::ExplorationNode, MAX_EXPLORATION_NODES),
        (I::ReplayWorld, MAX_REPLAY_WORLDS),
        (I::ActiveLease, MAX_ACTIVE_LEASES),
        (I::StagingPackage, MAX_STAGED_PACKAGES),
        (I::ConcurrentManagementDispatch, MAX_CONCURRENT_DISPATCH),
        (I::SkillGroupPerJob, MAX_SKILL_GROUPS_PER_JOB),
    ] {
        assert_eq!(constant, item.limit());
    }
}
