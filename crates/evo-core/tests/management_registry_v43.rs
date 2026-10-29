//! AG-136 E acceptance: an independent transcription of the plan §6.1.2
//! registration table (L811–L842, v4.5 `f6f9b1e4…`) cross-checked against the
//! single-source registry. This fixture is acceptance input written from the
//! truth table itself — it is not derived from `management_registry` and is
//! never a second runtime fact source.
use evo_core::contract::{MODEL_TOOLS, admin_ops, reject_admin_as_model_tool};
use evo_core::management_registry::{
    ExecutionKind, MANAGE_ROUTE_PREFIX, MANAGEMENT_CATALOG, PaidPolicy, Plane, REVIEW_OPERATION,
    REVIEW_ROUTE, RoleRequirement, all_names, dispatch_names, formal_names, lookup, manage_route,
    offline_names, review_operation,
};

/// One row of the independent source-table fixture: the truth table line, its
/// concrete names in table order, and the per-name typed expectations.
struct ExpectedOp {
    name: &'static str,
    line: u16,
    plane: Plane,
    role: RoleRequirement,
    paid: PaidPolicy,
    execution: ExecutionKind,
    clause: &'static str,
}

fn e(
    name: &'static str,
    line: u16,
    plane: Plane,
    role: RoleRequirement,
    paid: PaidPolicy,
    execution: ExecutionKind,
    clause: &'static str,
) -> ExpectedOp {
    ExpectedOp {
        name,
        line,
        plane,
        role,
        paid,
        execution,
        clause,
    }
}

/// Independent transcription of L811–L842 plus the one named control action.
fn source_table() -> Vec<ExpectedOp> {
    use ExecutionKind as X;
    use PaidPolicy as P;
    use Plane as Pl;
    use RoleRequirement as R;
    vec![
        // L811 | experiment.register；evaluation.start/status | dispatch | Evaluator | 否（占查询/alpha额度） | E01/E05；§6.6
        e(
            "experiment.register",
            811,
            Pl::Dispatch,
            R::Evaluator,
            P::Never,
            X::AsyncJob,
            "E01/E05；§6.6",
        ),
        e(
            "evaluation.start",
            811,
            Pl::Dispatch,
            R::Evaluator,
            P::Never,
            X::AsyncJob,
            "E01/E05；§6.6",
        ),
        e(
            "evaluation.status",
            811,
            Pl::Dispatch,
            R::Evaluator,
            P::Never,
            X::AsyncJob,
            "E01/E05；§6.6",
        ),
        // L812 | exploration.start；replay.run；meta.start | dispatch | Admin | 否 | E09/E10/E14；§6.1
        e(
            "exploration.start",
            812,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E09/E10/E14；§6.1",
        ),
        e(
            "replay.run",
            812,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E09/E10/E14；§6.1",
        ),
        e(
            "meta.start",
            812,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E09/E10/E14；§6.1",
        ),
        // L813 | curriculum.step | dispatch | Admin | 提案生成为是（占§8课程额度） | E12；§8.1
        e(
            "curriculum.step",
            813,
            Pl::Dispatch,
            R::Admin,
            P::Conditional("提案生成为是（占§8课程额度）"),
            X::AsyncJob,
            "E12；§8.1",
        ),
        // L814 | job.status；job.cancel | dispatch | 作业属主 | 否 | §6.1.1
        e(
            "job.status",
            814,
            Pl::Dispatch,
            R::JobOwner,
            P::Never,
            X::JobLifecycle,
            "§6.1.1",
        ),
        e(
            "job.cancel",
            814,
            Pl::Dispatch,
            R::JobOwner,
            P::Never,
            X::JobLifecycle,
            "§6.1.1",
        ),
        // L815 | exploration.step | dispatch | Admin | 是 | E09；§7.1–§7.2.3
        e(
            "exploration.step",
            815,
            Pl::Dispatch,
            R::Admin,
            P::Always,
            X::AsyncJob,
            "E09；§7.1–§7.2.3",
        ),
        // L816 | optimization.step | dispatch | Admin | 是 | E03/E07；§6.7
        e(
            "optimization.step",
            816,
            Pl::Dispatch,
            R::Admin,
            P::Always,
            X::AsyncJob,
            "E03/E07；§6.7",
        ),
        // L817 | practice.register/run；skill_group.run | dispatch | Admin | run为是 | E09/E12；§7.7、§8.5
        e(
            "practice.register",
            817,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E09/E12；§7.7、§8.5",
        ),
        e(
            "practice.run",
            817,
            Pl::Dispatch,
            R::Admin,
            P::Always,
            X::AsyncJob,
            "E09/E12；§7.7、§8.5",
        ),
        e(
            "skill_group.run",
            817,
            Pl::Dispatch,
            R::Admin,
            P::Always,
            X::AsyncJob,
            "E09/E12；§7.7、§8.5",
        ),
        // L818 | development.control.register | dispatch | Admin（不得为executor或grader） | 否 | E03/E09
        e(
            "development.control.register",
            818,
            Pl::Dispatch,
            R::AdminNotExecutorOrGrader,
            P::Never,
            X::AsyncJob,
            "E03/E09",
        ),
        // L819 | development.probe_compose.enable/disable | dispatch | Admin | 否 | E09；§6.7.5
        e(
            "development.probe_compose.enable",
            819,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E09；§6.7.5（v4.5补充，Q-46）",
        ),
        e(
            "development.probe_compose.disable",
            819,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E09；§6.7.5（v4.5补充，Q-46）",
        ),
        // L820 | consolidation.trigger；consolidation.run | dispatch | Admin | trigger否、run是 | E13；§11.5
        e(
            "consolidation.trigger",
            820,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E13；§11.5",
        ),
        e(
            "consolidation.run",
            820,
            Pl::Dispatch,
            R::Admin,
            P::Always,
            X::AsyncJob,
            "E13；§11.5",
        ),
        // L821 | curriculum.register/record_cycle/proposal/select/record_applied_asset | dispatch | Admin | 否 | E12；§8.1.1、§8.4.1
        e(
            "curriculum.register",
            821,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E12；§8.1.1、§8.4.1",
        ),
        e(
            "curriculum.record_cycle",
            821,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E12；§8.1.1、§8.4.1",
        ),
        e(
            "curriculum.proposal",
            821,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E12；§8.1.1、§8.4.1",
        ),
        e(
            "curriculum.select",
            821,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E12；§8.1.1、§8.4.1",
        ),
        e(
            "curriculum.record_applied_asset",
            821,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E12；§8.1.1、§8.4.1",
        ),
        // L822 | monitoring.environment.record；monitoring.drift.record | dispatch | Host/Admin；Admin | 否 | E13
        e(
            "monitoring.environment.record",
            822,
            Pl::Dispatch,
            R::HostOrAdmin,
            P::Never,
            X::AsyncJob,
            "E13",
        ),
        e(
            "monitoring.drift.record",
            822,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E13",
        ),
        // L823 | source.revoke；cleanup.continue | dispatch（清理另由启动流程续跑） | Admin（失败重试仅Admin） | 否 | E08；§11.3.1
        e(
            "source.revoke",
            823,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E08；§11.3.1",
        ),
        e(
            "cleanup.continue",
            823,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E08；§11.3.1",
        ),
        // L824 | budget.* | dispatch | Admin | 否（授权与记账） | E04；§3.5.1、§6.6
        e(
            "budget.root.authorize",
            824,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E04；§3.5.1、§6.6",
        ),
        e(
            "budget.stage_plan.bind",
            824,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E04；§3.5.1、§6.6",
        ),
        e(
            "budget.calls.list_open",
            824,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E04；§3.5.1、§6.6",
        ),
        e(
            "budget.call.reconcile",
            824,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E04；§3.5.1、§6.6",
        ),
        // L825 | host_surface.register | dispatch或启动流程 | Admin | 否 | E02/E16.4；§5.5.1
        e(
            "host_surface.register",
            825,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E02/E16.4；§5.5.1",
        ),
        // L826 | import.register/execute | dispatch | Admin | 否 | E16.1；§6.3
        e(
            "import.register",
            826,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.1；§6.3",
        ),
        e(
            "import.execute",
            826,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.1；§6.3",
        ),
        // L827 | asset.stage/abort/handoff；export.prepare/complete/abort；trace.export | dispatch | Admin | 否 | E16.1/E16.2；§11.2–§11.2.1
        e(
            "asset.stage",
            827,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.1/E16.2；§11.2–§11.2.1",
        ),
        e(
            "asset.abort",
            827,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.1/E16.2；§11.2–§11.2.1",
        ),
        e(
            "asset.handoff",
            827,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.1/E16.2；§11.2–§11.2.1",
        ),
        e(
            "export.prepare",
            827,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.1/E16.2；§11.2–§11.2.1",
        ),
        e(
            "export.complete",
            827,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.1/E16.2；§11.2–§11.2.1",
        ),
        e(
            "export.abort",
            827,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.1/E16.2；§11.2–§11.2.1",
        ),
        e(
            "trace.export",
            827,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.1/E16.2；§11.2–§11.2.1",
        ),
        // L828 | seed.install/reset_stage | dispatch | Admin | 否 | E16.3；§11.1.1
        e(
            "seed.install",
            828,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.3；§11.1.1",
        ),
        e(
            "seed.reset_stage",
            828,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E16.3；§11.1.1",
        ),
        // L829 | replay_economics.register/start/status/cancel | dispatch | Evaluator | 否（执行时为是） | E11
        e(
            "replay_economics.register",
            829,
            Pl::Dispatch,
            R::Evaluator,
            P::Conditional("执行时为是"),
            X::AsyncJob,
            "E11",
        ),
        e(
            "replay_economics.start",
            829,
            Pl::Dispatch,
            R::Evaluator,
            P::Conditional("执行时为是"),
            X::AsyncJob,
            "E11",
        ),
        e(
            "replay_economics.status",
            829,
            Pl::Dispatch,
            R::Evaluator,
            P::Conditional("执行时为是"),
            X::AsyncJob,
            "E11",
        ),
        e(
            "replay_economics.cancel",
            829,
            Pl::Dispatch,
            R::Evaluator,
            P::Conditional("执行时为是"),
            X::AsyncJob,
            "E11",
        ),
        // L830 | backup.create | dispatch | Admin | 否 | E08/E16.5
        e(
            "backup.create",
            830,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E08/E16.5",
        ),
        // L831 | formal.host.bootstrap；formal.provider.register；formal.meter.register；evaluation.control.register | 正式权威平面 | Admin（控制登记为Evaluator） | 否 | E04/E05；§4.2
        e(
            "formal.host.bootstrap",
            831,
            Pl::FormalAuthority,
            R::Admin,
            P::Never,
            X::FormalCommand,
            "E04/E05；§4.2",
        ),
        e(
            "formal.provider.register",
            831,
            Pl::FormalAuthority,
            R::Admin,
            P::Never,
            X::FormalCommand,
            "E04/E05；§4.2",
        ),
        e(
            "formal.meter.register",
            831,
            Pl::FormalAuthority,
            R::Admin,
            P::Never,
            X::FormalCommand,
            "E04/E05；§4.2",
        ),
        e(
            "evaluation.control.register",
            831,
            Pl::FormalAuthority,
            R::Evaluator,
            P::Never,
            X::FormalCommand,
            "E04/E05；§4.2",
        ),
        // L832 | release.approve/canary/activate/rollback | 正式权威平面 | Admin，且符合§4批准主体分离 | 否 | E06；§6.2
        e(
            "release.approve",
            832,
            Pl::FormalAuthority,
            R::AdminWithApprovalSubjectSeparation,
            P::Never,
            X::FormalCommand,
            "E06；§6.2",
        ),
        e(
            "release.canary",
            832,
            Pl::FormalAuthority,
            R::AdminWithApprovalSubjectSeparation,
            P::Never,
            X::FormalCommand,
            "E06；§6.2",
        ),
        e(
            "release.activate",
            832,
            Pl::FormalAuthority,
            R::AdminWithApprovalSubjectSeparation,
            P::Never,
            X::FormalCommand,
            "E06；§6.2",
        ),
        e(
            "release.rollback",
            832,
            Pl::FormalAuthority,
            R::AdminWithApprovalSubjectSeparation,
            P::Never,
            X::FormalCommand,
            "E06；§6.2",
        ),
        // L833 | formal.execute/grade/cost/capture_cost/finalize | 正式权威平面 | 按命令固定为Host或Evaluator | execute为是；grade调用计价的外部grader时为是 | E04/E05
        e(
            "formal.execute",
            833,
            Pl::FormalAuthority,
            R::FixedFormalHost,
            P::Always,
            X::FormalCommand,
            "E04/E05",
        ),
        e(
            "formal.grade",
            833,
            Pl::FormalAuthority,
            R::FixedFormalEvaluator,
            P::Conditional("调用计价的外部grader时为是（formal_evaluation阶段）"),
            X::FormalCommand,
            "E04/E05",
        ),
        e(
            "formal.cost",
            833,
            Pl::FormalAuthority,
            R::FixedFormalHost,
            P::Never,
            X::FormalCommand,
            "E04/E05",
        ),
        e(
            "formal.capture_cost",
            833,
            Pl::FormalAuthority,
            R::FixedFormalHost,
            P::Never,
            X::FormalCommand,
            "E04/E05",
        ),
        e(
            "formal.finalize",
            833,
            Pl::FormalAuthority,
            R::FixedFormalEvaluator,
            P::Never,
            X::FormalCommand,
            "E04/E05",
        ),
        // L834 | repair_template.register | dispatch | Admin | 否 | E09；§7.2.3
        e(
            "repair_template.register",
            834,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E09；§7.2.3",
        ),
        // L835 | service.register | dispatch | Admin | 否 | E04；§4.2
        e(
            "service.register",
            835,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E04；§4.2",
        ),
        // L836 | direction_hint.register | dispatch | Admin | 否 | E11；§9.1.1
        e(
            "direction_hint.register",
            836,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E11；§9.1.1",
        ),
        // L837 | safe_baseline.register | dispatch | Admin | 否 | E02；§5.3、§5.6
        e(
            "safe_baseline.register",
            837,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E02；§5.3、§5.6",
        ),
        // L838 | routing.preference.decide；routing.capability.review；diagnostic.resolve | dispatch | Admin | 否 | E03；§6.4
        e(
            "routing.preference.decide",
            838,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E03；§6.4",
        ),
        e(
            "routing.capability.review",
            838,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E03；§6.4",
        ),
        e(
            "diagnostic.resolve",
            838,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E03；§6.4",
        ),
        // L839 | emergency.stop | dispatch | Admin | 否 | E06/E13；§3.3、§11
        e(
            "emergency.stop",
            839,
            Pl::Dispatch,
            R::Admin,
            P::Never,
            X::AsyncJob,
            "E06/E13；§3.3、§11",
        ),
        // L840 | ticket.void；holdout.epoch.close；anchor.version.register | dispatch | Evaluator（独立评测负责人） | 否 | E01/E05；§3.2.1–§3.2.3
        e(
            "ticket.void",
            840,
            Pl::Dispatch,
            R::Evaluator,
            P::Never,
            X::AsyncJob,
            "E01/E05；§3.2.1–§3.2.3",
        ),
        e(
            "holdout.epoch.close",
            840,
            Pl::Dispatch,
            R::Evaluator,
            P::Never,
            X::AsyncJob,
            "E01/E05；§3.2.1–§3.2.3",
        ),
        e(
            "anchor.version.register",
            840,
            Pl::Dispatch,
            R::Evaluator,
            P::Never,
            X::AsyncJob,
            "E01/E05；§3.2.1–§3.2.3",
        ),
        // L841 | 控制面登记与审阅（闭合族）| dispatch | 按所引条款 | 否 | §4；§6.5 L945–947；E06 L1738–1752
        e(
            "control.review",
            841,
            Pl::Dispatch,
            R::TrustedGatewayAdmin,
            P::Never,
            X::SyncReadonlyView,
            "§6.1.2 L804；§4；§6.5 L945–947；E06 L1738–1752",
        ),
        // L842 | restore | 离线脚本 | 操作系统运维人员 | 否 | E08；V018
        e(
            "restore",
            842,
            Pl::OfflineScript,
            R::OperatingSystemOperator,
            P::Never,
            X::OfflineScript,
            "E08；V018",
        ),
    ]
}

/// The fixture and the registry agree name-by-name, in the same table order,
/// on every typed field (E: 逐名 role/paid/clause/syncview).
#[test]
fn registry_matches_the_independent_source_table_exactly() {
    let expected = source_table();
    assert_eq!(expected.len(), 75, "fixture is 74+1");
    assert_eq!(MANAGEMENT_CATALOG.len(), expected.len());
    for (actual, expected) in MANAGEMENT_CATALOG.iter().zip(&expected) {
        assert_eq!(actual.name, expected.name, "name/order drift");
        assert_eq!(actual.table_line, expected.line, "{}", actual.name);
        assert_eq!(actual.plane, expected.plane, "{}", actual.name);
        assert_eq!(actual.role, expected.role, "{}", actual.name);
        assert_eq!(actual.paid, expected.paid, "{}", actual.name);
        assert_eq!(actual.execution, expected.execution, "{}", actual.name);
        assert_eq!(actual.clause, expected.clause, "{}", actual.name);
        assert!(!actual.clause.is_empty(), "{}", actual.name);
    }
}

/// The counts the card pins: 74+1 names over 32 families, 61/13/1 planes.
#[test]
fn counts_are_72_plus_1_over_31_families_and_59_13_1_planes() {
    assert_eq!(all_names().len(), 75);
    assert_eq!(dispatch_names().len(), 61);
    assert_eq!(formal_names().len(), 13);
    assert_eq!(offline_names().len(), 1);
    assert_eq!(admin_ops(), dispatch_names());

    // 72 original names + the single control.review.
    let original = all_names()
        .iter()
        .filter(|name| {
            !matches!(
                **name,
                REVIEW_OPERATION
                    | "development.probe_compose.enable"
                    | "development.probe_compose.disable"
            )
        })
        .count();
    assert_eq!(original, 72);

    // 32 distinct table rows = 32 families, exactly L811..=L842.
    let mut lines: Vec<u16> = MANAGEMENT_CATALOG.iter().map(|op| op.table_line).collect();
    lines.sort_unstable();
    lines.dedup();
    assert_eq!(lines.len(), 32);
    assert_eq!(lines.first(), Some(&811));
    assert_eq!(lines.last(), Some(&842));
    let expected_lines: Vec<u16> = (811..=842).collect();
    assert_eq!(lines, expected_lines);

    // The projections partition the catalog and are duplicate-free.
    let mut union: Vec<&str> = dispatch_names()
        .iter()
        .chain(formal_names())
        .chain(offline_names())
        .copied()
        .collect();
    union.sort_unstable();
    let mut all = all_names().to_vec();
    all.sort_unstable();
    assert_eq!(union, all);
    let before = all.len();
    all.dedup();
    assert_eq!(all.len(), before, "duplicate registered name");

    // The closed control family (L841) registers exactly one action; it is
    // not a wildcard for arbitrary control.* names.
    let control_family: Vec<_> = MANAGEMENT_CATALOG
        .iter()
        .filter(|op| op.table_line == 841)
        .collect();
    assert_eq!(control_family.len(), 1);
    assert_eq!(control_family[0].name, REVIEW_OPERATION);
}

/// The view name/route projection: one sync view, route concat-derived from
/// the same registered name.
#[test]
fn review_projection_is_the_single_concat_derived_sync_view() {
    assert_eq!(REVIEW_OPERATION, "control.review");
    assert_eq!(REVIEW_ROUTE, "/v1/manage/control.review");
    assert_eq!(
        REVIEW_ROUTE,
        format!("{}{}", MANAGE_ROUTE_PREFIX, REVIEW_OPERATION)
    );
    assert_eq!(manage_route(REVIEW_OPERATION), REVIEW_ROUTE);
    assert_eq!(
        manage_route("evaluation.start"),
        "/v1/manage/evaluation.start"
    );

    let review = review_operation();
    assert_eq!(review.name, REVIEW_OPERATION);
    assert_eq!(review.plane, Plane::Dispatch);
    assert_eq!(review.execution, ExecutionKind::SyncReadonlyView);
    assert_eq!(review.role, RoleRequirement::TrustedGatewayAdmin);
    assert_eq!(review.paid, PaidPolicy::Never);
    assert_eq!(review.table_line, 841);

    let views = MANAGEMENT_CATALOG
        .iter()
        .filter(|op| op.execution == ExecutionKind::SyncReadonlyView)
        .count();
    assert_eq!(views, 1, "exactly one registered sync view");
}

/// Conditional paid policies are preserved as conditions, never flattened:
/// the table's split rows keep their per-name split.
#[test]
fn conditional_paid_policies_are_not_flattened() {
    let paid = |name: &str| lookup(name).unwrap().paid;
    assert_eq!(paid("consolidation.trigger"), PaidPolicy::Never);
    assert_eq!(paid("consolidation.run"), PaidPolicy::Always);
    assert_eq!(paid("practice.register"), PaidPolicy::Never);
    assert_eq!(paid("practice.run"), PaidPolicy::Always);
    assert_eq!(paid("skill_group.run"), PaidPolicy::Always);
    assert_eq!(
        paid("curriculum.step"),
        PaidPolicy::Conditional("提案生成为是（占§8课程额度）")
    );
    assert_eq!(
        paid("replay_economics.start"),
        PaidPolicy::Conditional("执行时为是")
    );
    assert_eq!(paid("formal.execute"), PaidPolicy::Always);
    assert_eq!(
        paid("formal.grade"),
        PaidPolicy::Conditional("调用计价的外部grader时为是（formal_evaluation阶段）")
    );
    assert_eq!(paid("formal.cost"), PaidPolicy::Never);
    assert_eq!(paid("exploration.step"), PaidPolicy::Always);
    assert_eq!(paid("optimization.step"), PaidPolicy::Always);
    assert_eq!(paid("control.review"), PaidPolicy::Never);
    assert_eq!(paid("restore"), PaidPolicy::Never);
}

/// V100.a closure at the contract boundary: all 75 registered names are
/// rejected as model tools, the four model tools are untouched and disjoint,
/// and unknown names stay unknown.
#[test]
fn model_tool_boundary_over_the_full_catalog() {
    assert_eq!(MODEL_TOOLS.len(), 4);
    for &name in all_names() {
        let error = reject_admin_as_model_tool(name).unwrap_err();
        assert!(matches!(error, evo_core::Error::Forbidden), "{name}");
        assert!(!MODEL_TOOLS.contains(&name), "{name}");
    }
    for tool in MODEL_TOOLS {
        assert!(reject_admin_as_model_tool(tool).is_ok(), "{tool}");
        assert!(lookup(tool).is_none(), "{tool}");
    }
    for name in [
        "budget.grant",
        "control",
        "control.",
        "control.deploy",
        "",
        "restore.bak",
    ] {
        let error = reject_admin_as_model_tool(name).unwrap_err();
        assert!(
            matches!(error, evo_core::Error::Invalid(ref message) if message == "unknown model tool"),
            "{name:?}: {error:?}"
        );
    }
}

/// Off-table dynamic names and near misses are unknown to the registry.
#[test]
fn off_table_names_are_unknown_to_the_registry() {
    for name in [
        "evaluation.startt",
        "Evaluation.start",
        " evaluation.start",
        "evaluation.start ",
        "control.review ",
        "control.*",
        "control",
        "*",
        "job.status/cancel",
        "restore --force",
    ] {
        assert!(lookup(name).is_none(), "{name:?}");
    }
}

#[test]
fn probe_compose_pair_is_registered_without_enabling_or_changing_existing_metadata() {
    let names = [
        "development.probe_compose.enable",
        "development.probe_compose.disable",
    ];
    for name in names {
        let entry = lookup(name).unwrap();
        assert_eq!(entry.table_line, 819);
        assert_eq!(entry.plane, Plane::Dispatch);
        assert_eq!(entry.role, RoleRequirement::Admin);
        assert_eq!(entry.paid, PaidPolicy::Never);
        assert_eq!(entry.execution, ExecutionKind::AsyncJob);
        assert_eq!(entry.clause, "E09；§6.7.5（v4.5补充，Q-46）");
        assert!(dispatch_names().contains(&name));
        assert!(!MODEL_TOOLS.contains(&name));
    }
    let position = all_names()
        .iter()
        .position(|name| *name == names[0])
        .unwrap();
    assert_eq!(all_names()[position - 1], "development.control.register");
    assert_eq!(all_names()[position + 1], names[1]);
    assert_eq!(all_names()[position + 2], "consolidation.trigger");
    let old_expected: Vec<_> = source_table()
        .into_iter()
        .filter(|entry| !names.contains(&entry.name))
        .collect();
    let old_actual: Vec<_> = MANAGEMENT_CATALOG
        .iter()
        .filter(|entry| !names.contains(&entry.name))
        .collect();
    assert_eq!(old_expected.len(), 73);
    assert_eq!(old_actual.len(), old_expected.len());
    for (actual, expected) in old_actual.iter().zip(old_expected) {
        assert_eq!(
            (
                actual.name,
                actual.table_line,
                actual.plane,
                actual.role,
                actual.paid,
                actual.execution,
                actual.clause
            ),
            (
                expected.name,
                expected.line,
                expected.plane,
                expected.role,
                expected.paid,
                expected.execution,
                expected.clause
            )
        );
    }
}
