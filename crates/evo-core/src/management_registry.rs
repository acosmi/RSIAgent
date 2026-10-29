//! The single typed management-operation registry (plan v4.5 §6.1.2 L795–854,
//! V100.a/e L2374–2382).
//!
//! One authoring constant (`MANAGEMENT_CATALOG`) registers every concrete
//! operation name of the §6.1.2 table L811–L842 plus the one named action of
//! the closed control family (`control.review`, cited per §4, §6.5 L945–947
//! and E06 L1738–1752). All name projections — `all_names`, `dispatch_names`,
//! `formal_names`, `offline_names`, the review name and its route — derive
//! from that one catalog, so there is no second fact table that can drift.
//!
//! This module is pure static registration metadata. It grants no authority,
//! creates no handler, job, budget or persistence, and does not claim that any
//! runtime consumes it: the existing seven dispatch handlers, the formal
//! authority commands and the offline restore keep their own real guards.

/// Execution plane of a registered operation (§6.1.2 plane notes L844–854).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Plane {
    /// `rsia serve` `/v1/manage/*` and `rsia manage`.
    Dispatch,
    /// `formal_local_host` Operator/Authority channel and trusted supervisor
    /// commands; never a generic manage call.
    FormalAuthority,
    /// Read-only backup and new-directory restore script.
    OfflineScript,
}

/// Who may execute a registered operation, per its table row. The variants
/// keep the table's conditions typed; none is flattened to a role list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoleRequirement {
    /// Explicit Admin.
    Admin,
    /// The registered independent evaluation role.
    Evaluator,
    /// monitoring.environment.record: Host or Admin.
    HostOrAdmin,
    /// job.status/job.cancel: the job's owner identities (namespace, actor and
    /// role as persisted with the job), not a role class.
    JobOwner,
    /// development.control.register: Admin who is neither executor nor grader.
    AdminNotExecutorOrGrader,
    /// release.approve/canary/activate/rollback: Admin and the §4
    /// approval-subject separation.
    AdminWithApprovalSubjectSeparation,
    /// Formal-authority command fixed by the command to Host.
    FixedFormalHost,
    /// Formal-authority command fixed by the command to Evaluator.
    FixedFormalEvaluator,
    /// control.review: the Admin of this gateway's trusted namespace, per the
    /// cited clauses (§4, §6.5 L945–947, E06 L1738–1752).
    TrustedGatewayAdmin,
    /// restore: the operating-system operator, offline.
    OperatingSystemOperator,
}

/// Whether a registered operation spends money. Conditional rows keep their
/// condition; nothing is flattened to a bool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaidPolicy {
    /// 否 — never dispatches a paid call.
    Never,
    /// 是 — the operation itself dispatches paid work.
    Always,
    /// Paid only under the stated table condition (verbatim).
    Conditional(&'static str),
}

/// How a registered operation executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExecutionKind {
    /// Dispatch-plane async operation: submit persists and returns a job id
    /// (§6.1.2 L799). Registered names without a handler are refused before
    /// payload parsing or persistence.
    AsyncJob,
    /// job.status/job.cancel: dedicated lifecycle routes over existing jobs
    /// (§6.1.1), never a new job.
    JobLifecycle,
    /// The one registered synchronous read-only control-plane view: reads
    /// existing governance facts and creates no job, private input, audit row,
    /// budget call or model call (§6.1.2 L804, §6.5 L945–947, E06 L1738–1752).
    SyncReadonlyView,
    /// Formal-authority command over its own channel (§4.2, E04/E05/E06).
    FormalCommand,
    /// Offline restore script (E08, V018).
    OfflineScript,
}

/// One registered operation with its table metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagementOperation {
    /// The exact operation name (`family.action`); the minimal code identifier.
    pub name: &'static str,
    /// The §6.1.2 table row this name is registered under (L811–L842); the
    /// 32 distinct rows are the 32 families.
    pub table_line: u16,
    pub plane: Plane,
    pub role: RoleRequirement,
    pub paid: PaidPolicy,
    pub execution: ExecutionKind,
    /// The row's responsibility/clause cell; the control family cites the
    /// clauses its action is required by (V100.a). Never empty.
    pub clause: &'static str,
}

/// The route prefix of the dispatch plane (§6.1.2 plane note L845).
pub const MANAGE_ROUTE_PREFIX: &str = "/v1/manage/";

/// The one named action of the closed control family (family.action).
pub const REVIEW_OPERATION: &str = "control.review";

/// The concrete view route, concat-constructed from the registered name.
pub const REVIEW_ROUTE: &str = concat!("/v1/manage/", "control.review");

macro_rules! op {
    ($name:literal, $line:literal, $plane:ident, $role:ident, $paid:expr, $exec:ident, $clause:literal) => {
        ManagementOperation {
            name: $name,
            table_line: $line,
            plane: Plane::$plane,
            role: RoleRequirement::$role,
            paid: $paid,
            execution: ExecutionKind::$exec,
            clause: $clause,
        }
    };
}

/// The single authoring catalog: every concrete name of §6.1.2 table
/// L811–L842 in table order, then the control family's one named action.
pub const MANAGEMENT_CATALOG: &[ManagementOperation] = &[
    // L811 experiment.register；evaluation.start/status
    op!(
        "experiment.register",
        811,
        Dispatch,
        Evaluator,
        PaidPolicy::Never,
        AsyncJob,
        "E01/E05；§6.6"
    ),
    op!(
        "evaluation.start",
        811,
        Dispatch,
        Evaluator,
        PaidPolicy::Never,
        AsyncJob,
        "E01/E05；§6.6"
    ),
    op!(
        "evaluation.status",
        811,
        Dispatch,
        Evaluator,
        PaidPolicy::Never,
        AsyncJob,
        "E01/E05；§6.6"
    ),
    // L812 exploration.start；replay.run；meta.start
    op!(
        "exploration.start",
        812,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E09/E10/E14；§6.1"
    ),
    op!(
        "replay.run",
        812,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E09/E10/E14；§6.1"
    ),
    op!(
        "meta.start",
        812,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E09/E10/E14；§6.1"
    ),
    // L813 curriculum.step
    op!(
        "curriculum.step",
        813,
        Dispatch,
        Admin,
        PaidPolicy::Conditional("提案生成为是（占§8课程额度）"),
        AsyncJob,
        "E12；§8.1"
    ),
    // L814 job.status；job.cancel
    op!(
        "job.status",
        814,
        Dispatch,
        JobOwner,
        PaidPolicy::Never,
        JobLifecycle,
        "§6.1.1"
    ),
    op!(
        "job.cancel",
        814,
        Dispatch,
        JobOwner,
        PaidPolicy::Never,
        JobLifecycle,
        "§6.1.1"
    ),
    // L815 exploration.step（每次推进一个节点）
    op!(
        "exploration.step",
        815,
        Dispatch,
        Admin,
        PaidPolicy::Always,
        AsyncJob,
        "E09；§7.1–§7.2.3"
    ),
    // L816 optimization.step（G1单步内容优化）
    op!(
        "optimization.step",
        816,
        Dispatch,
        Admin,
        PaidPolicy::Always,
        AsyncJob,
        "E03/E07；§6.7"
    ),
    // L817 practice.register/run；skill_group.run — run为是
    op!(
        "practice.register",
        817,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E09/E12；§7.7、§8.5"
    ),
    op!(
        "practice.run",
        817,
        Dispatch,
        Admin,
        PaidPolicy::Always,
        AsyncJob,
        "E09/E12；§7.7、§8.5"
    ),
    op!(
        "skill_group.run",
        817,
        Dispatch,
        Admin,
        PaidPolicy::Always,
        AsyncJob,
        "E09/E12；§7.7、§8.5"
    ),
    // L818 development.control.register
    op!(
        "development.control.register",
        818,
        Dispatch,
        AdminNotExecutorOrGrader,
        PaidPolicy::Never,
        AsyncJob,
        "E03/E09"
    ),
    // L819 development.probe_compose.enable/disable (registration only).
    op!(
        "development.probe_compose.enable",
        819,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E09；§6.7.5（v4.5补充，Q-46）"
    ),
    op!(
        "development.probe_compose.disable",
        819,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E09；§6.7.5（v4.5补充，Q-46）"
    ),
    // L820 consolidation.trigger；consolidation.run — trigger否、run是
    op!(
        "consolidation.trigger",
        820,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E13；§11.5"
    ),
    op!(
        "consolidation.run",
        820,
        Dispatch,
        Admin,
        PaidPolicy::Always,
        AsyncJob,
        "E13；§11.5"
    ),
    // L821 curriculum.register/record_cycle/proposal/select/record_applied_asset
    op!(
        "curriculum.register",
        821,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E12；§8.1.1、§8.4.1"
    ),
    op!(
        "curriculum.record_cycle",
        821,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E12；§8.1.1、§8.4.1"
    ),
    op!(
        "curriculum.proposal",
        821,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E12；§8.1.1、§8.4.1"
    ),
    op!(
        "curriculum.select",
        821,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E12；§8.1.1、§8.4.1"
    ),
    op!(
        "curriculum.record_applied_asset",
        821,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E12；§8.1.1、§8.4.1"
    ),
    // L822 monitoring.environment.record（Host/Admin）；monitoring.drift.record（Admin）
    op!(
        "monitoring.environment.record",
        822,
        Dispatch,
        HostOrAdmin,
        PaidPolicy::Never,
        AsyncJob,
        "E13"
    ),
    op!(
        "monitoring.drift.record",
        822,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E13"
    ),
    // L823 source.revoke；cleanup.continue（清理另由启动流程续跑；失败重试仅Admin）
    op!(
        "source.revoke",
        823,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E08；§11.3.1"
    ),
    op!(
        "cleanup.continue",
        823,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E08；§11.3.1"
    ),
    // L824 budget.*（授权与记账）
    op!(
        "budget.root.authorize",
        824,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E04；§3.5.1、§6.6"
    ),
    op!(
        "budget.stage_plan.bind",
        824,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E04；§3.5.1、§6.6"
    ),
    op!(
        "budget.calls.list_open",
        824,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E04；§3.5.1、§6.6"
    ),
    op!(
        "budget.call.reconcile",
        824,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E04；§3.5.1、§6.6"
    ),
    // L825 host_surface.register（dispatch或启动流程）
    op!(
        "host_surface.register",
        825,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E02/E16.4；§5.5.1"
    ),
    // L826 import.register/execute
    op!(
        "import.register",
        826,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.1；§6.3"
    ),
    op!(
        "import.execute",
        826,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.1；§6.3"
    ),
    // L827 asset.stage/abort/handoff；export.prepare/complete/abort；trace.export
    op!(
        "asset.stage",
        827,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.1/E16.2；§11.2–§11.2.1"
    ),
    op!(
        "asset.abort",
        827,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.1/E16.2；§11.2–§11.2.1"
    ),
    op!(
        "asset.handoff",
        827,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.1/E16.2；§11.2–§11.2.1"
    ),
    op!(
        "export.prepare",
        827,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.1/E16.2；§11.2–§11.2.1"
    ),
    op!(
        "export.complete",
        827,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.1/E16.2；§11.2–§11.2.1"
    ),
    op!(
        "export.abort",
        827,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.1/E16.2；§11.2–§11.2.1"
    ),
    op!(
        "trace.export",
        827,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.1/E16.2；§11.2–§11.2.1"
    ),
    // L828 seed.install/reset_stage
    op!(
        "seed.install",
        828,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.3；§11.1.1"
    ),
    op!(
        "seed.reset_stage",
        828,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E16.3；§11.1.1"
    ),
    // L829 replay_economics.register/start/status/cancel — 否（执行时为是）
    op!(
        "replay_economics.register",
        829,
        Dispatch,
        Evaluator,
        PaidPolicy::Conditional("执行时为是"),
        AsyncJob,
        "E11"
    ),
    op!(
        "replay_economics.start",
        829,
        Dispatch,
        Evaluator,
        PaidPolicy::Conditional("执行时为是"),
        AsyncJob,
        "E11"
    ),
    op!(
        "replay_economics.status",
        829,
        Dispatch,
        Evaluator,
        PaidPolicy::Conditional("执行时为是"),
        AsyncJob,
        "E11"
    ),
    op!(
        "replay_economics.cancel",
        829,
        Dispatch,
        Evaluator,
        PaidPolicy::Conditional("执行时为是"),
        AsyncJob,
        "E11"
    ),
    // L830 backup.create
    op!(
        "backup.create",
        830,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E08/E16.5"
    ),
    // L831 正式权威平面：formal.host.bootstrap；formal.provider.register；
    // formal.meter.register；evaluation.control.register（控制登记为Evaluator）
    op!(
        "formal.host.bootstrap",
        831,
        FormalAuthority,
        Admin,
        PaidPolicy::Never,
        FormalCommand,
        "E04/E05；§4.2"
    ),
    op!(
        "formal.provider.register",
        831,
        FormalAuthority,
        Admin,
        PaidPolicy::Never,
        FormalCommand,
        "E04/E05；§4.2"
    ),
    op!(
        "formal.meter.register",
        831,
        FormalAuthority,
        Admin,
        PaidPolicy::Never,
        FormalCommand,
        "E04/E05；§4.2"
    ),
    op!(
        "evaluation.control.register",
        831,
        FormalAuthority,
        Evaluator,
        PaidPolicy::Never,
        FormalCommand,
        "E04/E05；§4.2"
    ),
    // L832 正式权威平面：release.approve/canary/activate/rollback（§4批准主体分离）
    op!(
        "release.approve",
        832,
        FormalAuthority,
        AdminWithApprovalSubjectSeparation,
        PaidPolicy::Never,
        FormalCommand,
        "E06；§6.2"
    ),
    op!(
        "release.canary",
        832,
        FormalAuthority,
        AdminWithApprovalSubjectSeparation,
        PaidPolicy::Never,
        FormalCommand,
        "E06；§6.2"
    ),
    op!(
        "release.activate",
        832,
        FormalAuthority,
        AdminWithApprovalSubjectSeparation,
        PaidPolicy::Never,
        FormalCommand,
        "E06；§6.2"
    ),
    op!(
        "release.rollback",
        832,
        FormalAuthority,
        AdminWithApprovalSubjectSeparation,
        PaidPolicy::Never,
        FormalCommand,
        "E06；§6.2"
    ),
    // L833 正式权威平面：formal.execute/grade/cost/capture_cost/finalize（按命令固定为Host或Evaluator）
    op!(
        "formal.execute",
        833,
        FormalAuthority,
        FixedFormalHost,
        PaidPolicy::Always,
        FormalCommand,
        "E04/E05"
    ),
    op!(
        "formal.grade",
        833,
        FormalAuthority,
        FixedFormalEvaluator,
        PaidPolicy::Conditional("调用计价的外部grader时为是（formal_evaluation阶段）"),
        FormalCommand,
        "E04/E05"
    ),
    op!(
        "formal.cost",
        833,
        FormalAuthority,
        FixedFormalHost,
        PaidPolicy::Never,
        FormalCommand,
        "E04/E05"
    ),
    op!(
        "formal.capture_cost",
        833,
        FormalAuthority,
        FixedFormalHost,
        PaidPolicy::Never,
        FormalCommand,
        "E04/E05"
    ),
    op!(
        "formal.finalize",
        833,
        FormalAuthority,
        FixedFormalEvaluator,
        PaidPolicy::Never,
        FormalCommand,
        "E04/E05"
    ),
    // L834 repair_template.register
    op!(
        "repair_template.register",
        834,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E09；§7.2.3"
    ),
    // L835 service.register（非正式用途的外部服务登记）
    op!(
        "service.register",
        835,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E04；§4.2"
    ),
    // L836 direction_hint.register
    op!(
        "direction_hint.register",
        836,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E11；§9.1.1"
    ),
    // L837 safe_baseline.register
    op!(
        "safe_baseline.register",
        837,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E02；§5.3、§5.6"
    ),
    // L838 routing.preference.decide；routing.capability.review；diagnostic.resolve
    op!(
        "routing.preference.decide",
        838,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E03；§6.4"
    ),
    op!(
        "routing.capability.review",
        838,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E03；§6.4"
    ),
    op!(
        "diagnostic.resolve",
        838,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E03；§6.4"
    ),
    // L839 emergency.stop（停用受影响能力）
    op!(
        "emergency.stop",
        839,
        Dispatch,
        Admin,
        PaidPolicy::Never,
        AsyncJob,
        "E06/E13；§3.3、§11"
    ),
    // L840 ticket.void；holdout.epoch.close；anchor.version.register（独立评测负责人）
    op!(
        "ticket.void",
        840,
        Dispatch,
        Evaluator,
        PaidPolicy::Never,
        AsyncJob,
        "E01/E05；§3.2.1–§3.2.3"
    ),
    op!(
        "holdout.epoch.close",
        840,
        Dispatch,
        Evaluator,
        PaidPolicy::Never,
        AsyncJob,
        "E01/E05；§3.2.1–§3.2.3"
    ),
    op!(
        "anchor.version.register",
        840,
        Dispatch,
        Evaluator,
        PaidPolicy::Never,
        AsyncJob,
        "E01/E05；§3.2.1–§3.2.3"
    ),
    // L841 控制面登记与审阅（闭合族；本卡登记的唯一命名动作，按所引条款）
    op!(
        "control.review",
        841,
        Dispatch,
        TrustedGatewayAdmin,
        PaidPolicy::Never,
        SyncReadonlyView,
        "§6.1.2 L804；§4；§6.5 L945–947；E06 L1738–1752"
    ),
    // L842 离线脚本：restore
    op!(
        "restore",
        842,
        OfflineScript,
        OperatingSystemOperator,
        PaidPolicy::Never,
        OfflineScript,
        "E08；V018"
    ),
];

// ---------------------------------------------------------------------------
// Derived projections. Every slice below is const-derived from
// MANAGEMENT_CATALOG alone; no name list is authored twice.
// ---------------------------------------------------------------------------
const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn plane_matches(entry: Plane, wanted: Plane) -> bool {
    matches!(
        (entry, wanted),
        (Plane::Dispatch, Plane::Dispatch)
            | (Plane::FormalAuthority, Plane::FormalAuthority)
            | (Plane::OfflineScript, Plane::OfflineScript)
    )
}

const fn count_plane(plane: Plane) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < MANAGEMENT_CATALOG.len() {
        if plane_matches(MANAGEMENT_CATALOG[i].plane, plane) {
            n += 1;
        }
        i += 1;
    }
    n
}

const fn project_plane<const N: usize>(plane: Plane) -> [&'static str; N] {
    let mut out = [""; N];
    let mut i = 0;
    let mut j = 0;
    while i < MANAGEMENT_CATALOG.len() {
        if plane_matches(MANAGEMENT_CATALOG[i].plane, plane) {
            out[j] = MANAGEMENT_CATALOG[i].name;
            j += 1;
        }
        i += 1;
    }
    if j != N {
        panic!("plane projection count drifted from the catalog");
    }
    out
}

const fn project_all_names<const N: usize>() -> [&'static str; N] {
    let mut out = [""; N];
    let mut i = 0;
    while i < MANAGEMENT_CATALOG.len() {
        out[i] = MANAGEMENT_CATALOG[i].name;
        i += 1;
    }
    out
}

/// Total registered concrete names: the 74 of the current table plus
/// `control.review`.
pub const REGISTERED_COUNT: usize = MANAGEMENT_CATALOG.len();

/// Distinct §6.1.2 table rows (the families); must be exactly 32.
pub const FAMILY_COUNT: usize = {
    let mut seen: [u16; 64] = [0; 64];
    let mut n = 0;
    let mut i = 0;
    while i < MANAGEMENT_CATALOG.len() {
        let line = MANAGEMENT_CATALOG[i].table_line;
        let mut found = false;
        let mut k = 0;
        while k < n {
            if seen[k] == line {
                found = true;
                break;
            }
            k += 1;
        }
        if !found {
            seen[n] = line;
            n += 1;
        }
        i += 1;
    }
    n
};

const DISPATCH_COUNT: usize = count_plane(Plane::Dispatch);
const FORMAL_COUNT: usize = count_plane(Plane::FormalAuthority);
const OFFLINE_COUNT: usize = count_plane(Plane::OfflineScript);

/// All 75 registered names in table order.
pub const ALL_NAMES: [&str; REGISTERED_COUNT] = project_all_names::<REGISTERED_COUNT>();
/// The 61 dispatch-plane names (60 table names + control.review); the projection
/// `contract::admin_ops` serves.
pub const DISPATCH_NAMES: [&str; DISPATCH_COUNT] = project_plane::<DISPATCH_COUNT>(Plane::Dispatch);
/// The 13 formal-authority-plane names.
pub const FORMAL_NAMES: [&str; FORMAL_COUNT] =
    project_plane::<FORMAL_COUNT>(Plane::FormalAuthority);
/// The 1 offline-script name.
pub const OFFLINE_NAMES: [&str; OFFLINE_COUNT] =
    project_plane::<OFFLINE_COUNT>(Plane::OfflineScript);

// Fail closed at compile time if the catalog drifts from the §6.1.2 counts,
// repeats a name, leaves a clause empty, or breaks the route derivation.
const _: () = assert!(REGISTERED_COUNT == 75, "catalog must hold 74+1 names");
const _: () = assert!(DISPATCH_COUNT == 61, "dispatch projection must be 61");
const _: () = assert!(FORMAL_COUNT == 13, "formal projection must be 13");
const _: () = assert!(OFFLINE_COUNT == 1, "offline projection must be 1");
const _: () = assert!(FAMILY_COUNT == 32, "table rows must be 32 families");
const _: () = assert!(names_unique(), "duplicate registered name");
const _: () = assert!(clauses_cited(), "operation without a cited clause");
const _: () = assert!(
    review_entry_registered(),
    "control.review missing or malformed"
);
const _: () = assert!(
    route_is_concat_of_registered_name(),
    "route not concat-derived"
);

const fn names_unique() -> bool {
    let mut i = 0;
    while i < MANAGEMENT_CATALOG.len() {
        let mut j = i + 1;
        while j < MANAGEMENT_CATALOG.len() {
            if str_eq(MANAGEMENT_CATALOG[i].name, MANAGEMENT_CATALOG[j].name) {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}

const fn clauses_cited() -> bool {
    let mut i = 0;
    while i < MANAGEMENT_CATALOG.len() {
        let entry = &MANAGEMENT_CATALOG[i];
        if entry.clause.is_empty() || entry.table_line < 811 || entry.table_line > 842 {
            return false;
        }
        i += 1;
    }
    true
}

const fn review_entry_registered() -> bool {
    let mut i = 0;
    while i < MANAGEMENT_CATALOG.len() {
        if str_eq(MANAGEMENT_CATALOG[i].name, REVIEW_OPERATION) {
            let entry = &MANAGEMENT_CATALOG[i];
            return matches!(entry.plane, Plane::Dispatch)
                && matches!(entry.execution, ExecutionKind::SyncReadonlyView)
                && matches!(entry.role, RoleRequirement::TrustedGatewayAdmin)
                && entry.table_line == 841;
        }
        i += 1;
    }
    false
}

const fn route_is_concat_of_registered_name() -> bool {
    let route = REVIEW_ROUTE.as_bytes();
    let prefix = MANAGE_ROUTE_PREFIX.as_bytes();
    let name = REVIEW_OPERATION.as_bytes();
    if route.len() != prefix.len() + name.len() {
        return false;
    }
    let mut i = 0;
    while i < prefix.len() {
        if route[i] != prefix[i] {
            return false;
        }
        i += 1;
    }
    let mut j = 0;
    while j < name.len() {
        if route[prefix.len() + j] != name[j] {
            return false;
        }
        j += 1;
    }
    true
}

/// Every registered name, in table order.
pub fn all_names() -> &'static [&'static str] {
    &ALL_NAMES
}

/// The dispatch-plane projection (`admin_ops`), 58 original names plus
/// `control.review`.
pub fn dispatch_names() -> &'static [&'static str] {
    &DISPATCH_NAMES
}

/// The formal-authority-plane projection; never an `admin_ops` or classify
/// Admin member.
pub fn formal_names() -> &'static [&'static str] {
    &FORMAL_NAMES
}

/// The offline-script projection.
pub fn offline_names() -> &'static [&'static str] {
    &OFFLINE_NAMES
}

/// The registered metadata of one name, or `None` for any dynamic or
/// off-table name (which stays rejected everywhere).
pub fn lookup(name: &str) -> Option<&'static ManagementOperation> {
    MANAGEMENT_CATALOG.iter().find(|entry| entry.name == name)
}

/// The metadata of the one registered synchronous read-only review action.
pub fn review_operation() -> &'static ManagementOperation {
    MANAGEMENT_CATALOG
        .iter()
        .find(|entry| entry.name == REVIEW_OPERATION)
        .expect("catalog asserts control.review is registered")
}

/// The dispatch-plane route of a registered name, concat-constructed from the
/// same registry (`MANAGE_ROUTE_PREFIX` + name).
pub fn manage_route(name: &str) -> String {
    format!("{MANAGE_ROUTE_PREFIX}{name}")
}
