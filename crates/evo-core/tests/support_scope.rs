//! Strict derived support-scope verification for E16.6 (V071/V072/V074/V080/V098/V123).
//! The current plan (lineage v4.1→v4.2→v4.3→v4.4→v4.5, plan §18.9–§18.11) remains normative; this
//! test rejects incomplete or inflated indexes. Historical evidence records keep the lineage digest
//! they were verified against; only the top level must bind the current plan. The v4.5 machine form
//! of plan §18.10 (Q-50) adds plan_recheck values, supersedes/completes, withdrawn_records and the
//! missing-evidence register; plan §2.9/§16.8 add the Harness² borrowings and fixed sources.

use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

const PLAN_VERSION: &str = "v4.5";
const PLAN_SHA: &str = "f6f9b1e44f3b1f364c0db8720f7eaec490ed7a03ddc053de8bfbab65326e6164";
/// Registered plan lineage (plan §18.9–§18.11) as (version, file, sha256), oldest first; the last
/// entry is the current binding. v4.4 was never bound on its own; the v4.5 rebind registers it.
const PLAN_LINEAGE: &[(&str, &str, &str)] = &[
    (
        "v4.1",
        "RSIAgent-v4.1定稿-工程实施方案-2026-09-19.md",
        "45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150",
    ),
    (
        "v4.2",
        "RSIAgent-v4.2定稿-工程实施方案-2026-09-30.md",
        "70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455",
    ),
    (
        "v4.3",
        "RSIAgent-v4.3定稿-工程实施方案-2026-10-12.md",
        "73dd00bf5ef6e9757d0c391c9a8be4ca92b6e06a750aacb542817f82cbd208a7",
    ),
    (
        "v4.4",
        "RSIAgent-v4.4定稿-工程实施方案-2026-10-23.md",
        "45ae29d71396f77dffc3baab7fed50564334734623bcec15f267778ddebeb496",
    ),
    (
        "v4.5",
        "RSIAgent-v4.5定稿-工程实施方案-2026-10-24.md",
        "f6f9b1e44f3b1f364c0db8720f7eaec490ed7a03ddc053de8bfbab65326e6164",
    ),
];
/// Per-record recheck marker (plan §18.10, v4.5 Q-50; §18.5): "required" never passes and
/// rechecked_<version> must name a plan later than the record's own. The expected marker follows
/// from the record: a record bound to the current plan carries not_affected explicitly; an older
/// record of the v4.5 union carries PLAN_RECHECKED; a v4.1/v4.2 record of a task outside that
/// union keeps V43_RECHECKED when v4.3 rechecked its task (a record outside the current union
/// keeps its historical value, §18.10); every other record keeps not_affected (explicit or absent).
/// v4.4 was never bound by the index on its own, so rechecked_v4.4 is not a marker.
const PLAN_RECHECKED: &str = "rechecked_v4.5";
const V43_RECHECKED: &str = "rechecked_v4.3";
const PLAN_RECHECK_VALUES: &[&str] = &[
    "not_affected",
    "required",
    "rechecked_v4.2",
    V43_RECHECKED,
    PLAN_RECHECKED,
];
/// Exact E-task union of the landing points of plan §19.6 (as corrected by v4.5) and §19.7
/// (plan §18.11); keys match literally, so "E16" is the parent key only (plan §18.10, Q-50).
const PLAN_RECHECK_TASKS: &[&str] = &[
    "E00", "E01", "E02", "E03", "E04", "E05", "E06", "E07", "E08", "E09", "E10", "E13", "E16",
    "E16.1", "E16.3", "E16.5", "E16.6",
];
/// Tasks rechecked by the v4.3 revision (AG-124/#165): every task except E17/E18.
const V43_RECHECK_TASKS: &[&str] = &[
    "E00", "E01", "E02", "E03", "E04", "E05", "E06", "E07", "E08", "E09", "E10", "E11", "E12",
    "E13", "E14", "E15", "E16", "E16.1", "E16.2", "E16.3", "E16.4", "E16.5", "E16.6",
];
const V43_RECHECK_FROM: &[&str] = &["v4.1", "v4.2"];
/// supersedes/completes exist since plan v4.5; a record carrying them is bound to v4.5 or later.
const RELATION_MIN_VERSION: &str = "v4.5";
/// V001–V123 (plan §13; v4.5 adds no V number).
const V_SCENARIO_COUNT: usize = 123;
/// Reviewed withdrawn tuples (id, withdrawn_in_pr, clause, owner_task, status) of plan §18.10:
/// 19 records withdrawn by PR #165/#169 and the five withdrawn by the v4.5 rebind. Dropping or
/// rewriting an entry fails; a listed id never returns to verified_subscopes (V080.d).
const EXPECTED_WITHDRAWN: &[(&str, u64, &str, &str, &str)] = &[
    (
        "E10.controller_acceptance",
        165,
        "§7.4.2–§7.4.3",
        "AG-182",
        "superseded",
    ),
    (
        "E10.replay_management_controller_acceptance",
        165,
        "§7.4.2–§7.4.3",
        "AG-182",
        "superseded",
    ),
    (
        "E10.ag064_replay_episode_recovery",
        165,
        "§7.4.2–§7.4.3",
        "AG-182",
        "superseded",
    ),
    (
        "E10.ag103_replay_fairness_waits_legal_set_v42",
        165,
        "§7.4.2–§7.4.3",
        "AG-182",
        "superseded",
    ),
    (
        "E08.ag037_economic_replay_cleanup",
        165,
        "§7.4.2–§7.4.3",
        "AG-182",
        "superseded",
    ),
    (
        "E09.ag074_local_scoped_acceptance",
        165,
        "§7.4.2–§7.4.3",
        "AG-182",
        "superseded",
    ),
    (
        "E16.6.ag012_plan_v42_index_binding",
        165,
        "§18.9",
        "AG-124",
        "replaced_by_binding",
    ),
    (
        "E04.ag097_broker_close_before_refuse",
        165,
        "§11.3.1/§11.6",
        "AG-119",
        "pending_recheck",
    ),
    (
        "E04.ag105_broker_bound_recovery_close_v42",
        165,
        "§11.3.1/§11.6",
        "AG-119",
        "pending_recheck",
    ),
    (
        "E08.ag046_write_side_revocation",
        165,
        "§11.3.1",
        "AG-119",
        "pending_recheck",
    ),
    (
        "E08.ag059_manifest_prewrite_gate",
        165,
        "§11.3.1",
        "AG-119",
        "pending_recheck",
    ),
    (
        "E04.ag054_typed_predispatch",
        169,
        "§7.3.1",
        "AG-132",
        "pending_recheck",
    ),
    (
        "E06.controller_acceptance",
        165,
        "§5.5.2",
        "AG-169",
        "pending_recheck",
    ),
    (
        "E13.controller_acceptance",
        165,
        "§5.5.2",
        "AG-169",
        "pending_recheck",
    ),
    (
        "E16.4.real_store_rejection_controller_acceptance",
        165,
        "§5.5.2",
        "AG-169",
        "pending_recheck",
    ),
    (
        "E09.ag042_recovery_counting",
        165,
        "§7.2.3",
        "AG-171",
        "pending_recheck",
    ),
    (
        "E13.ag028_consolidation_trigger",
        165,
        "§6.1.2",
        "AG-139",
        "pending_recheck",
    ),
    (
        "E16.1.durable_import_controller_acceptance",
        165,
        "§11.2.1",
        "AG-148",
        "pending_recheck",
    ),
    (
        "E02.ag062_required_improver_baselines",
        165,
        "§5.1.2",
        "AG-135b",
        "pending_recheck",
    ),
    (
        "E09.ag033_run_next_trusted_gate",
        200,
        "§6.7.4",
        "AG-187",
        "pending_recheck",
    ),
    (
        "E16.6.ag131_v43_project_license_metadata",
        200,
        "§2.8.1",
        "AG-182",
        "superseded",
    ),
    (
        "E05.ag052_versioned_batch_verdicts",
        200,
        "§3.4",
        "AG-192",
        "pending_recheck",
    ),
    (
        "E05.ag080_local_scoped_acceptance",
        200,
        "§3.4",
        "AG-192",
        "pending_recheck",
    ),
    (
        "E05.ag101_streaming_verdicts_v42",
        200,
        "§3.4",
        "AG-192",
        "pending_recheck",
    ),
];
/// Reviewed relation pairs (kind, new record id, old record id): 7 supersedes and 25 completes.
const EXPECTED_RELATIONS: &[(&str, &str, &str)] = &[
    (
        "supersedes",
        "E08.ag182_replay_v3_economic_cleanup",
        "E08.ag037_economic_replay_cleanup",
    ),
    (
        "supersedes",
        "E09.ag182_replay_v3_legal_prefix",
        "E09.ag074_local_scoped_acceptance",
    ),
    (
        "supersedes",
        "E10.ag182_replay_v3_controller_acceptance",
        "E10.controller_acceptance",
    ),
    (
        "supersedes",
        "E10.ag182_replay_v3_episode_recovery",
        "E10.ag064_replay_episode_recovery",
    ),
    (
        "supersedes",
        "E10.ag182_replay_v3_fairness_waits_legal_set",
        "E10.ag103_replay_fairness_waits_legal_set_v42",
    ),
    (
        "supersedes",
        "E10.ag182_replay_v3_management_controller_acceptance",
        "E10.replay_management_controller_acceptance",
    ),
    (
        "supersedes",
        "E16.6.ag182_v45_project_license_metadata",
        "E16.6.ag131_v43_project_license_metadata",
    ),
    (
        "completes",
        "E00.ag182_rerun_controller_acceptance",
        "E00.controller_acceptance",
    ),
    (
        "completes",
        "E01.ag182_rerun_controller_acceptance",
        "E01.controller_acceptance",
    ),
    (
        "completes",
        "E02.ag182_rerun_ag092_local_scoped_acceptance",
        "E02.ag092_local_scoped_acceptance",
    ),
    (
        "completes",
        "E02.ag182_rerun_ag093_local_scoped_acceptance",
        "E02.ag093_local_scoped_acceptance",
    ),
    (
        "completes",
        "E02.ag182_rerun_controller_acceptance",
        "E02.controller_acceptance",
    ),
    (
        "completes",
        "E03.ag182_rerun_controller_acceptance",
        "E03.controller_acceptance",
    ),
    (
        "completes",
        "E04.ag182_rerun_ag077_local_scoped_acceptance",
        "E04.ag077_local_scoped_acceptance",
    ),
    (
        "completes",
        "E04.ag182_rerun_controller_acceptance",
        "E04.controller_acceptance",
    ),
    (
        "completes",
        "E05.ag182_rerun_ag083_partition_admission_rejection_atomicity",
        "E05.ag083_partition_admission_rejection_atomicity",
    ),
    (
        "completes",
        "E05.ag182_rerun_ag087_local_scoped_acceptance",
        "E05.ag087_local_scoped_acceptance",
    ),
    (
        "completes",
        "E05.ag182_rerun_ag088_local_scoped_acceptance",
        "E05.ag088_local_scoped_acceptance",
    ),
    (
        "completes",
        "E05.ag182_rerun_ag089_local_scoped_acceptance",
        "E05.ag089_local_scoped_acceptance",
    ),
    (
        "completes",
        "E05.ag182_rerun_ag090_local_scoped_acceptance",
        "E05.ag090_local_scoped_acceptance",
    ),
    (
        "completes",
        "E05.ag182_rerun_ag091_local_scoped_acceptance",
        "E05.ag091_local_scoped_acceptance",
    ),
    (
        "completes",
        "E05.ag182_rerun_controller_acceptance",
        "E05.controller_acceptance",
    ),
    (
        "completes",
        "E06.ag182_rerun_ag084_local_scoped_acceptance",
        "E06.ag084_local_scoped_acceptance",
    ),
    (
        "completes",
        "E07.ag182_rerun_controller_acceptance",
        "E07.controller_acceptance",
    ),
    (
        "completes",
        "E08.ag182_rerun_ag075_local_scoped_acceptance",
        "E08.ag075_local_scoped_acceptance",
    ),
    (
        "completes",
        "E08.ag182_rerun_controller_acceptance",
        "E08.controller_acceptance",
    ),
    (
        "completes",
        "E09.ag182_rerun_controller_acceptance",
        "E09.controller_acceptance",
    ),
    (
        "completes",
        "E11.ag182_rerun_controller_acceptance",
        "E11.controller_acceptance",
    ),
    (
        "completes",
        "E12.ag182_rerun_controller_acceptance",
        "E12.controller_acceptance",
    ),
    (
        "completes",
        "E16.1.ag182_rerun_ag070_local_scoped_acceptance",
        "E16.1.ag070_local_scoped_acceptance",
    ),
    (
        "completes",
        "E16.1.ag182_rerun_ag076_local_scoped_acceptance",
        "E16.1.ag076_local_scoped_acceptance",
    ),
    (
        "completes",
        "E16.6.ag182_rerun_ag086_local_scoped_acceptance",
        "E16.6.ag086_local_scoped_acceptance",
    ),
];
/// V080.d "旧记录被改写": SHA-256 of the canonical JSON (keys sorted, no insignificant whitespace,
/// UTF-8; evo_core::canonical_fingerprint) without plan_recheck of all 138 records: the 93
/// historical records retained from the v4.3 index binding (base a389c4e3) and the 45 records
/// added by AG-182 (PR #200), pinned by AG-200 after the merge of PR #200 (02a135aa) was
/// backfilled into its 33 records.
const EXPECTED_RECORD_DIGESTS: &[(&str, &str)] = &[
    (
        "E00.controller_acceptance",
        "1d8594a791d51459e7272f8c97bc17df016df597bd85b696ce2b2379313ee80a",
    ),
    (
        "E00.ag099_step_future_default_test_stack",
        "ff982495a71a566a1a2f30c43a74ebce147321b32a65979fcc6ea9c9bfd229fa",
    ),
    (
        "E00.ag182_rerun_controller_acceptance",
        "ff8adb183d2bb8270d680fbcd6b01f096944b787f425a9b7cbe8d957a35b6a59",
    ),
    (
        "E01.controller_acceptance",
        "074a9cd7508f9ba3a13f2bc5f0894740ae57c8593ecc7d411c9e9d64d3990501",
    ),
    (
        "E01.ag057_explicit_sample_registration",
        "1661bc81322d2817fcf025437d354b083a2e5b9fa83db9c07ab3a4a24b9b9bfe",
    ),
    (
        "E01.ag144_feasibility_approval_alignment_v43",
        "3d3f37831720573ad7de398fe3bf7a8c69ca4be3dff4727e2e6201438c5e8078",
    ),
    (
        "E01.ag182_rerun_controller_acceptance",
        "23d263f89c080b38eca2bb506058793448780de6c3ca903e15ae33c177c52eaa",
    ),
    (
        "E02.controller_acceptance",
        "e394666658305fca71b0c7d631aa1b323dea92fd8cd555845ee74165640704c1",
    ),
    (
        "E02.ag055_v1_wire_goldens",
        "906e3945c9b4fd329f7e99bee4f8300e886fc6571363fd22bd9acfab5ad5215a",
    ),
    (
        "E02.ag065_host_surface_name_coverage",
        "c17b10748a3d1938f470192799bb72ca377bb117ea013c642b5d878120a40199",
    ),
    (
        "E02.ag092_local_scoped_acceptance",
        "29bde3c9b96dc0ed4209cae7fce85ccb53aada9374ef38073eaa65ad51176c5c",
    ),
    (
        "E02.ag093_local_scoped_acceptance",
        "2847402ce6df60554ac63bc9c6e557194b768c56f53f7eae2111c60fea2c86e6",
    ),
    (
        "E02.ag126_v43_host_surface_duplicates",
        "4a6d22bd80043575aabd11caeff4dd92028912bd6ce033cb3116a49f4d64efde",
    ),
    (
        "E02.ag136_management_registry_single_source_v43",
        "2559f07520e1d6c1bbd8805e3356cae41688e85c245a7016fe510502cf6766e7",
    ),
    (
        "E02.ag182_rerun_ag092_local_scoped_acceptance",
        "cab188530425235d566fe1d64b4971fdb0e7cd630a7845b516cbd38e53f7159d",
    ),
    (
        "E02.ag182_rerun_ag093_local_scoped_acceptance",
        "f13839e456363cbf0e3f8c48d6cb35e2152be19d5e45d0b21f1acd6b7c2037c7",
    ),
    (
        "E02.ag182_rerun_controller_acceptance",
        "8e716d2c64888a520fe992a393c31cc8df64e540c1b554b76ad111dddb9076f3",
    ),
    (
        "E03.controller_acceptance",
        "724c6e819805aaff53c31acf981b14f4ed45c66cbf30e879388ba30c095250ca",
    ),
    (
        "E03.ag016_development_receipts",
        "2291dcc8302da2ddb57e19d0cadf992f145853d612c72e25497185201d90f916",
    ),
    (
        "E03.ag079_local_scoped_acceptance",
        "0d6187cdd07d2302d608afb46d8c0dcec7d9ba718f69098cb6e32d51c63d641a",
    ),
    (
        "E03.ag182_rerun_controller_acceptance",
        "0d61b603b9de44bbb348331aa09a445382af8ac9bae67eb8f6cf57619887ace8",
    ),
    (
        "E04.controller_acceptance",
        "f49955f820927e62185e48ed3e36e6daf97a1b518b21b7ec2b65662ce3bb9cbb",
    ),
    (
        "E04.ag082_local_scoped_acceptance",
        "4f3bfad1a3766ec3f05185061823c5162c31e22b49406caf38c427302e19f976",
    ),
    (
        "E04.ag077_local_scoped_acceptance",
        "5544b5435969d00c5edb1dc62fdb738e0b2d16ef16f86bb40160150e4bb446ed",
    ),
    (
        "E04.ag121_v43_redacted_bound_request_identity",
        "e8e3528768d23ab3fdb673a819cb981345f12794e0d8280fb09d1624f93d0cbd",
    ),
    (
        "E04.ag143_external_sampling_projection_v43",
        "a9cb2fe2b95f1b1c3cd3a980beee9e57bdbbdd990256c22a16c02b7c3e15bc6d",
    ),
    (
        "E04.ag134_root_budget_cancellation_v43",
        "7930cb43067589856a17e70db2560fe12a7c84ff72455dae114947acd5859912",
    ),
    (
        "E04.ag182_rerun_ag077_local_scoped_acceptance",
        "c13bda7a9c27925f8448dfb6220328ce5ea1121b6eb2c7232c71ba0011b20e9f",
    ),
    (
        "E04.ag182_rerun_controller_acceptance",
        "ebca59d1d54f7b3ab778198a36015bc6d7875d18ff05063afbfa60948daea712",
    ),
    (
        "E05.controller_acceptance",
        "561829b705c81d4754ede69684bb680c264411f28b7ccbca92c9c70d3b1a1c3a",
    ),
    (
        "E05.ag083_partition_admission_rejection_atomicity",
        "a583638461b587598d2d1de13848710ef6e0268dde1685b91d95b3fa26278125",
    ),
    (
        "E05.ag087_local_scoped_acceptance",
        "a826a4fd3234faaa99bcc00b54907549131ed911e9a2c226ba23b17fc3098c67",
    ),
    (
        "E05.ag088_local_scoped_acceptance",
        "bf0c6a100434e38626ec4e66dc3cc763fa6864bb51210ecf972eda19477d3989",
    ),
    (
        "E05.ag089_local_scoped_acceptance",
        "b488a11ca760fcfae119f7972142f0ffaab5ea0c1f3f11032fcb0d074e343b27",
    ),
    (
        "E05.ag090_local_scoped_acceptance",
        "f8b4e7a1999f1426eb8098d9bf43d027a0e3940b41f79fbfbe26d468f0fa8694",
    ),
    (
        "E05.ag091_local_scoped_acceptance",
        "3effa80c484dbb8810011f7501887f7f1bc4c611b407758d4d0d7ebed6c59a6d",
    ),
    (
        "E05.ag095_canonical_grader_receipt_digest",
        "924974f332b9cf7bcb5138b631f90360ec724f57c8f392eaba01d5efa7e79640",
    ),
    (
        "E05.ag109_external_host_authority_bound_ticket",
        "4254c96216040460b6f65594e7343a24416901afbdd079c4055bb781f3a37e5c",
    ),
    (
        "E05.ag100_formal_candidate_dependency_binding_v42",
        "fb3d5e5739db8d1160bcae324927994183fd9daefccae406c0bd8587176b392d",
    ),
    (
        "E05.ag118_late_receipts_after_final_report_v42",
        "1db33e283844a4f0f2b852ef99e1c58b772a2dea0805d65e1beeb7e87486cecb",
    ),
    (
        "E05.ag137_evaluator_control_authentication",
        "08131d590c228486bbed331162892cac9b0b7dfca9d72acc561c048c9bd25eb5",
    ),
    (
        "E05.ag120_one_execution_receipt_per_call",
        "8c4a67f78c641ba7fe694400e7ab543256f926be705a4cf72938cb9f39f00e2b",
    ),
    (
        "E05.ag122_ga18_bound_cycle_validation",
        "afe565d3d31dbfb7673985b9ca69dbd8966910c43c85359b9846eda3c10680e0",
    ),
    (
        "E05.ag182_rerun_ag083_partition_admission_rejection_atomicity",
        "8170f3e50d9a4d5a4bfe7286b55d75216858a92cf8c9b8f23206ca421fce74ed",
    ),
    (
        "E05.ag182_rerun_ag087_local_scoped_acceptance",
        "89f58763843d61d1838d1f44661ea548e44b9f70a5856d83bb9a414488ab6b74",
    ),
    (
        "E05.ag182_rerun_ag088_local_scoped_acceptance",
        "bbfe46fe78042708fb504984b2e72da8ec8f553b963f6bbc175ef00043ae00ae",
    ),
    (
        "E05.ag182_rerun_ag089_local_scoped_acceptance",
        "08bfe321465a193366d43aab54d1e3be2e7a9629f3a47dfbfcee0dbc68959368",
    ),
    (
        "E05.ag182_rerun_ag090_local_scoped_acceptance",
        "b73e1c420dbfb6a4316d97b64d72d89df7c0edada237fe166c4dd2220399253c",
    ),
    (
        "E05.ag182_rerun_ag091_local_scoped_acceptance",
        "6fa5c9ad08f8f38ce686cfa3ed6de341f436438f75bf132acb7d8a1daa0618a8",
    ),
    (
        "E05.ag182_rerun_controller_acceptance",
        "634f91847f89d8ba23f5f459508e0282d11c54b3e613dc4fb6ee85e55d28ad0c",
    ),
    (
        "E06.ag084_local_scoped_acceptance",
        "15f35447ee36c450ad8fa9bd3f95dd4e587ff2b758bd1591e9b88abf43eb3cdb",
    ),
    (
        "E06.ag116_edit_compiled_produce_review_v42",
        "74dc866a916baa251ba3283b6318403ae416564798946bb7a7e9cdfe7c99f5ba",
    ),
    (
        "E06.f01_builtin_generation_strategy_authority",
        "bfe72fbcf7e57974143375e97fee3a75c29c06b80f4e8abd3b86a15058391546",
    ),
    (
        "E06.ag182_rerun_ag084_local_scoped_acceptance",
        "e00b4966b14b40419c6de57ed34850bca1fe7eefe632593be6519a92e83f726e",
    ),
    (
        "E07.controller_acceptance",
        "ac6314300943f30c0e27913b978be822f9387d553006e5d6e16f62b60d517f41",
    ),
    (
        "E07.ag015_curriculum_step_consumer",
        "6e61f463ac09aa20e2c5d993d2c6d74d093093370955cc78f52ae46b938601d8",
    ),
    (
        "E07.ag017_exploration_start_consumer",
        "aebd4d7fc8c56b3f39da0759083899fe36203a1044a39a9e22ae1bc343be2e54",
    ),
    (
        "E07.ag027_management_cleanup_closure",
        "6af732292546dcb863e8c1a0c155de4a127109711d262b21a9b68524d2b496f9",
    ),
    (
        "E07.ag044_submit_upstream_gate",
        "c673295669527dfd97da6fe6f46ee4ab02f7500a6afac11c40eb98f6027ec39e",
    ),
    (
        "E07.ag108_candidate_review",
        "ad759df1c7f9182e866d8f1ccaf0cf9dfc0ce3b72faedf9c0889c4b3ffa19123",
    ),
    (
        "E07.ag182_rerun_controller_acceptance",
        "ff3ec07197b51da4996c73772a0bfb994c70ed80ec5b35255383ec6085d0fdd7",
    ),
    (
        "E08.controller_acceptance",
        "c62759622bb999c43cb7ebf7bb40f67360e3a7b6421fe2caeddad1338e544991",
    ),
    (
        "E08.ag036_revocation_source_collision",
        "fae74423b0845f16d88ae3378495089a087a2639230ae1f7ceab97f21ed5d122",
    ),
    (
        "E08.ag035_cleanup_fixpoint",
        "2204561a86ab1cb38dc2b99844e229bfd063bb564e416f77e5be21e6b8ffdf25",
    ),
    (
        "E08.ag038_budget_revoke_gate",
        "2bd2b2fa39b9f5566f572502b6bc6c9191d76c02eaa1ffe7c33eaa51a1fecc17",
    ),
    (
        "E08.ag043_late_response_revoked",
        "ddefd6ba46b238eb37dda20f805a57019d0b6ea86b55958c5141754f7b30aa66",
    ),
    (
        "E08.ag045_late_stage_fact_redaction",
        "2e57c45b6d4c8679531ae431d5728f87edc299664ccce772db057a5dd72d1bba",
    ),
    (
        "E08.ag053_bounded_dependency_snapshots",
        "4a38d2bdfb2e3e241dddf8fe73d4a5f6f5e1ef879b6653321e0b0c4ba761b363",
    ),
    (
        "E08.ag068_native_authority_cleanup",
        "37f7628c981293b236f058ac277063d4a062b6982f21ab9eb7c45bcf411ea77d",
    ),
    (
        "E08.ag069_local_scoped_acceptance",
        "b83833fdeea7603cdfaacf17075d84dd645d8da683faca0ddff9fe6d5e69cc02",
    ),
    (
        "E08.ag073_local_scoped_acceptance",
        "0424d968409ba37abd2ea887353ca3bae11da1e1312e29492a111263b7724459",
    ),
    (
        "E08.ag075_local_scoped_acceptance",
        "24452482075fe9e8132119ccf6448e9ea4b9dbc03bd16bd1a24596cc9cb45df3",
    ),
    (
        "E08.ag096_unreadable_bound_request_isolated",
        "a26967f7fedd6681424cadd4fa2508c73ebf77aa7a098f72c97863c668148a86",
    ),
    (
        "E08.ag107_budget_ref_missing_fail_closed_v42",
        "a0a679df3d49ca1bd3ce663a3bf83b427c1cfb2cc2d42c690477deac0d597ed6",
    ),
    (
        "E08.ag106_budget_startup_recovery_v42",
        "32f8e7825843418114547ad3f3d95fdc8872afa5663d6dd8ae3eed2ba9549744",
    ),
    (
        "E08.ag104_restore_prefix_migration_upgrade_v42",
        "755f4769045cf640f44ad391eddb152e7a2c561c65b94e0aa51363115fe8a02a",
    ),
    (
        "E08.ag117_formal_call_revoke_cleanup_v42",
        "e105db3dc22aa5e1eefbdf18fae36b01fadca3858e5f06c2e1aa1ceb5b1066a5",
    ),
    (
        "E08.ag182_replay_v3_economic_cleanup",
        "1735efd723fd789edd5b6213c384b662b5dff785871ec1817e244cacb39c3a65",
    ),
    (
        "E08.ag182_rerun_ag075_local_scoped_acceptance",
        "c803b3dd8caff0f85438e2b7f7f8b057ef8e3ef4c2233fec6c205e206788e109",
    ),
    (
        "E08.ag182_rerun_controller_acceptance",
        "f61df7ebb6b9fec080628b87c764e10cf146fea1cd67f1252ad22aa2b295e6ac",
    ),
    (
        "E09.controller_acceptance",
        "f5ce8f536ed777d9534b2514867708d8b7ea8718be0e07c7184a262a7035c617",
    ),
    (
        "E09.ag021_skill_group_jobs",
        "12c6f148b56305cd068652cecaa4bea1b9f5cb98fcc0540d680b8f8084c575bb",
    ),
    (
        "E09.ag024_exploration_cleanup_edges",
        "7076ba1d4f789db61481c97e527aa1e50c4801db91d7b1b9416e86f8fc52fe60",
    ),
    (
        "E09.ag026_practice_attempt_sets",
        "9f5054d227dbd8a2851f5709c3aa2365bd09909e9656d45cb0c66ff56b4828af",
    ),
    (
        "E09.ag039_legal_actions",
        "26c2b8b3a7c4700ac2e10fce9ba2f7675cc348d7fb26feffe94720c7bed67b71",
    ),
    (
        "E09.ag040_exploration_registration",
        "d940e29be319c6e21569d7d68df0977bb67bb4302942e398b70aa97f0e188d8b",
    ),
    (
        "E09.ag041_postpaid_convergence",
        "384514f590f119120bbcab247d565328f27484cefe7d77df7fd0e1c308b82cc2",
    ),
    (
        "E09.ag048_terminal_classes",
        "7c21051db6efae69f59f43de82fd099c9950cfd39c1267709c0fbbfef70b3f5c",
    ),
    (
        "E09.ag103_fairness_waits_legal_set_v42",
        "0132841ac9a6043c5d32db733a7a9b8aff8270762fde2acba30c63043c656398",
    ),
    (
        "E09.ag182_replay_v3_legal_prefix",
        "f000dc8d18848f0dfb750867c506b677e9cee9633d718035757a6cb2098e54e9",
    ),
    (
        "E09.ag182_rerun_controller_acceptance",
        "a82beb8d2ba5c8397814bcfa5453837426d036a97cfde51f0eca087f2b950694",
    ),
    (
        "E10.ag032_replay_redacted_reads",
        "a957960111997b98119c09151e57e4e949a35a6e8d312d2b6f373a09f9d4104b",
    ),
    (
        "E10.ag147_replay_lineage_versioning_v43",
        "56d997331600edc5aef4684ad7d8c9f755c1e05f8923b10d09f400650de64926",
    ),
    (
        "E10.ag182_replay_v3_controller_acceptance",
        "2f397f805e25dac7652fd1d0f7c43ca4f7d3aec593829cd15d334ca322e153ef",
    ),
    (
        "E10.ag182_replay_v3_management_controller_acceptance",
        "62031e625060284ba7ab39c6664a4bed4a2e9618f393c61a1be7f4ff3e4da5b4",
    ),
    (
        "E10.ag182_replay_v3_episode_recovery",
        "74bcbff491e88f189a6003659aaaf53518abcf3bb4e815b021ca287689f67e8c",
    ),
    (
        "E10.ag182_replay_v3_fairness_waits_legal_set",
        "d3290a8a1260c958b39d88fe3d6d67c091bbfb6be5cd11381bcd20f20776f27f",
    ),
    (
        "E11.controller_acceptance",
        "da5e5ea97acd78efeded949a10a9548a1617972bf576851777e94d1815ba8678",
    ),
    (
        "E11.ag182_rerun_controller_acceptance",
        "581d88b20c959aca7127d22ffb7ac5283bf60ebb405675004f24c9decb258aa0",
    ),
    (
        "E12.controller_acceptance",
        "86e83f3766896892230f8e092d2941223b7dfa00f3c87657ca3544b4377358a6",
    ),
    (
        "E12.ag025_curriculum_cleanup",
        "e7848f07c68cc83acd4b5aa491db6a624b2e904dd52922c329920f99dbb64683",
    ),
    (
        "E12.ag050_curriculum_priority",
        "178dcb3f3445b5ce93b6f6dbb421aaceb7bf4886dfe2906f1ce748fda4b1956e",
    ),
    (
        "E12.ag182_rerun_controller_acceptance",
        "a10c6776facc52c80de4e41e6a20245baa89054f2b7aec723de2f8ef6b137fb2",
    ),
    (
        "E13.ag022_consolidation_closure",
        "7a7440e4247874584b1b0177ec063175fad1b5f59fa0054c7469d383086f6a4d",
    ),
    (
        "E13.ag030_consolidation_budget_stage",
        "36b9fc48a26ea31f0a4e12b779938232ce01b1e8602a7f231d87cc9b919e81e4",
    ),
    (
        "E13.ag031_bounded_terminal_category",
        "77481ced5879ebab6c1fad3223385f6346eb3bcc38b5347446f4f26687564c27",
    ),
    (
        "E13.ag081_local_scoped_acceptance",
        "3c0a40f2c518b1f89dc053088c57b6316c3e5000f23c60680bb5b353b6af3c34",
    ),
    (
        "E14.ag019_exploration_policy_inheritance",
        "6ac6910902a56230da0de48ef8a421fed311979ed655312ffe2156f6689c9c02",
    ),
    (
        "E14.ag034_metatrial_fork",
        "70570be81dc7f69159de7b9ab6b8698b69421357af209fa993254761ce08f681",
    ),
    (
        "E14.ag058_current_description_boundaries",
        "968ace7f370ef39e775d0ae4bcb225e3a20bc27046b1c25077f208ac49212289",
    ),
    (
        "E16.1.ag076_local_scoped_acceptance",
        "1db84fb47a4dd266efe79ff2d6cd62f7b300a3ea2526e307bc70adaf5a94bd12",
    ),
    (
        "E16.1.ag070_local_scoped_acceptance",
        "f593c9ee97df37e97c455d4e70b0c1feafe6bd31911375f64327a6cb740d0eaf",
    ),
    (
        "E16.1.ag182_rerun_ag070_local_scoped_acceptance",
        "0afe71a49608966752a01b14404eb88617d01d72b0029033675c680dca851450",
    ),
    (
        "E16.1.ag182_rerun_ag076_local_scoped_acceptance",
        "8001653350fbfcbf248cb026112892c01b318c2a8accc124871bbc187db6d778",
    ),
    (
        "E16.2.durable_staging_publish_controller_acceptance",
        "adcb1f3a66b3a675cd4aa3e31a2c49c4e734f83042422740093650a24d3f4413",
    ),
    (
        "E16.2.ag071_known_marker_utf8_blocking",
        "21554a03c30f5e2c6124aa7a9abf0ad598df1f736f144a4cc3ded4e42be539b8",
    ),
    (
        "E16.2.ag085_utf8_privacy_boundary_regressions",
        "2cef286c6f53f1fa34318b4cca3b559676d2a369e46038557239ea997062f70b",
    ),
    (
        "E16.3.seed_install_reset_controller_acceptance",
        "54785b9628e46df095c5eda46b691af653d93a90828c26c655b44f1995179faa",
    ),
    (
        "E16.3.ag127_v43_seed_classification",
        "1705c3600e0c1a14370dd4c580837cc31f0fbfdffe254a53e052991aad0e56f8",
    ),
    (
        "E16.4.ag066_local_stdio_harness",
        "4e6c261717492dc027fcfaafc2133ad37fe32a7496f47d945cf5dcd9d28a1617",
    ),
    (
        "E16.4.ag126_v43_host_surface_record",
        "f5f66a5cadb290764631e415b3eddeb60cdbce534e777229382078a7359f528a",
    ),
    (
        "E16.5.gate_functions_controller_acceptance",
        "cd8c6699299b7d6159b2d0fd0f7d530d6b987dfd1ddf0ae58b4c5fdacec7b802",
    ),
    (
        "E16.5.ag014_capacity_gates_wired",
        "efe04bb278bd8ffdc4ec7da6d955fd312609b88a08188e4acf394d6236ec8faa",
    ),
    (
        "E16.5.ag018_startup_recovery_gate",
        "bd15e029359e9afafd6d70a6560ebf33df1bfafdab24dec2f61e2bbba6ef2d74",
    ),
    (
        "E16.5.ag029_restore_coverage",
        "13224ab8a4ce6aa16305738272f61c5d8ee3d7322ac7c61fc670375acb6be038",
    ),
    (
        "E16.5.ag051_economic_restore",
        "ca1b560d5c179f8de71df3b770c63c99ef95146a683dcae79c5cca020f7072fa",
    ),
    (
        "E16.5.ag125_v43_single_writer",
        "bc602ffe5639d5aaa0db5e54629a0237c30fd2dbba2d0957ff99a18ec4283d1f",
    ),
    (
        "E16.5.ag129_v43_system_clock_conversion",
        "a9f3d6efb7173b6ca5bb1bd64946a1919212aeb90213076fc9f92d680af842f9",
    ),
    (
        "E16.5.ag129_v43_broker_first_clock_refusal",
        "3505db766eb5baa3ca918da282110c07c64a448942776484451bdb409cb32349",
    ),
    (
        "E16.5.ag150_readonly_dependency_api_v43",
        "0d71ec6742bc4ae73065efb0e635f8e866a049c2a50bf9a1053241f8d063235c",
    ),
    (
        "E16.5.ag153_shared_prepare_contract_v43",
        "666c76b37ca7689a77c286e29be410f492aa4277caf15d4db518e3a9cf25b83f",
    ),
    (
        "E16.6.ag086_local_scoped_acceptance",
        "a55a8e65cb303d4c683a356ce7ccb3922c7480ed1041722775506b5880c8c537",
    ),
    (
        "E16.6.ag114_evidence_identity_ownership",
        "70b124f58240ea0b25b0ac9cc573531f70359883aef1e50858cc60ae80cb8f8a",
    ),
    (
        "E16.6.ag182_v45_project_license_metadata",
        "56eb362a529ab381aca9035c2361df5097abba5e3c08df0dc2226dadf15f6ee7",
    ),
    (
        "E16.6.ag182_rerun_ag086_local_scoped_acceptance",
        "415850c541a618837ccc08f8973558f9223aceff92f917931a452935120b887e",
    ),
    (
        "E16.6.ag182_plan_v45_index_binding",
        "3fe7f10b44b56128e6f19a6955def29b91be3493271112169d89437c50d38cc2",
    ),
    (
        "E17.disabled_rejection_controller_acceptance",
        "6046a7d9fe6f644c02b88ad8b52ccd2a25aa16078563031332fb1e15c9084442",
    ),
    (
        "E18.disabled_rejection_controller_acceptance",
        "75279240c222bbd5e5a65656afa30f7cb0b186253f95ec496d3b27f04acee0a1",
    ),
];
/// Reviewed (record_id, field, path) entries of the missing-evidence register (42).
const EXPECTED_MISSING_TRIPLES: &[(&str, &str, &str)] = &[
    (
        "E00.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e00-directed-test.log",
    ),
    (
        "E00.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e00.json",
    ),
    (
        "E01.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e01-tests.log",
    ),
    (
        "E01.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e01-input.json",
    ),
    (
        "E02.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e02-tests.log",
    ),
    (
        "E02.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e02-formatted-input.json",
    ),
    (
        "E02.ag092_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG092-existing-current27df8c2-fixed39d2d86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E02.ag093_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG093-existing-currentc04850d-fixed49806d7-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r2-20261011.json",
    ),
    (
        "E03.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e03-final-tests.log",
    ),
    (
        "E03.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e03-final-input.json",
    ),
    (
        "E04.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e04-storage-final.log",
    ),
    (
        "E04.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e04-final-input.json",
    ),
    (
        "E04.ag077_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG077-existing-PR125-current529-fixed26d3-ROOT-fresh12-actual-current-main-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E05.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e05-final-streaming.log",
    ),
    (
        "E05.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e05-final-input.json",
    ),
    (
        "E05.controller_acceptance",
        "record_source",
        "out/implementation-v4.1-20260919/controller-e05-acceptance.json",
    ),
    (
        "E05.ag083_partition_admission_rejection_atomicity",
        "log_path",
        "out/scratch/ag-083-impl/current-fixed-single-clippy-repair-full8-direct-root-run-r1-user-continue-20261003/full-workspace-MAIN-original-native.json",
    ),
    (
        "E05.ag087_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG087-existing-current77152-fixedfde3dd4-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E05.ag088_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG088-existing-current8cbf82c-fixede538b96-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E05.ag089_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG089-existing-currentbd20099-fixed6040bf3-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E05.ag090_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG090-existing-currentc9aa71e-fixeda994443-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E05.ag091_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG091-existing-current1f1b593-fixed730f242-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E06.ag084_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG084-existing-current349-fixed3c5-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E07.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e07-service.log",
    ),
    (
        "E07.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e07-protocol-input.json",
    ),
    (
        "E07.controller_acceptance",
        "record_source",
        "out/implementation-v4.1-20260919/controller-e07-protocol-acceptance.json",
    ),
    (
        "E08.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e08-storage-tests.log",
    ),
    (
        "E08.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e08-final-input.json",
    ),
    (
        "E08.controller_acceptance",
        "record_source",
        "out/implementation-v4.1-20260919/controller-e08-acceptance.json",
    ),
    (
        "E08.ag075_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG075-existing-current1132-fixed8a92e57-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E09.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e09-engine.log",
    ),
    (
        "E09.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e09-input.json",
    ),
    (
        "E09.controller_acceptance",
        "record_source",
        "out/implementation-v4.1-20260919/controller-e09-acceptance.json",
    ),
    (
        "E11.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e11-core.log",
    ),
    (
        "E11.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e11-final-input.json",
    ),
    (
        "E11.controller_acceptance",
        "record_source",
        "out/implementation-v4.1-20260919/controller-e11-acceptance.json",
    ),
    (
        "E12.controller_acceptance",
        "log_path",
        "out/implementation-v4.1-20260919/controller-e12-engine.log",
    ),
    (
        "E12.controller_acceptance",
        "input_ref",
        "out/implementation-v4.1-20260919/controller-e12-final-input.json",
    ),
    (
        "E12.controller_acceptance",
        "record_source",
        "out/implementation-v4.1-20260919/controller-e12-acceptance.json",
    ),
    (
        "E16.1.ag076_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG076-existing-currentAA51-fixed1355e95-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E16.1.ag070_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG070-existing-current3ad-fixed887d97a-ROOT-standard8-head38-parent36-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
    (
        "E16.6.ag086_local_scoped_acceptance",
        "log_path",
        "out/audit-20260930/qa/takeover-20261002/AG086-existing-currentAA831-fixed908eb85-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json",
    ),
];
const WITHDRAWN_KEYS: &[&str] = &["clause", "id", "owner_task", "status", "withdrawn_in_pr"];
const WITHDRAWN_STATUSES: &[&str] = &["pending_recheck", "superseded", "replaced_by_binding"];
/// Missing-evidence register (plan §18.10 "本地缺失的证据路径", v4.5 Q-50).
const MISSING_REGISTER_PATH: &str = "reports/evidence-missing.json";
/// Reviewed SHA-256 of the register file itself (AG-200, review S-1), pinned here and in the Python
/// checker: the manifest pointer, this constant and the file must agree, so an edited entry (an
/// expected identity included) cannot pass by refreshing the pointer alone. Kept on one line.
#[rustfmt::skip]
const EXPECTED_MISSING_REGISTER_SHA256: &str = "b843e3f9692c5ba18ce9148278b3267584222461406fdccc17162ed84c115337";
/// The local evidence store (the absolute directory holding out/, normally the primary checkout)
/// for the test of the real manifest; without it out/ evidence paths are not judged (AG-200).
const EVIDENCE_ROOT_ENV: &str = "RSIA_SUPPORT_SCOPE_EVIDENCE_ROOT";
const MISSING_SCHEMA_VERSION: &str = "rsia.evidence_missing.v1";
const EVIDENCE_PATH_FIELDS: &[&str] = &["input_ref", "log_path", "record_source"];
/// The untracked local evidence store of the primary checkout (.gitignore /out/); a clone or
/// worktree does not carry it, so only paths outside it must exist in the checkout.
const LOCAL_EVIDENCE_STORE: &str = "out";
/// Harness² fixed sources (plan §16.8).
const HARNESS2_REPO: &str = "https://github.com/google-research/harness2";
const HARNESS2_COMMIT: &str = "1cfe254e8d25b15f37048c3730134686abe60335";
const HARNESS2_TREE: &str = "edc756a4d03a2826a248115482084a176e60e751";
const HSP01_PATH: &str = "arXiv:2610.14224v1，Section 2.2.1与Appendix F";
const EXPECTED_HSF: &[(&str, &str, &str)] = &[
    (
        "HSF01",
        "README.md",
        "a5e91206948cd3d5bbed30c2a951923020e780d2",
    ),
    (
        "HSF02",
        "LICENSE",
        "d645695673349e3947e8e5ae42332d0ac3164cd7",
    ),
    (
        "HSF03",
        "docs/EXPERIMENTS.md",
        "bd52e1506037d172a03c128427881d5342bbace7",
    ),
    (
        "HSF04",
        "harness2/core/recursion.py",
        "dda6b98cd5af100b8941f33cbb079462299c5280",
    ),
    (
        "HSF05",
        "harness2/core/selection.py",
        "2cf04f50e35fd82d7abaea279a83b4a7eb4c2193",
    ),
    (
        "HSF06",
        "harness2/core/prompts.py",
        "9ccacf23b64ac15b06a1939f26224960f56d757c",
    ),
    (
        "HSF07",
        "harness2/core/library.py",
        "39e50684b4044e483a3c16a447958044d4b5e6de",
    ),
    (
        "HSF08",
        "harness2/core/context.py",
        "72f2868b06c13e30c031cb91c0f04dcd80293ec9",
    ),
    (
        "HSF09",
        "harness2/core/schedule.py",
        "f91f450b1d01011982069f4b41e6e3a77abcc9cc",
    ),
    (
        "HSF10",
        "harness2/substrate/opencode.py",
        "ad0a2ea4d59bda362838c76912e43b6f7c333254",
    ),
    (
        "HSF11",
        "harness2/core/report.py",
        "b7ecf6e42a1061218b0eb8cde9c441c136e28e72",
    ),
    (
        "HSF12",
        "harness2/core/ledger.py",
        "319a6697758d05294dab9f42e8f8700759d7e6f6",
    ),
    (
        "HSF13",
        "harness2/core/tree.py",
        "c9bf1341226aac448f6ec82038cb6d3896799412",
    ),
    (
        "HSF14",
        "harness2/core/proposer.py",
        "4a235075cae72059fa701001f31bf4505cbbc3ba",
    ),
    (
        "HSF15",
        "harness2/demo.py",
        "487de02d44941754d955ad8a4b0a9e0e6f841db5",
    ),
    (
        "HSF16",
        "harness2/cli/run_sweep.py",
        "0876f4e68f3b92a4cf9d6838223c00e2ef960d27",
    ),
];
/// Harness² borrowings (plan §2.9): (id, section, hsf_refs, e_tasks, v_scenarios, adoption).
const EXPECTED_HS: &[(&str, &str, &str, &str, &str, &str)] = &[
    (
        "HS01",
        "§3.1.1/§6.7.5",
        "HSF04 HSF05",
        "E07 E09",
        "V122",
        "adopted",
    ),
    ("HS02", "§6.7.2/§6.7.5", "HSF06", "E09", "V122", "adopted"),
    (
        "HS03",
        "§3.6/§6.7.5",
        "HSF05",
        "E09 E13",
        "V091 V122",
        "adopted",
    ),
    (
        "HS04",
        "§5.8/§11.5",
        "HSF07 HSF09",
        "E03 E09",
        "V090",
        "adopted",
    ),
    ("HS05", "§6.7.3", "HSF05", "E09", "V122", "registered_only"),
    (
        "HS06",
        "§3.2/§6.3",
        "HSF08",
        "E03 E05",
        "V090",
        "registered_only",
    ),
    ("HS07", "§6.3", "HSF10", "E03 E16.1", "V052 V088", "adopted"),
    (
        "HS08",
        "§3.5/§3.6",
        "HSF11 HSF12",
        "E04 E07",
        "V096",
        "adopted",
    ),
];
const SKILLOPT_REPO: &str = "https://github.com/microsoft/SkillOpt.git";
const SKILLOPT_COMMIT: &str = "79124b37e9a6371e13b753f8bcd7adb1e493ade1";
const ALLOWED_STATUSES: &[&str] = &[
    "planned",
    "in_progress",
    "blocked",
    "implemented_not_verified",
    "verified",
    "explicitly_out_of_scope",
];
/// Records of pull requests numbered above this value (fifth round onward) must carry
/// `execution`; earlier records may omit it and are not backfilled or guessed.
const EXECUTION_REQUIRED_ABOVE_PR: u64 = 141;
const EXECUTION_KEYS: &[&str] = &["command", "evidence", "exit_code", "runner", "scope"];
const EXECUTION_RUNNERS: &[&str] = &[
    "ctrl_verify.sh",
    "ctrl_verify_v43.sh",
    "ctrl_verify_v45.sh",
    "codex_task",
    "other",
];
const EXECUTION_SCOPES: &[&str] = &["workspace", "single_target"];
/// `execution.command` must be what was run, not the declared single-target command copied again.
const DECLARED_COMMAND_PREFIX: &str = "cargo test --locked --offline";
const PRODUCTION_CONSUMER_VALUES: &[&str] = &["wired", "none_library_only"];

/// The reviewed V080.d pins shared by both checkers: withdrawn tuples, relation pairs, record
/// digests, missing-register triples and the register file SHA-256.
struct ReviewedPins {
    withdrawn: BTreeMap<String, (u64, String, String, String)>,
    relations: BTreeSet<(String, String, String)>,
    records: BTreeMap<String, String>,
    missing: BTreeSet<(String, String, String)>,
    register_sha256: String,
}

impl ReviewedPins {
    fn reviewed() -> Self {
        Self {
            withdrawn: EXPECTED_WITHDRAWN
                .iter()
                .map(|(id, pr, clause, owner, status)| {
                    (
                        id.to_string(),
                        (
                            *pr,
                            clause.to_string(),
                            owner.to_string(),
                            status.to_string(),
                        ),
                    )
                })
                .collect(),
            relations: EXPECTED_RELATIONS
                .iter()
                .map(|(kind, new, old)| (kind.to_string(), new.to_string(), old.to_string()))
                .collect(),
            records: EXPECTED_RECORD_DIGESTS
                .iter()
                .map(|(id, digest)| (id.to_string(), digest.to_string()))
                .collect(),
            missing: EXPECTED_MISSING_TRIPLES
                .iter()
                .map(|(id, field, path)| (id.to_string(), field.to_string(), path.to_string()))
                .collect(),
            register_sha256: EXPECTED_MISSING_REGISTER_SHA256.to_string(),
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_manifest(root: &Path) -> Value {
    let path = root.join("reports/support-scope.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("support-scope.json must exist at {path:?}: {error}"));
    load_json_strict(&content)
        .unwrap_or_else(|error| panic!("support-scope.json must be valid unique-key JSON: {error}"))
}

/// Preserve the existing Value API while rejecting duplicate keys in original bytes.
struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> serde::de::Visitor<'de> for UniqueVisitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(value.into())))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(value.into())))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueJson(Value::Number(number)))
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value.into())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueJson(value)) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(UniqueJson(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate JSON key: {key}"
                        )));
                    }
                    let UniqueJson(value) = map.next_value()?;
                    values.insert(key, value);
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

fn load_json_strict(content: &str) -> Result<Value, String> {
    serde_json::from_str::<UniqueJson>(content)
        .map(|value| value.0)
        .map_err(|error| format!("invalid unique-key JSON: {error}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectLicenseMetadata {
    schema_version: String,
    status: String,
    path: String,
    sha256: String,
    members: Vec<String>,
}

fn validate_project_license(value: &Value, root: &Path) -> Result<(), String> {
    const REVIEWED_SHA: &str = "164d42969d38c372e7cd85fe0daf749b6e8409fbd28afd36d1d2b098c5e9debb";
    let license = value["project_license"]
        .as_object()
        .ok_or("project_license must be an object")?;
    if license.keys().cloned().collect::<BTreeSet<_>>() != split_set("path sha256 status")
        || license["status"] != "source_available_personal_noncommercial_free"
        || license["path"] != "LICENSE"
        || license["sha256"].as_str() != Some(REVIEWED_SHA)
    {
        return Err("project_license must bind the reviewed license text".into());
    }
    validate_repo_file(root, "LICENSE", "project LICENSE")?;
    validate_repo_file(
        root,
        "scripts/check_support_scope.py",
        "project license checker",
    )?;
    let body = std::fs::read(root.join("LICENSE"))
        .map_err(|error| format!("cannot read LICENSE: {error}"))?;
    if evo_core::hash(&body) != REVIEWED_SHA {
        return Err("project LICENSE differs from the reviewed text".into());
    }
    let output = std::process::Command::new("python3")
        .arg(root.join("scripts/check_support_scope.py"))
        .arg("--repo-root")
        .arg(root)
        .arg("--project-license-metadata-only")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run read-only project metadata check: {error}"))?;
    if !output.status.success() {
        return Err("read-only project metadata check failed".into());
    }
    let raw = std::str::from_utf8(&output.stdout).map_err(|_| "project metadata is not UTF-8")?;
    // The derived strict struct rejects duplicate/unknown fields and trailing JSON.
    let metadata: ProjectLicenseMetadata =
        serde_json::from_str(raw).map_err(|error| format!("invalid project metadata: {error}"))?;
    let expected: Vec<String> = [
        "apps/rsia",
        "crates/evo-core",
        "crates/evo-engine",
        "crates/evo-http",
        "crates/evo-mcp",
        "crates/evo-storage",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if metadata.schema_version != "rsia.project_license_metadata.v1"
        || metadata.status != "source_available_personal_noncommercial_free"
        || metadata.path != "LICENSE"
        || metadata.sha256 != REVIEWED_SHA
        || metadata.members != expected
    {
        return Err("project metadata does not match LICENSE and six workspace members".into());
    }
    Ok(())
}

fn expected_e_scenarios() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    [
        ("E00", "V001 V071 V072 V073 V080 V098 V113 V116 V123"),
        ("E01", "V010 V011 V012 V013 V084 V085 V096 V097 V103 V114 V118 V122"),
        ("E02", "V002 V003 V005 V009 V043 V044 V045 V046 V047 V048 V060 V062 V078 V079 V081 V082 V083 V084 V085 V086 V087 V088 V089 V090 V091 V092 V093 V094 V095 V096 V100 V101 V107 V108 V109 V120"),
        ("E03", "V004 V005 V006 V017 V051 V052 V054 V055 V056 V057 V058 V087 V088 V089 V090 V094 V096 V106 V108 V121"),
        ("E04", "V007 V008 V009 V038 V083 V085 V086 V087 V093 V094 V096 V101 V103 V106 V110 V111 V112"),
        ("E05", "V010 V011 V012 V013 V028 V048 V084 V085 V090 V091 V094 V097 V101 V104 V114"),
        ("E06", "V014 V015 V016 V017 V047 V049 V050 V059 V064 V070 V075 V077 V078 V084 V085 V089 V091 V094 V095 V096 V100 V108 V119"),
        ("E07", "V003 V010 V014 V015 V016 V042 V047 V059 V070 V077 V087 V088 V089 V091 V096 V097 V112 V114 V121 V122"),
        ("E08", "V017 V018 V038 V056 V069 V075 V081 V082 V083 V084 V085 V086 V090 V092 V094 V096 V099 V100 V102 V110 V111"),
        ("E09", "V019 V020 V022 V024 V025 V081 V086 V088 V089 V090 V091 V093 V094 V095 V096 V097 V100 V105 V106 V121 V122"),
        ("E10", "V020 V021 V022 V023 V025 V026 V027 V081 V086 V094 V096 V104 V105 V106"),
        ("E11", "V022 V026 V027 V028 V042 V081 V094 V096 V097 V104 V118"),
        ("E12", "V029 V030 V031 V034 V082 V083 V093 V094 V096 V109"),
        ("E13", "V016 V032 V037 V038 V082 V084 V087 V090 V091 V094 V096 V097 V100 V109 V121"),
        ("E14", "V015 V033 V034 V035 V036 V086 V090 V092 V094 V096 V097 V118 V119"),
        ("E15", "V026 V033 V035 V036 V042 V085 V092 V096 V097 V118"),
        ("E16", "V081 V082 V083 V084 V085 V086 V087 V088 V089 V090 V091 V092 V093 V094 V095 V096 V097 V098 V099 V100 V101 V102 V103 V104 V105 V106 V107 V108 V109 V110 V111 V112 V113 V114 V115 V116 V117 V118 V119 V120 V121 V122 V123"),
        ("E16.1", "V005 V006 V017 V051 V052 V053 V054 V055 V056 V076 V087 V090 V098 V117"),
        ("E16.2", "V017 V039 V066 V067 V068 V069 V070 V075 V091 V092 V098 V102 V117"),
        ("E16.3", "V014 V018 V038 V063 V064 V065 V078 V091 V092 V098 V115"),
        ("E16.4", "V002 V003 V009 V037 V039 V060 V061 V062 V077 V087 V091 V098 V101 V107 V108"),
        ("E16.5", "V007 V008 V017 V018 V037 V038 V039 V066 V069 V073 V075 V081 V082 V083 V084 V085 V086 V089 V090 V092 V094 V096 V098 V099 V101 V102 V104 V110 V113 V116 V120 V123"),
        ("E16.6", "V001 V039 V042 V071 V072 V073 V074 V080 V098 V116 V123"),
        ("E17", "V040"),
        ("E18", "V041"),
    ]
    .into_iter()
    .map(|(task, scenarios)| (task, scenarios.split_whitespace().collect()))
    .collect()
}

fn expected_e_ids() -> BTreeSet<String> {
    (0..=18)
        .map(|index| format!("E{index:02}"))
        .chain((1..=6).map(|index| format!("E16.{index}")))
        .collect()
}

fn string_set(value: &Value, field: &str) -> Result<BTreeSet<String>, String> {
    let array = value
        .as_array()
        .ok_or_else(|| format!("{field} must be an array"))?;
    let mut result = BTreeSet::new();
    for item in array {
        let text = item
            .as_str()
            .ok_or_else(|| format!("{field} contains a non-string entry"))?;
        if !result.insert(text.to_string()) {
            return Err(format!("{field} contains duplicate {text}"));
        }
    }
    Ok(result)
}

fn nonempty_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| format!("{field} must be a non-empty string"))
}

fn lower_hex(value: &Value, len: usize, field: &str) -> Result<(), String> {
    let text = nonempty_string(value, field)?;
    if text.len() != len
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "{field} must be {len} lowercase hexadecimal characters"
        ));
    }
    Ok(())
}

fn safe_relative(path: &str, field: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if path.is_absolute() || path.components().next().is_none() {
        return Err(format!("{field} must be a non-empty relative path"));
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "{field} contains traversal or non-normal components"
        ));
    }
    Ok(path.to_path_buf())
}

fn validate_repo_file(root: &Path, relative: &str, field: &str) -> Result<(), String> {
    let relative = safe_relative(relative, field)?;
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(format!("{field} contains a non-normal component"));
        };
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| format!("{field} does not exist: {}", relative.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "{field} contains a symlink component: {}",
                relative.display()
            ));
        }
    }
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| format!("{field} does not exist: {}", relative.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "{field} is not a regular file: {}",
            relative.display()
        ));
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize repo root: {error}"))?;
    let canonical_path = path
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize {field}: {error}"))?;
    if !canonical_path.starts_with(canonical_root) {
        return Err(format!("{field} escapes the repository"));
    }
    Ok(())
}

fn valid_ident(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Reuse the reviewed stdlib TOML reader in isolated metadata-only mode. The code
/// path is this checkout's checker, never a helper supplied by the inspected root.
/// Python >=3.11 and this shared-parser contract remain subject to second acceptance.
fn workspace_packages(root: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let checker_root = repo_root()
        .canonicalize()
        .map_err(|error| format!("cannot locate workspace checker: {error}"))?;
    validate_repo_file(
        &checker_root,
        "scripts/check_support_scope.py",
        "workspace metadata checker",
    )?;
    let output = std::process::Command::new("python3")
        .args(["-I", "-B"])
        .arg(checker_root.join("scripts/check_support_scope.py"))
        .arg("--repo-root")
        .arg(root)
        .arg("--workspace-packages-only")
        .current_dir(&checker_root)
        .output()
        .map_err(|error| format!("cannot read workspace packages: {error}"))?;
    if !output.status.success() {
        return Err("read-only workspace package check failed".into());
    }
    let raw = std::str::from_utf8(&output.stdout)
        .map_err(|_| "workspace package metadata is not UTF-8")?;
    decode_workspace_packages(raw, root)
}

fn decode_workspace_packages(raw: &str, root: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let value = load_json_strict(raw)?;
    let object = value
        .as_object()
        .filter(|value| !value.is_empty())
        .ok_or("workspace package metadata must be a nonempty object")?;
    let mut packages = BTreeMap::new();
    let mut members = BTreeSet::new();
    for (name, member) in object {
        if !valid_ident(name) {
            return Err("workspace package metadata has an invalid package name".into());
        }
        let relative = nonempty_string(member, "workspace member")?;
        if relative.contains('\\')
            || relative
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | ".."))
        {
            return Err("workspace package metadata has a non-normal member".into());
        }
        let member = safe_relative(relative, "workspace member")?;
        validate_repo_file(root, &format!("{relative}/Cargo.toml"), "member Cargo.toml")?;
        if !members.insert(member.clone()) || packages.insert(name.clone(), member).is_some() {
            return Err("workspace package metadata has a duplicate member/package".into());
        }
    }
    Ok(packages)
}

struct ResolvedTestCommand {
    target: PathBuf,
    member: PathBuf,
}

fn resolve_test_command(
    root: &Path,
    command: &str,
    packages: &BTreeMap<String, PathBuf>,
) -> Result<ResolvedTestCommand, String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.len() != 8
        || tokens[0] != "cargo"
        || tokens[1] != "test"
        || tokens[2] != "--locked"
        || tokens[3] != "--offline"
        || tokens[4] != "-p"
    {
        return Err("command must use the finite cargo test grammar".into());
    }
    let package = tokens[5];
    let member = packages
        .get(package)
        .ok_or_else(|| format!("cargo package {package} is not a declared workspace member"))?
        .clone();
    match tokens[6] {
        "--test" => {
            let target = tokens[7];
            if !valid_ident(target) {
                return Err("integration test target is invalid".into());
            }
            let path = member.join("tests").join(format!("{target}.rs"));
            validate_repo_file(root, path.to_str().unwrap(), "test command target")?;
            Ok(ResolvedTestCommand {
                target: path,
                member,
            })
        }
        "--lib" => {
            let filter = tokens[7];
            let module = filter
                .strip_suffix("::tests")
                .ok_or("--lib filter must end in ::tests")?;
            if module.is_empty() || !module.split("::").all(valid_ident) {
                return Err("--lib module filter is invalid".into());
            }
            let first = module.split("::").next().unwrap();
            let lib = root.join(&member).join("src/lib.rs");
            let lib_body = std::fs::read_to_string(&lib)
                .map_err(|_| format!("package {package} has no readable src/lib.rs"))?;
            if !lib_body.contains(&format!("mod {first};")) {
                return Err(format!("module {first} is not declared by {package}"));
            }
            let path = member
                .join("src")
                .join(format!("{}.rs", module.replace("::", "/")));
            validate_repo_file(root, path.to_str().unwrap(), "test command target")?;
            Ok(ResolvedTestCommand {
                target: path,
                member,
            })
        }
        _ => Err("command must contain exactly one --test or --lib target".into()),
    }
}

fn expected_merge_for_pr(pr: u64) -> Result<Option<&'static str>, String> {
    Ok(match pr {
        29 => Some("28f892c3d68591d201a8029345257f051fb123ef"),
        30 => Some("faec991036f03f27cf6d2ad3db86f841815fc828"),
        31 => Some("dfc861ce7572f8f9f49a8aaf9f0cc1687cba57f9"),
        32 => Some("9b13237af884c525652813ea508eb8c5f100d6f8"),
        33 => Some("c1cf6a2e91253218033b5883a11baa02c85a5d43"),
        34 => Some("63824fff2e8d0cd0767d31140d064972542f6b53"),
        35 => Some("c017e6a32da3cdf692c3321dd29d6510edada0c4"),
        36 => Some("794cf0f517a57e48166ef2ad44cb5762f3a61d5e"),
        37 => Some("b0662ca1b7a2fe27d4a4622918679eed4934e214"),
        38 => Some("7a8a35908a2f7cc72ed601cbc902a2e4b49120bf"),
        39 => Some("96e62412809610a72d2ab593aaca59a8bcebd6d2"),
        40 => Some("09b3ebe8bad12d59bd3c2f926d3050802372e96f"),
        41 => Some("b1127a228c4a40248631ed417aba0ad7ea1a0884"),
        42 => Some("2e209dcea36d63610c59d3797e039f35d063ec89"),
        43 => Some("59afd1663a0d5f5212077dd92beaaf7920cde0ff"),
        44 => Some("acda31895bb1cb42cf7985b907a4c600429573d0"),
        47 => Some("1c824e6614667da4dc7ea74e96b46adfd1d2c089"),
        48 => Some("4980aa498baa80fd82e83441839e606859fa4065"),
        50 => Some("e94042d3f2fd761455727a6c82ad13fd3d657d03"),
        51 => Some("bef7bd1763a01ccb677ada4acad71a063366539c"),
        54 => Some("16c2817bc192bf71535d2c99d867873e6e85bcf8"),
        60 => Some("dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b"),
        61 => Some("b220971c4e5ad44453c9d3e5787fbdfda6eb01ac"),
        62 => Some("9c3ec979fd2ee3c53dfbf4ddf96967e94808b11e"),
        63 => Some("3e2fc067190077b8cf8401875d6c627b8d9e13fc"),
        64 => Some("f8fa1a19d5e968f83a52385ecb43d07d56442610"),
        66 => Some("5a0ef4419c51687534baa3296dbbf7e310a5d810"),
        67 => Some("5c4f81dca79373c6871449aed287f61fb1f3cbc2"),
        69 => Some("31566ce87203b44c44b72986233af1c02fbe5cda"),
        70 => Some("c81b258b6f890ab78bc9c7111798f75560c2ea23"),
        71 => Some("9519c26f049afff63154fb2337893217b18b7e3c"),
        72 => Some("15b140ddef25c8e794d586fecae56f129226a5c6"),
        73 => Some("7ad4994c7a9208f523612d305660dc8ca9529620"),
        74 => Some("c72aae14c9f35c1bcc090b5a374d2696d57dacfc"),
        75 => Some("6a23cb3697f656e1f5132cbe62ae766b4c0383f0"),
        76 => Some("7120842a1f586ae394b76e146c8298f11bec52bc"),
        77 => Some("887b1d714581ae8a71d45243d38be1759697600d"),
        78 => Some("84d8a7054c2428db0f821fcd478c615b09ade76b"),
        79 => Some("b262bbb2a0ea9ead68bc33fe1e58235e85bcffea"),
        80 => Some("9e3e350ed5fbef2975117f12a7f8d3ca3b9cdcbf"),
        81 => Some("d9b3f6f5234ad14da18956a78c804e4c6818fa10"),
        82 => Some("8d959dee844fb16f0763b0684ba3adf48ad75f1f"),
        83 => Some("51db9b6c9a251def4e6566c96641ed8b63d43a85"),
        84 => Some("42f89bd602db0c2783de38d87d91e80cd65f220b"),
        85 => Some("f4b1ef66340412f5c0edf69a48549cd1386324cf"),
        86 => Some("16bdb08e1ac118e3e732a912358add468c49173b"),
        87 => Some("32a2f5090fe5136b21c7d3454f53964b36990247"),
        88 => Some("ca117c45d87986500b38bc5afee3d6e161fea4f5"),
        89 => Some("cc85b5c71cec06763909fb0efa33c4e296b6178d"),
        90 => Some("ecdfe31faabb21a368f08d341b2a6e15223b0849"),
        91 => Some("a38b1452ede8fc5755bcab65686edd2f2817b51a"),
        92 => Some("696252faf8ef2b2391102f4e6b0e9c0ee20c53d5"),
        93 => Some("81449ccd1a3a9f73b464438d8a6edd26a0ff9734"),
        94 => Some("4a8487413cb8fe51ee16b91a538a71816997adfb"),
        95 => Some("e2c9107142413c0ba304b5090692b56037958f40"),
        96 => Some("49d5b85ec4fa757144213573c585087798324e91"),
        97 => Some("804302602efe575d92973be20c532d264019f9aa"),
        99 => Some("3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360"),
        100 => Some("43f0022478d58dd8a8e6492cffb3aa24463b3fce"),
        101 => Some("296b5a07a7bf911d02454b683d5732e9d38e3ae5"),
        103 => Some("ba01a2e8489ceef6d1350f0babdac46d816071ec"),
        104 => Some("9b08c4b2fd882abab65f48ea0af8f23b13a3b50d"),
        105 => Some("5337f4b17558b4c95176885d39f2518142d965b9"),
        106 => Some("279ee7c5290fabbb20cbd16b7e53c5ac2b71c188"),
        108 => Some("6f0e5fa35b582f0f4cde4207ddf59d65a51b180a"),
        109 => Some("c6e13e906ea888916294e6deac77c37f4391225f"),
        110 => Some("eb275812f09dd8850ad4edac08a2282cb5e82112"),
        111 => Some("200c43bc19368ff52b064e67eb1b14e35f096d39"),
        113 => Some("212c78488b403397eaebaa65a6d280b9c6be3899"),
        114 => Some("c0c4dbd3f6f0464c6cb492c0eb298496fd88b9de"),
        116 => Some("7fa290883136f30fcdc87ff5a4de293aa0714507"),
        117 => Some("ddde63c6ab98a2528258d265999bb902a08b0f55"),
        118 => Some("5ee01d1e2ebb2401fe94530ff286801239940259"),
        119 => Some("06fb166b006e5479a80e04dc14998109c30b0d42"),
        121 => Some("9f0c40215a2d7a67ba66e5427cee71ca5228bde1"),
        122 => Some("90d3dcbef822b17abaffffbcc866b863336c4024"),
        123 => Some("618ad834ff73e878898fb64681a4efc323b84ac0"),
        124 => Some("5295769892af37988d6bc739bdfb49a834a3f7aa"),
        125 => Some("349879148c1f31fd1ebbee639aeff334a3762855"),
        126 => Some("ea02f116fe2172dcba621a60bf9b141237e1ba45"),
        127 => Some("aa51b1102e1886cbaf7b025d37b98ca30b6e2d3d"),
        128 => Some("1177c91931689794a7d28b614adc01d3a0435e46"),
        130 => Some("1132b6b4b6e0016540339ba65219daf1ceb35fb8"),
        131 => Some("aa831fba2d09d0ef2d0755c3d7a08971b8f6667f"),
        132 => Some("77152d77c96f497ebf2c0123e985f0832fa3cbd0"),
        133 => Some("8cbf82cb9a715a8b28070c6a7823b16093865425"),
        134 => Some("bd20099a01c64b87b9a75707a95c785486bb4fda"),
        135 => Some("c9aa71e7516e369672b0e5127e675d72eb199304"),
        136 => Some("1f1b593d5475043e707e17195222d82f2c5281eb"),
        137 => Some("27df8c2cfccbefa6544b55b697c10abbb6c0a61c"),
        138 => Some("77ef86294b7760fc7495d12d4029ca8a3567bc5e"),
        140 => Some("f6904dcd2d4f3b46b3311886fd4dcecd8a2f711c"),
        144 => Some("184764302e64ca3871fc68103bd07abec1125918"),
        145 => Some("7d39ef7350615be82d698d6914d03856fa0df97c"),
        146 => Some("98776f1717b37a47e4a7fcc4349ff0faff68ce1b"),
        147 => Some("1706cb04049c0306676a17fd34fd021717c5669b"),
        149 => Some("7dc89bffc49d8fe4c5d97e7ccd56d3d0ad597f1c"),
        150 => Some("94076d789f40347ab8263176044f041824c061fb"),
        151 => Some("63113ac95b01f78863da0f75c5a93e5bfa2dd9d8"),
        152 => Some("f355ec389513685097e2c4d85dfd0c062e29e915"),
        153 => Some("42decd1a17585c366b249104b0bbea700361d003"),
        154 => Some("aece7299d0078e5704bd74ed3ae04e828f3e2c6d"),
        157 => Some("647c7ce8361215632d62a42a8bf87d9df2bf3c2d"),
        158 => Some("4036c0d2d11a01ddcdcb2940d0b00f0f00975656"),
        159 => Some("a78a1e60a4298cd3ca57598a7aba7c5c84a8a721"),
        161 => Some("5401effdfadf048dd4537fdf1e1ca843b9482ac3"),
        162 => Some("5718bf5c0389588781b8dd2e76518da7daab6fd5"),
        163 => Some("4d1fd6cb43fbe64db81177d336b6dae3f061ca98"),
        165 => Some("2c35d2e0f663064de2f5ee76c46154db7a50cf63"),
        166 => Some("40c70df8e71e7d23a3f2ff373852451604b48ec4"),
        167 => Some("1ce4c0fc00b46f72d91b7595bd7911a9f3627c67"),
        168 => Some("9faa0b529fdb600b8c0e1e6d6cc866d278889353"),
        169 => Some("58c013572c806dc464a248331aeca41bcc4226c3"),
        170 => Some("89b3c20427e8b82f07ec470d9cd4f1435f0df7ac"),
        171 => Some("fbc8c5545760a17242c6ee588d6c9340d1d97300"),
        172 => Some("557fd81882123e2dc1f48e82c23f75e40c1c54a9"),
        173 => Some("59c0af57ee0fec291b12d5a7dab3ba62ca726579"),
        174 => Some("a5039eb21558cb466a0ee00a1b2ba022b11c12d1"),
        175 => Some("14f832e7bb9e321eafc38097538e9267183d5069"),
        178 => Some("8f38f4e39cceb67976f9120dc3f76fecce232c5c"),
        180 => Some("daa618a6a78cdb815102d22c57026d16e9d7aa90"),
        187 => Some("b166053d8fcf0fd8d046d3274e132a883b95e0bd"),
        188 => Some("85612648c313a032ea369b56b62e593203e1abff"),
        189 => Some("86221ff7a9ecf678d86f4c733befd4595cffd55e"),
        191 => Some("fc481354c047d63f90b687f1fef77b6f38ff9471"),
        192 => Some("1f624445d66caa8a854b9fa78abff4264439f505"),
        194 => Some("e9a90ad3acee6e9191cce75a3a808551b083e8f4"),
        195 => Some("10f172eab7b97c5790bec88a3c6de2326c6e9cc5"),
        197 => Some("a730b9a9935bee7a41e8a47e86f329b99012566b"),
        198 => Some("e012bba09cb662e9c01d61b52124a709106f75b8"),
        199 => Some("a389c4e32e315ff516eba996a0a3dd40faadba0f"),
        200 => Some("02a135aa918fce5dd2ea0c1c8eafc287318f10d2"),
        _ => return Err(format!("no reviewed merge status for PR {pr}")),
    })
}

/// Record-level `execution` states what was literally run, apart from the declared single target
/// in `command`/`exit_code`/`log_path`. Required above PR 141; checked strictly whenever present.
fn validate_execution(id: &str, record: &Value, pr_number: u64) -> Result<(), String> {
    let Some(execution) = record.get("execution") else {
        if pr_number > EXECUTION_REQUIRED_ABOVE_PR {
            return Err(format!(
                "{id}: PR #{pr_number} is above #{EXECUTION_REQUIRED_ABOVE_PR}, so the record must carry an execution object stating what was literally run"
            ));
        }
        return Ok(());
    };
    let object = execution
        .as_object()
        .ok_or_else(|| format!("{id} execution must be an object"))?;
    let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = EXECUTION_KEYS.iter().copied().collect();
    if keys != expected {
        return Err(format!(
            "{id} execution keys differ: missing={:?} extra={:?}",
            expected.difference(&keys).collect::<Vec<_>>(),
            keys.difference(&expected).collect::<Vec<_>>()
        ));
    }
    for (field, allowed) in [("runner", EXECUTION_RUNNERS), ("scope", EXECUTION_SCOPES)] {
        match object[field].as_str() {
            Some(text) if allowed.contains(&text) => {}
            _ => {
                return Err(format!(
                    "{id} execution {field} must be one of {allowed:?}, got {}",
                    object[field]
                ));
            }
        }
    }
    let command = nonempty_string(&object["command"], &format!("{id} execution command"))?;
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.starts_with(DECLARED_COMMAND_PREFIX) {
        return Err(format!(
            "{id} execution command must be the literally executed command, not the declared single-target form copied again"
        ));
    }
    let exit_code = &object["exit_code"];
    let is_integer = exit_code.is_i64() || exit_code.is_u64();
    if !is_integer {
        return Err(format!(
            "{id} execution exit_code must be the integer exit code of the executed command, got {exit_code}"
        ));
    }
    nonempty_string(&object["evidence"], &format!("{id} execution evidence"))?;
    Ok(())
}

/// Optional marker separating modules with a production caller from library-only ones; an absent
/// marker claims nothing.
fn validate_production_consumer(id: &str, record: &Value) -> Result<(), String> {
    let Some(value) = record.get("production_consumer") else {
        return Ok(());
    };
    match value.as_str() {
        Some(text) if PRODUCTION_CONSUMER_VALUES.contains(&text) => Ok(()),
        _ => Err(format!(
            "{id} production_consumer must be one of {PRODUCTION_CONSUMER_VALUES:?} when present, got {value}"
        )),
    }
}

fn valid_evidence_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// Owner task by the longest registered E-task prefix (E16.5.x belongs to E16.5, not E16).
fn evidence_task(id: &str) -> Option<String> {
    expected_e_ids()
        .into_iter()
        .filter(|candidate| {
            id.strip_prefix(candidate.as_str())
                .is_some_and(|suffix| suffix.starts_with('.'))
        })
        .max_by_key(String::len)
}

fn validate_evidence_namespace(id: &str, task: &str) -> Result<(), String> {
    if !valid_evidence_id(id) {
        return Err(format!("{task} evidence id is invalid"));
    }
    let owner = evidence_task(id);
    if owner.as_deref() != Some(task) {
        return Err(format!(
            "{task} evidence id {id} is not namespaced under its own task"
        ));
    }
    Ok(())
}

fn test_entry_belongs_to_task(entry: &str, member: &Path, files: &[Value]) -> bool {
    files.iter().any(|file| {
        file.as_str()
            .is_some_and(|file| file == entry || Path::new(file).starts_with(member))
    })
}

fn validate_evidence_refs(
    value: &Value,
    field: &str,
    tasks: &BTreeSet<String>,
    owners: &BTreeMap<String, String>,
) -> Result<(), String> {
    for reference in string_set(value, field)? {
        let owner = owners
            .get(&reference)
            .ok_or_else(|| format!("{field} references unknown evidence {reference}"))?;
        if !tasks.contains(owner) {
            return Err(format!(
                "{field} references evidence {reference} owned by unrelated task {owner}"
            ));
        }
    }
    Ok(())
}

fn validate_evidence_record(
    record: &Value,
    task: &str,
    root: &Path,
    implementation_files: &[Value],
    packages: &BTreeMap<String, PathBuf>,
) -> Result<String, String> {
    let id = nonempty_string(&record["id"], &format!("{task} evidence id"))?;
    validate_evidence_namespace(id, task)?;
    nonempty_string(&record["description"], &format!("{id} description"))?;
    let version = nonempty_string(&record["plan_version"], &format!("{id} plan_version"))?;
    let digest = nonempty_string(&record["plan_sha256"], &format!("{id} plan_sha256"))?;
    let Some((_, _, registered)) = PLAN_LINEAGE.iter().find(|(known, _, _)| *known == version)
    else {
        return Err(format!(
            "{id} plan_version {version} is outside the registered plan lineage"
        ));
    };
    if digest != *registered {
        return Err(format!(
            "{id} plan_sha256 is not the registered digest of plan {version}"
        ));
    }
    let recheck = match record.get("plan_recheck") {
        None => "not_affected",
        Some(value) => nonempty_string(value, &format!("{id} plan_recheck"))?,
    };
    if !PLAN_RECHECK_VALUES.contains(&recheck) {
        return Err(format!(
            "{id} plan_recheck {recheck} is not a registered marker"
        ));
    }
    if recheck == "required" {
        return Err(format!(
            "{id} still requires a recheck against plan {PLAN_VERSION}"
        ));
    }
    let position = |wanted: &str| {
        PLAN_LINEAGE
            .iter()
            .position(|(known, _, _)| *known == wanted)
    };
    if let Some(marker) = recheck.strip_prefix("rechecked_")
        && position(marker) <= position(version)
    {
        return Err(format!(
            "{id} plan_recheck {recheck} does not name a plan later than the record's own plan_version {version}"
        ));
    }
    if version == PLAN_VERSION {
        if record.get("plan_recheck").is_none() || recheck != "not_affected" {
            return Err(format!(
                "{id} is bound to the current plan {PLAN_VERSION} and must carry plan_recheck not_affected explicitly (plan §18.5)"
            ));
        }
    } else if PLAN_RECHECK_TASKS.contains(&task) {
        if recheck != PLAN_RECHECKED {
            return Err(format!(
                "{id} belongs to {task}, which the {PLAN_VERSION} revision touches, but was not rechecked"
            ));
        }
    } else if recheck == PLAN_RECHECKED {
        return Err(format!(
            "{id} plan_recheck {PLAN_RECHECKED} is valid only inside the {PLAN_VERSION} recheck union; {task} is outside it and keeps its historical marker"
        ));
    } else if V43_RECHECK_FROM.contains(&version) && V43_RECHECK_TASKS.contains(&task) {
        if recheck != V43_RECHECKED {
            return Err(format!(
                "{id} belongs to {task}, which is outside the {PLAN_VERSION} recheck union but was rechecked by v4.3, so it keeps plan_recheck {V43_RECHECKED}"
            ));
        }
    } else if recheck != "not_affected" {
        return Err(format!(
            "{id} plan_recheck {recheck} names a recheck no registered revision applied to {task} at plan {version}; it keeps not_affected"
        ));
    }
    let mut related = BTreeSet::new();
    for field in ["supersedes", "completes"] {
        if let Some(value) = record.get(field) {
            for other in string_set(value, &format!("{id} {field}"))? {
                if !valid_evidence_id(&other) {
                    return Err(format!("{id} {field} lists an invalid record id"));
                }
                if other == id {
                    return Err(format!("{id} {field} names the record itself"));
                }
                if !related.insert(other.clone()) {
                    return Err(format!("{id} cannot both supersede and complete {other}"));
                }
            }
        }
    }
    if (record.get("supersedes").is_some() || record.get("completes").is_some())
        && position(version) < position(RELATION_MIN_VERSION)
    {
        return Err(format!(
            "{id} carries supersedes or completes, which exist since plan {RELATION_MIN_VERSION}; it must be bound to plan {RELATION_MIN_VERSION} or later"
        ));
    }
    if let Some(note) = record.get("note") {
        nonempty_string(note, &format!("{id} note"))?;
    }
    lower_hex(&record["source_sha"], 40, &format!("{id} source_sha"))?;
    let pr = nonempty_string(&record["pr"], &format!("{id} pr"))?;
    let prefix = "https://github.com/acosmi/RSIAgent/pull/";
    let pr_number = pr
        .strip_prefix(prefix)
        .filter(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| format!("{id} has an invalid pull request URL"))?
        .parse::<u64>()
        .map_err(|_| format!("{id} has an invalid pull request number"))?;
    if !record["merged_sha"].is_null() && record["merged_sha"] == record["source_sha"] {
        return Err(format!(
            "{id} merged_sha must differ from its own source_sha"
        ));
    }
    let expected_merge = expected_merge_for_pr(pr_number)?;
    match expected_merge {
        Some(expected) => {
            lower_hex(&record["merged_sha"], 40, &format!("{id} merged_sha"))?;
            if record["merged_sha"] != expected {
                return Err(format!(
                    "{id} merged_sha differs from the reviewed PR merge"
                ));
            }
        }
        None if !record["merged_sha"].is_null() => {
            return Err(format!("{id} records a merge for an unmerged PR"));
        }
        None => {}
    }
    lower_hex(&record["input_digest"], 64, &format!("{id} input_digest"))?;
    if record["exit_code"].as_i64() != Some(0) {
        return Err(format!("{id} verified execution must have exit_code 0"));
    }
    for field in ["actual_result", "scope", "risk", "rollback"] {
        nonempty_string(&record[field], &format!("{id} {field}"))?;
    }
    let log = nonempty_string(&record["log_path"], &format!("{id} log_path"))?;
    safe_relative(log, &format!("{id} log_path"))?;
    let input_ref = nonempty_string(&record["input_ref"], &format!("{id} input_ref"))?;
    safe_relative(input_ref, &format!("{id} input_ref"))?;
    let source = nonempty_string(&record["record_source"], &format!("{id} record_source"))?;
    safe_relative(source, &format!("{id} record_source"))?;
    let entry = nonempty_string(&record["test_entry"], &format!("{id} test_entry"))?;
    validate_repo_file(root, entry, &format!("{id} test_entry"))?;
    let command = nonempty_string(&record["command"], &format!("{id} command"))?;
    let resolved = resolve_test_command(root, command, packages)?;
    if resolved.target.as_path() != Path::new(entry) {
        return Err(format!(
            "{id} command target {} differs from test_entry {entry}",
            resolved.target.display()
        ));
    }
    if !test_entry_belongs_to_task(entry, &resolved.member, implementation_files) {
        return Err(format!(
            "{id} test_entry is not part of the task's implementation crate/files"
        ));
    }
    validate_execution(id, record, pr_number)?;
    validate_production_consumer(id, record)?;
    Ok(id.to_string())
}

fn validate_e_scopes(value: &Value, root: &Path) -> Result<BTreeMap<String, String>, String> {
    let object = value.as_object().ok_or("e_scopes must be an object")?;
    let actual: BTreeSet<String> = object.keys().cloned().collect();
    let expected = expected_e_ids();
    if actual != expected {
        return Err(format!(
            "e_scopes exact set mismatch: missing={:?} extra={:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        ));
    }
    let expected_scenarios = expected_e_scenarios();
    let mut evidence_ids = BTreeMap::new();
    let packages = workspace_packages(root)?;
    for (task, entry) in object {
        nonempty_string(&entry["title"], &format!("{task} title"))?;
        let status = nonempty_string(&entry["status"], &format!("{task} status"))?;
        if !ALLOWED_STATUSES.contains(&status) {
            return Err(format!("{task} has unknown status {status}"));
        }
        nonempty_string(&entry["status_reason"], &format!("{task} status_reason"))?;
        let files = entry["implementation_files"]
            .as_array()
            .ok_or_else(|| format!("{task} implementation_files must be an array"))?;
        if status != "planned" && files.is_empty() {
            return Err(format!(
                "{task} non-planned scope needs implementation_files"
            ));
        }
        for file in files {
            let file = nonempty_string(file, &format!("{task} implementation file"))?;
            validate_repo_file(root, file, &format!("{task} implementation file"))?;
        }
        let actual_scenarios = string_set(&entry["scenarios"], &format!("{task} scenarios"))?;
        let expected_for_task: BTreeSet<String> = expected_scenarios[task.as_str()]
            .iter()
            .map(|value| value.to_string())
            .collect();
        if actual_scenarios != expected_for_task {
            return Err(format!(
                "{task} scenario responsibility differs from plan {PLAN_VERSION}"
            ));
        }
        let remaining = entry["remaining"]
            .as_array()
            .ok_or_else(|| format!("{task} remaining must be an array"))?;
        if status == "verified" && !remaining.is_empty() {
            return Err(format!(
                "{task} cannot be verified while remaining is non-empty"
            ));
        }
        if status != "verified" && remaining.is_empty() {
            return Err(format!(
                "{task} non-verified scope must state remaining work"
            ));
        }
        for item in remaining {
            nonempty_string(item, &format!("{task} remaining item"))?;
        }
        let evidence = entry["verified_subscopes"]
            .as_array()
            .ok_or_else(|| format!("{task} verified_subscopes must be an array"))?;
        if status == "verified" && evidence.is_empty() {
            return Err(format!("{task} verified status lacks execution evidence"));
        }
        for record in evidence {
            let id = validate_evidence_record(record, task, root, files, &packages)?;
            if evidence_ids.insert(id.clone(), task.clone()).is_some() {
                return Err(format!("duplicate evidence id {id}"));
            }
        }
    }
    Ok(evidence_ids)
}

struct CommitmentExpected {
    id: &'static str,
    sections: &'static str,
    sources: &'static str,
    e_tasks: &'static str,
    scenarios: &'static str,
}

fn expected_commitments(kind: &str) -> Vec<CommitmentExpected> {
    let rows: &[(&str, &str, &str, &str, &str)] = match kind {
        "B" => &[
            ("B01", "§5.2", "RH01 RH02", "E02 E06", "V044 V048 V079"),
            ("B02", "§5.3", "RH03", "E02", "V043 V045 V046 V078"),
            (
                "B03",
                "§5.4 §6.5",
                "RH04",
                "E02 E06 E07",
                "V047 V049 V050 V059 V077",
            ),
            ("B04", "§5.5", "RH05", "E02 E16.4", "V060 V061 V062"),
            ("B05", "§6.3", "RH06", "E03", "V051 V052 V055 V056"),
            ("B06", "§6.4", "RH07", "E03 E14", "V054 V057 V058 V070"),
            (
                "B07",
                "§5.6 §6.3",
                "RH08",
                "E03 E16.1",
                "V052 V053 V054 V056 V076",
            ),
            ("B08", "§11.1", "RH09", "E06 E16.3", "V063 V064 V065 V078"),
            (
                "B09",
                "§6.5 §9",
                "RH01 RH07",
                "E06 E14 E15",
                "V015 V033 V036 V070",
            ),
            (
                "B10",
                "§11.2–§11.3",
                "RH01",
                "E08 E16.2 E16.6",
                "V066 V067 V068 V069 V073 V075",
            ),
        ],
        "U" => &[
            ("U01", "§7.1 §7.5", "", "E02 E09 E10 E11", "V025 V028 V081"),
            ("U02", "§7.5.1–§7.5.3", "", "E10 E11", "V021 V023 V027 V081"),
            ("U03", "§8.1", "", "E12 E13", "V029 V034 V082"),
            ("U04", "§8.2–§8.4", "", "E04 E12", "V007 V030 V031 V083"),
            ("U05", "§3.2 §10", "", "E01 E05 E06", "V010 V011 V084"),
            ("U06", "§3.4.1 §6.6", "", "E01 E04 E05", "V012 V013 V085"),
            ("U07", "§7.1.1 §7.2.1", "", "E09", "V020 V024 V086"),
            (
                "U08",
                "§1.4 §12 §14.2",
                "",
                "E00 E05 E09 E10 E12 E16.6",
                "V042 V062 V080 V081 V082 V083 V084 V085 V086",
            ),
        ],
        "K" => &[
            (
                "K01",
                "§6.4.1 §5.8",
                "SO01 SO06",
                "E02 E03 E04 E07 E13",
                "V087",
            ),
            ("K02", "§6.7.1–§6.7.2", "SO02 SO03", "E03 E09", "V088"),
            ("K03", "§5.3.1 §5.8", "SO04 SO05", "E02 E03 E06 E09", "V089"),
            (
                "K04",
                "§6.7.3 §5.8 §11.5",
                "SO01 SO02",
                "E03 E05 E08 E09 E13",
                "V090 V094",
            ),
            (
                "K05",
                "§11.5 §10.2",
                "SO01 SO07 SO10",
                "E05 E06 E09 E13",
                "V091",
            ),
            ("K06", "§9.1 §3.1.1", "SO01 SO08", "E14 E15", "V092 V097"),
            (
                "K07",
                "§8.5 §7.4.1",
                "SO12 SO13",
                "E04 E09 E10 E12",
                "V093 V094",
            ),
            ("K08", "§7.7", "SO11 SO14", "E06 E09", "V095"),
            (
                "K09",
                "§3.1.1 §3.6 §6.7.4",
                "SO13 SO15 SO16",
                "E01 E04 E07 E11 E15",
                "V096 V097",
            ),
            (
                "K10",
                "§2.7–§2.8 §16.5–§16.6 §18.7",
                "SO09 SO10 SO18",
                "E00 E05 E06 E16",
                "V091 V098",
            ),
        ],
        _ => unreachable!(),
    };
    rows.iter()
        .map(|row| CommitmentExpected {
            id: row.0,
            sections: row.1,
            sources: row.2,
            e_tasks: row.3,
            scenarios: row.4,
        })
        .collect()
}

fn split_set(values: &str) -> BTreeSet<String> {
    values.split_whitespace().map(str::to_string).collect()
}

fn expected_commitment_implementation_status(id: &str) -> &'static str {
    match id {
        "B09" | "K06" => "planned",
        "B04" | "B06" | "B07" | "B10" | "U01" | "U02" | "U03" | "U04" | "U07" | "U08" | "K01"
        | "K05" | "K07" | "K08" | "K09" | "K10" => "in_progress",
        _ => "implemented_not_verified",
    }
}

fn validate_commitments(
    kind: &str,
    value: &Value,
    root: &Path,
    e_ids: &BTreeSet<String>,
    v_ids: &BTreeSet<String>,
    so_ids: &BTreeSet<String>,
    evidence_ids: &BTreeMap<String, String>,
) -> Result<(), String> {
    let array = value
        .as_array()
        .ok_or_else(|| format!("{kind} commitments must be an array"))?;
    let expected = expected_commitments(kind);
    if array.len() != expected.len() {
        return Err(format!("{kind} commitments have the wrong count"));
    }
    let mut by_id = BTreeMap::new();
    for entry in array {
        let id = nonempty_string(&entry["id"], &format!("{kind} commitment id"))?;
        if by_id.insert(id, entry).is_some() {
            return Err(format!("duplicate commitment id {id}"));
        }
    }
    for expected in expected {
        let entry = by_id
            .get(expected.id)
            .ok_or_else(|| format!("missing commitment {}", expected.id))?;
        if entry["specification_status"] != "included" || entry["effect_status"] != "not_claimed" {
            return Err(format!(
                "{} conflates specification or effect status",
                expected.id
            ));
        }
        for field in ["implementation_status", "verification_status"] {
            let status = nonempty_string(&entry[field], &format!("{} {field}", expected.id))?;
            if !ALLOWED_STATUSES.contains(&status) {
                return Err(format!("{} has invalid {field}", expected.id));
            }
        }
        if entry["implementation_status"] != expected_commitment_implementation_status(expected.id)
        {
            return Err(format!(
                "{} implementation status hides a known consumer gap or completed subset",
                expected.id
            ));
        }
        let sections = string_set(&entry["sections"], &format!("{} sections", expected.id))?;
        let sources = string_set(&entry["sources"], &format!("{} sources", expected.id))?;
        let tasks = string_set(&entry["e_tasks"], &format!("{} e_tasks", expected.id))?;
        let scenarios = string_set(
            &entry["v_scenarios"],
            &format!("{} v_scenarios", expected.id),
        )?;
        if sections != split_set(expected.sections)
            || sources != split_set(expected.sources)
            || tasks != split_set(expected.e_tasks)
            || scenarios != split_set(expected.scenarios)
        {
            return Err(format!(
                "{} trace chain differs from plan {PLAN_VERSION}",
                expected.id
            ));
        }
        if !tasks.is_subset(e_ids) || !scenarios.is_subset(v_ids) {
            return Err(format!("{} points to an unknown E or V id", expected.id));
        }
        if kind == "K" && !sources.is_subset(so_ids) {
            return Err(format!("{} points to an unknown SO id", expected.id));
        }
        if kind == "B"
            && !sources.iter().all(|source| {
                matches!(
                    source.as_str(),
                    "RH01" | "RH02" | "RH03" | "RH04" | "RH05" | "RH06" | "RH07" | "RH08" | "RH09"
                )
            })
        {
            return Err(format!("{} points to an unknown RH id", expected.id));
        }
        let artifacts = entry["local_artifacts"]
            .as_array()
            .ok_or_else(|| format!("{} local_artifacts must be an array", expected.id))?;
        if artifacts.is_empty() {
            return Err(format!("{} has no local consumer/artifact", expected.id));
        }
        for artifact in artifacts {
            let artifact = nonempty_string(artifact, "local artifact")?;
            validate_repo_file(root, artifact, &format!("{} local artifact", expected.id))?;
        }
        validate_evidence_refs(
            &entry["related_evidence_refs"],
            &format!("{} related_evidence_refs", expected.id),
            &tasks,
            evidence_ids,
        )?;
        nonempty_string(&entry["remaining"], &format!("{} remaining", expected.id))?;
    }
    Ok(())
}

struct SoExpected {
    id: &'static str,
    path: &'static str,
    blob: &'static str,
    range: &'static str,
    tasks: &'static str,
}

fn expected_so() -> Vec<SoExpected> {
    [
        (
            "SO01",
            "skillopt/engine/trainer.py",
            "520a90eb8af422368c8e498b8487504bb8f6c279",
            "1–240、1270–2110",
            "E03 E05 E09 E13 E14",
        ),
        (
            "SO02",
            "skillopt/gradient/reflect.py",
            "df68c9b76de83b2465b267b04310399ec5f3cec8",
            "300–700",
            "E03 E09",
        ),
        (
            "SO03",
            "skillopt/gradient/aggregate.py",
            "14f0cd978f35afe579913c9b5677a803bbce9e70",
            "全文",
            "E03 E09",
        ),
        (
            "SO04",
            "skillopt/optimizer/clip.py",
            "a2ed965f7c81c18426e82e2fd49e589b0952a92f",
            "全文",
            "E02 E03 E09",
        ),
        (
            "SO05",
            "skillopt/optimizer/skill.py",
            "65d574157f47e9b3f415d4db94b0e5a9c09e4826",
            "全文",
            "E02 E03 E06 E09",
        ),
        (
            "SO06",
            "skillopt/optimizer/skill_aware.py",
            "de39427ed0cc85fa1e716363a22140c91090b2d6",
            "全文",
            "E03 E07 E13",
        ),
        (
            "SO07",
            "skillopt/optimizer/slow_update.py",
            "9f8dd52606c3ada4fea1e6c0f72ec38f8a5139e8",
            "1–260",
            "E09 E13",
        ),
        (
            "SO08",
            "skillopt/optimizer/meta_skill.py",
            "6e34ff10d90655bb620d3e63b1d2041338616a5c",
            "全文",
            "E14 E15",
        ),
        (
            "SO09",
            "skillopt/evaluation/gate.py",
            "10461a98756ebacbf7d5a8bbdc67657950e2c199",
            "全文",
            "E05 E06",
        ),
        (
            "SO10",
            "configs/_base_/default.yaml",
            "4fe64088dc8011a2121335155d4a79ae7f90e9f5",
            "全文；本轮重读90–150",
            "E00 E03 E05 E13",
        ),
        (
            "SO11",
            "skillopt_sleep/consolidate.py",
            "5b80b4a9825fb5b5a79b04fdad4c655d5f3228f6",
            "全文",
            "E05 E06 E09",
        ),
        (
            "SO12",
            "skillopt_sleep/rollout.py",
            "f889333b3eecb83693a7005154dedadccbdc07f2",
            "全文",
            "E09 E12",
        ),
        (
            "SO13",
            "skillopt_sleep/replay.py",
            "1502a71cf9069b8ea23293acbb080225cdc51de2",
            "全文",
            "E04 E09 E10 E12",
        ),
        (
            "SO14",
            "skillopt_sleep/multi_skill.py",
            "32247639849e2c327ab7a1b7b3258dc7c35f53d4",
            "全文",
            "E06 E09",
        ),
        (
            "SO15",
            "skillopt_sleep/budget.py",
            "48875ca0f01cf099967ca2b5a45891e903645e36",
            "全文",
            "E01 E04 E11",
        ),
        (
            "SO16",
            "docs/sleep/RESULTS.md",
            "441ffa28f8644631c275456c7ccf4e75fd1d8c99",
            "全文",
            "E01 E07 E15",
        ),
        (
            "SO17",
            "docs/guide/training-loop.md",
            "30393dca9533de90e0cd0ff95f26eb327bfec0ff",
            "全文",
            "E03 E09",
        ),
        (
            "SO18",
            "LICENSE",
            "cf7bcb2ba475e33648180599c0f83c425f1705d9",
            "全文；本轮重读及固定字节校验",
            "E00 E16.2 E16.5",
        ),
    ]
    .into_iter()
    .map(|row| SoExpected {
        id: row.0,
        path: row.1,
        blob: row.2,
        range: row.3,
        tasks: row.4,
    })
    .collect()
}

fn validate_so_sources(
    value: &Value,
    root: &Path,
    e_ids: &BTreeSet<String>,
    evidence_ids: &BTreeMap<String, String>,
) -> Result<BTreeSet<String>, String> {
    let array = value.as_array().ok_or("so_sources must be an array")?;
    let mut by_id = BTreeMap::new();
    for entry in array {
        let id = nonempty_string(&entry["id"], "SO id")?;
        if by_id.insert(id, entry).is_some() {
            return Err(format!("duplicate SO id {id}"));
        }
    }
    let expected = expected_so();
    if by_id.len() != expected.len() {
        return Err("SO01-SO18 exact set is incomplete".into());
    }
    for expected in &expected {
        let entry = by_id
            .get(expected.id)
            .ok_or_else(|| format!("missing {}", expected.id))?;
        if entry["repository"] != SKILLOPT_REPO
            || entry["commit"] != SKILLOPT_COMMIT
            || entry["path"] != expected.path
            || entry["blob_sha"] != expected.blob
            || entry["read_range"] != expected.range
            || entry["license"] != "MIT"
            || entry["license_status"] != "reference_pin_from_spec_not_reverified_by_cp001"
            || entry["verification_status"] != "reference_pins_only"
        {
            return Err(format!(
                "{} pin/provenance differs from plan {PLAN_VERSION}",
                expected.id
            ));
        }
        let tasks = string_set(&entry["e_tasks"], &format!("{} e_tasks", expected.id))?;
        if tasks != split_set(expected.tasks) || !tasks.is_subset(e_ids) {
            return Err(format!(
                "{} task mapping differs from plan {PLAN_VERSION}",
                expected.id
            ));
        }
        if let Some(refs) = entry.get("verification_evidence_refs") {
            validate_evidence_refs(
                refs,
                &format!("{} verification_evidence_refs", expected.id),
                &tasks,
                evidence_ids,
            )?;
        }
        if entry["local_use"]["mode"] != "reference_only_no_upstream_runtime_import" {
            return Err(format!(
                "{} must state reference-only local use",
                expected.id
            ));
        }
        let artifacts = entry["local_use"]["artifacts"]
            .as_array()
            .ok_or_else(|| format!("{} local artifacts missing", expected.id))?;
        if artifacts.is_empty() {
            return Err(format!("{} has no explicit local landing", expected.id));
        }
        for artifact in artifacts {
            validate_repo_file(
                root,
                nonempty_string(artifact, "SO local artifact")?,
                &format!("{} local artifact", expected.id),
            )?;
        }
    }
    Ok(expected
        .into_iter()
        .map(|entry| entry.id.to_string())
        .collect())
}

/// Derived from the per-E lists (plan §12 V-reference verbs, v4.5 Q-49: "新增"/"补充断言" count,
/// "涉及" does not; E06 therefore no longer carries V081).
fn expected_v_to_e() -> BTreeMap<String, BTreeSet<String>> {
    let mut result: BTreeMap<String, BTreeSet<String>> = (1..=V_SCENARIO_COUNT)
        .map(|i| (format!("V{i:03}"), BTreeSet::new()))
        .collect();
    for (task, scenarios) in expected_e_scenarios() {
        for scenario in scenarios {
            result.get_mut(scenario).unwrap().insert(task.to_string());
        }
    }
    result
}

fn validate_v_scenarios(
    value: &Value,
    evidence_ids: &BTreeMap<String, String>,
) -> Result<BTreeSet<String>, String> {
    let array = value.as_array().ok_or("v_scenarios must be an array")?;
    let mut by_id = BTreeMap::new();
    for entry in array {
        let id = nonempty_string(&entry["id"], "V id")?;
        if by_id.insert(id, entry).is_some() {
            return Err(format!("duplicate V id {id}"));
        }
    }
    let expected = expected_v_to_e();
    let actual_ids: BTreeSet<String> = by_id.keys().map(|id| (*id).to_string()).collect();
    let expected_ids: BTreeSet<String> = expected.keys().cloned().collect();
    if actual_ids != expected_ids {
        return Err("V001-V123 exact set is incomplete".into());
    }
    for (id, expected_tasks) in &expected {
        let entry = by_id[id.as_str()];
        if entry["specification_status"] != "included" || entry["effect_status"] != "not_claimed" {
            return Err(format!("{id} conflates specification or effect status"));
        }
        for field in ["status", "verification_status"] {
            let status = nonempty_string(&entry[field], &format!("{id} {field}"))?;
            if !ALLOWED_STATUSES.contains(&status) {
                return Err(format!("{id} has unknown {field}"));
            }
        }
        let tasks = string_set(&entry["e_tasks"], &format!("{id} e_tasks"))?;
        if &tasks != expected_tasks {
            return Err(format!(
                "{id} E responsibility mapping differs from plan {PLAN_VERSION}"
            ));
        }
        validate_evidence_refs(
            &entry["related_evidence_refs"],
            &format!("{id} evidence refs"),
            &tasks,
            evidence_ids,
        )?;
        nonempty_string(&entry["remaining"], &format!("{id} remaining"))?;
    }
    Ok(actual_ids)
}

fn validate_trace_index_coverage(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("trace_index_coverage must be an object")?;
    if object.keys().cloned().collect::<BTreeSet<_>>()
        != ["E00".to_string(), "E16.6".to_string()]
            .into_iter()
            .collect()
    {
        return Err("trace index coverage must be recorded separately for E00 and E16.6".into());
    }
    // V080 (E00/E16.6) tracks V001–V123, HS01–HS08, HSF01–HSF16 and HSP01 (plan §18.11, Q-50).
    if object["E00"]["purpose"] != "index_consistency_only_not_scenario_execution"
        || string_set(&object["E00"]["ranges"], "E00 trace ranges")?
            != split_set("K01–K10 SO01–SO18 V087–V123 HS01–HS08 HSF01–HSF16 HSP01")
    {
        return Err(format!(
            "E00 trace-only responsibility differs from plan {PLAN_VERSION}"
        ));
    }
    if object["E16.6"]["purpose"] != "complete_derived_index_coverage_not_scenario_execution"
        || string_set(&object["E16.6"]["ranges"], "E16.6 trace ranges")?
            != split_set(
                "B01–B10 U01–U08 K01–K10 SO01–SO18 E00–E18 E16.1–E16.6 V001–V123 HS01–HS08 HSF01–HSF16 HSP01",
            )
    {
        return Err(format!(
            "E16.6 complete index responsibility differs from plan {PLAN_VERSION}"
        ));
    }
    Ok(())
}

fn validate_plan_lineage(value: &Value) -> Result<(), String> {
    let array = value.as_array().ok_or("plan_lineage must be an array")?;
    if array.len() != PLAN_LINEAGE.len() {
        return Err(format!(
            "plan_lineage must list exactly the {} registered plan versions",
            PLAN_LINEAGE.len()
        ));
    }
    let allowed: BTreeSet<&str> = ["version", "file", "sha256"].into_iter().collect();
    for (entry, (version, file, sha256)) in array.iter().zip(PLAN_LINEAGE) {
        let object = entry
            .as_object()
            .ok_or("plan_lineage entries must be objects")?;
        if object.keys().map(String::as_str).collect::<BTreeSet<_>>() != allowed {
            return Err(format!(
                "plan_lineage entry for {version} carries unknown or missing keys"
            ));
        }
        if entry["version"] != *version || entry["file"] != *file || entry["sha256"] != *sha256 {
            return Err(format!(
                "plan_lineage entry for {version} differs from the registered lineage"
            ));
        }
    }
    let last = array.last().ok_or("plan_lineage must not be empty")?;
    if last["version"] != PLAN_VERSION || last["sha256"] != PLAN_SHA {
        return Err(format!(
            "plan_lineage must end with the current plan {PLAN_VERSION}"
        ));
    }
    Ok(())
}

/// Pinned meaning of the execution-looking machine fields; the manifest's top-level
/// `field_semantics` must restate it verbatim (plan E16.6: a derived index may not be a second
/// source of truth).
fn expected_field_semantics() -> Vec<(&'static str, String)> {
    vec![
        (
            "command",
            "declared single-target checker command; syntax validated, not necessarily the literally executed command".to_string(),
        ),
        (
            "exit_code",
            "exit code of the controller gate run that covered the declared target".to_string(),
        ),
        (
            "log_path",
            "local evidence file containing that target's result (controller test log or acceptance receipt)".to_string(),
        ),
        (
            "execution",
            format!("optional record-level object, required when the pull request number is above {EXECUTION_REQUIRED_ABOVE_PR}, stating what was actually run: runner (ctrl_verify.sh, codex_task or other), command (the literally executed command), scope (workspace or single_target), exit_code (the integer exit code of that command) and evidence (the log_path or the identifier of the summary line inside the acceptance receipt); command, exit_code and log_path of the record keep describing the declared target; earlier records are neither backfilled nor guessed"),
        ),
        (
            "production_consumer",
            "optional record-level marker: none_library_only means the recorded module has no production caller in engine/http/mcp/apps as of the commit named in the record scope; wired means a production caller exists; an absent marker means this was not assessed".to_string(),
        ),
        // Appended by the v4.5 rebind (plan §18.10 machine form, Q-50); the five above are unchanged.
        (
            "execution_runners",
            "execution.runner additionally accepts the stage-8 gate scripts ctrl_verify_v43.sh and ctrl_verify_v45.sh; the runner names stated in the execution definition keep their meaning".to_string(),
        ),
        (
            "supersedes",
            "optional record-level array of withdrawn record ids that this re-verification replaces; every id must be listed in withdrawn_records with status superseded and belong to the same E task; a record carrying supersedes is bound to plan v4.5 or later; the replaced record is neither rewritten nor revived".to_string(),
        ),
        (
            "completes",
            "optional record-level array of still-valid record ids of the same E task whose evidence this new record completes (for example a re-run of a record with registered missing evidence); a record carrying completes is bound to plan v4.5 or later; the completed records stay unchanged and keep their own credit".to_string(),
        ),
        (
            "note",
            "optional record-level free-text note (for example a category or the task that will complete the record); it carries no verification credit".to_string(),
        ),
        (
            "withdrawn_records",
            "top-level list of records withdrawn from verified_subscopes because a revision changed the behavior they asserted: id, withdrawn_in_pr, clause, owner_task and status (pending_recheck, superseded or replaced_by_binding); every entry is pinned as a reviewed (id, withdrawn_in_pr, clause, owner_task, status) tuple in both checkers and a listed id never reappears in verified_subscopes".to_string(),
        ),
        (
            "evidence_missing_register",
            "top-level path and SHA-256 of the missing-evidence register, whose (record_id, field, path) entries are pinned in both checkers: an absent registered path is reported as missing, or as registered, not checked when it lies in the local evidence store and that store is not supplied; an absent path outside the register fails once its location is checked (outside out/ always, inside out/ with the local evidence store); a registered path that reappears fails unless its registered bytes and SHA-256 match (a path registered with unknown identity always fails on reappearance)".to_string(),
        ),
        // Appended by the AG-182 rework (review R-01/R-03/R-06/R-07).
        (
            "plan_recheck",
            "per-record recheck marker of the plan §18.10 machine form: a record bound to the current plan carries not_affected explicitly; a record bound to an earlier plan carries rechecked_v4.5 when its task is in the v4.5 recheck union, keeps rechecked_v4.3 when it was verified against v4.1 or v4.2 and its task is outside that union but was rechecked by v4.3 (every task except E17 and E18), and otherwise keeps not_affected (explicit or absent); rechecked_v4.5 is valid only inside the v4.5 recheck union, rechecked_v4.4 is never valid because v4.4 was never bound by the index on its own, and required never passes".to_string(),
        ),
        (
            "record_pins",
            "both checkers pin the reviewed supersedes and completes pairs (new record id to old record id) and, for every historical record retained from the previous index binding, the SHA-256 of its canonical JSON (keys sorted, no insignificant whitespace, UTF-8) without plan_recheck; a missing, extra or rewritten pin fails; the records added by an index pull request are pinned by the next index pull request, after the merge of the adding pull request is backfilled".to_string(),
        ),
        // Appended by AG-200 (fail-closed evidence judgment, Rust evidence-store input, register file
        // pin, merge facts); the thirteen definitions above are unchanged.
        (
            "evidence_judgment",
            "top-level key of the Python checker report, with evidence_unjudged: judged when every evidence path was judged and none failed (with --evidence-root, out/ paths are judged in the local evidence store it names, the directory holding out/, normally the primary checkout); not_performed_structure_only when --structure-only is given, which counts the absent out/ evidence paths, registered or not, in evidence_unjudged without judging them; not_performed when the check stopped before every path was judged, in particular when neither option is given and an out/ evidence path is unjudged, which fails the check (fail closed); --structure-only and --evidence-root are mutually exclusive; the Rust test judges out/ evidence paths only when the environment variable RSIA_SUPPORT_SCOPE_EVIDENCE_ROOT names that store and otherwise reports that the evidence judgment was not performed".to_string(),
        ),
        (
            "evidence_missing_register_pin",
            "both checkers pin the missing-evidence register file itself: evidence_missing_register.path must be reports/evidence-missing.json and evidence_missing_register.sha256 must equal both the reviewed register SHA-256 constant of each checker and the SHA-256 of the file, so no register entry (including an expected identity, an unknown marker or a missing window) changes without changing both checkers; the reviewed (record_id, field, path) triples stay pinned as well".to_string(),
        ),
        (
            "pr_state",
            "optional record-level pull request state: draft, open, closed or merged; merged requires a merged_sha and every other state requires merged_sha to be null; an absent pr_state asserts no state".to_string(),
        ),
        (
            "merged_sha",
            "record-level merge commit of the record's pull request, null only while that pull request is unmerged: it must equal the reviewed merge fact of the pull request in the Rust table expected_merge_for_pr (which the Python checker reads), differ from the record's own source_sha, agree across every record of the same pull request, and be neither the merge commit of another pull request nor the source_sha of a record of another pull request; a record added by an index pull request carries null until the next index pull request backfills that merge (with pr_state merged); it remains a manifest assertion whose git ancestry neither checker verifies".to_string(),
        ),
    ]
}

fn validate_field_semantics(value: &Value) -> Result<(), String> {
    let object = value.as_object().ok_or(
        "field_semantics must be an object stating what command, exit_code, log_path, execution and production_consumer mean",
    )?;
    let expected = expected_field_semantics();
    let actual_keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    let expected_keys: BTreeSet<&str> = expected.iter().map(|(key, _)| *key).collect();
    if actual_keys != expected_keys {
        return Err(format!(
            "field_semantics keys differ: missing={:?} extra={:?}",
            expected_keys.difference(&actual_keys).collect::<Vec<_>>(),
            actual_keys.difference(&expected_keys).collect::<Vec<_>>()
        ));
    }
    for (key, text) in &expected {
        if object[*key] != text.as_str() {
            return Err(format!(
                "field_semantics.{key} differs from the definition pinned by the checker"
            ));
        }
    }
    Ok(())
}

fn exact_keys(
    object: &serde_json::Map<String, Value>,
    required: &[&str],
    optional: &[&str],
    what: &str,
) -> Result<(), String> {
    let actual: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    let required: BTreeSet<&str> = required.iter().copied().collect();
    let allowed: BTreeSet<&str> = required
        .iter()
        .copied()
        .chain(optional.iter().copied())
        .collect();
    if !actual.is_superset(&required) || !actual.is_subset(&allowed) {
        return Err(format!(
            "{what} keys differ: missing={:?} extra={:?}",
            required.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&allowed).collect::<Vec<_>>()
        ));
    }
    Ok(())
}

fn pull_request_number(value: &Value, what: &str) -> Result<u64, String> {
    nonempty_string(value, what)?
        .strip_prefix("https://github.com/acosmi/RSIAgent/pull/")
        .filter(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|number| number.parse::<u64>().ok())
        .ok_or_else(|| format!("{what} must be a pull request URL"))
}

fn valid_owner_task(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("AG-") else {
        return false;
    };
    let (digits, suffix) = rest.split_at(rest.len().min(3));
    digits.len() == 3
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && (suffix.is_empty()
            || (suffix.len() == 1 && suffix.bytes().all(|b| b.is_ascii_lowercase())))
}

/// Every verified record by id, with its owning task (for the cross-record checks below).
fn collect_records(e_scopes: &Value) -> BTreeMap<String, (String, Value)> {
    let mut records = BTreeMap::new();
    for (task, entry) in e_scopes.as_object().into_iter().flatten() {
        for record in entry["verified_subscopes"].as_array().into_iter().flatten() {
            if let Some(id) = record["id"].as_str() {
                records.insert(id.to_string(), (task.clone(), record.clone()));
            }
        }
    }
    records
}

/// plan §18.10 (v4.5 Q-50): the reviewed pending-recheck list; withdrawn ids never reappear;
/// supersedes and completes point at the right records.
fn validate_withdrawn_records(
    value: &Value,
    records: &BTreeMap<String, (String, Value)>,
    pins: &ReviewedPins,
) -> Result<(), String> {
    let array = value
        .as_array()
        .ok_or("withdrawn_records must be an array")?;
    let mut withdrawn: BTreeMap<String, &Value> = BTreeMap::new();
    for entry in array {
        let object = entry
            .as_object()
            .ok_or("withdrawn_records entries must be objects")?;
        exact_keys(object, WITHDRAWN_KEYS, &[], "withdrawn_records entry")?;
        let id = nonempty_string(&entry["id"], "withdrawn id")?;
        if !valid_evidence_id(id) || evidence_task(id).is_none() {
            return Err(format!(
                "withdrawn id {id} is not namespaced under a registered E task"
            ));
        }
        let pr = pull_request_number(&entry["withdrawn_in_pr"], &format!("{id} withdrawn_in_pr"))?;
        expected_merge_for_pr(pr)?;
        if !nonempty_string(&entry["clause"], &format!("{id} clause"))?.starts_with('§') {
            return Err(format!(
                "{id} clause must cite the plan section of the changed behavior"
            ));
        }
        if !valid_owner_task(nonempty_string(
            &entry["owner_task"],
            &format!("{id} owner_task"),
        )?) {
            return Err(format!(
                "{id} owner_task must be a registered task number such as AG-187"
            ));
        }
        if !WITHDRAWN_STATUSES
            .contains(&nonempty_string(&entry["status"], &format!("{id} status"))?)
        {
            return Err(format!(
                "{id} withdrawn status must be one of {WITHDRAWN_STATUSES:?}"
            ));
        }
        if withdrawn.insert(id.to_string(), entry).is_some() {
            return Err(format!("withdrawn_records lists {id} twice"));
        }
    }
    let expected: BTreeSet<String> = pins.withdrawn.keys().cloned().collect();
    let actual: BTreeSet<String> = withdrawn.keys().cloned().collect();
    if actual != expected {
        return Err(format!(
            "withdrawn_records differ from the reviewed withdrawn list: missing={:?} extra={:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        ));
    }
    for (id, entry) in &withdrawn {
        let tuple = (
            pull_request_number(&entry["withdrawn_in_pr"], &format!("{id} withdrawn_in_pr"))?,
            entry["clause"].as_str().unwrap_or_default().to_string(),
            entry["owner_task"].as_str().unwrap_or_default().to_string(),
            entry["status"].as_str().unwrap_or_default().to_string(),
        );
        let reviewed = &pins.withdrawn[id];
        if tuple != *reviewed {
            return Err(format!(
                "{id} withdrawn entry (withdrawn_in_pr, clause, owner_task, status) {tuple:?} differs from the reviewed tuple {reviewed:?}; withdrawn entries are pinned in both checkers"
            ));
        }
    }
    if let Some(revived) = withdrawn.keys().find(|id| records.contains_key(*id)) {
        return Err(format!(
            "withdrawn record {revived} reappears in verified_subscopes; a withdrawn record is never revived"
        ));
    }
    let mut superseded = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    for (id, (task, record)) in records {
        if let Some(list) = record.get("supersedes") {
            for old in string_set(list, &format!("{id} supersedes"))? {
                let entry = withdrawn.get(&old).ok_or_else(|| {
                    format!("{id} supersedes {old}, which is not a withdrawn record")
                })?;
                if entry["status"] != "superseded" {
                    return Err(format!(
                        "{id} supersedes {old}, but its withdrawn status is not superseded"
                    ));
                }
                if evidence_task(&old).as_deref() != Some(task.as_str()) {
                    return Err(format!(
                        "{id} supersedes {old} of another E task; the re-verification belongs to the withdrawn record's own task"
                    ));
                }
                pairs.insert(("supersedes".to_string(), id.clone(), old.clone()));
                superseded.insert(old);
            }
        }
        if let Some(list) = record.get("completes") {
            for kept in string_set(list, &format!("{id} completes"))? {
                if withdrawn.contains_key(&kept) {
                    return Err(format!("{id} completes the withdrawn record {kept}"));
                }
                let Some((kept_task, _)) = records.get(&kept) else {
                    return Err(format!(
                        "{id} completes {kept}, which is not a current verified record"
                    ));
                };
                if kept_task != task {
                    return Err(format!(
                        "{id} completes {kept} of task {kept_task}; a re-run belongs to the completed record's own E task"
                    ));
                }
                pairs.insert(("completes".to_string(), id.clone(), kept));
            }
        }
    }
    for (id, entry) in &withdrawn {
        if entry["status"] == "superseded" && !superseded.contains(id) {
            return Err(format!(
                "{id} is marked superseded but no verified record supersedes it"
            ));
        }
    }
    if pairs != pins.relations {
        return Err(format!(
            "supersedes/completes pairs differ from the reviewed relation pairs: missing={:?} extra={:?}; a relation pair is reviewed in both checkers",
            pins.relations.difference(&pairs).collect::<Vec<_>>(),
            pairs.difference(&pins.relations).collect::<Vec<_>>()
        ));
    }
    Ok(())
}

/// SHA-256 of a record's canonical JSON without plan_recheck (evo_core::canonical_fingerprint:
/// keys sorted at every depth, no insignificant whitespace, UTF-8).
fn record_digest(record: &Value) -> Result<String, String> {
    let mut body = record.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("plan_recheck");
    }
    evo_core::canonical_fingerprint(&body)
        .map_err(|_| "a record cannot be fingerprinted".to_string())
}

/// plan §18.10 / V080.d: the historical records retained from the previous index binding are
/// never rewritten.
fn validate_record_pins(
    records: &BTreeMap<String, (String, Value)>,
    pins: &ReviewedPins,
) -> Result<(), String> {
    for (id, expected) in &pins.records {
        let (_, record) = records.get(id).ok_or_else(|| {
            format!(
                "{id} pinned historical record is missing from verified_subscopes; a historical record leaves the index only through a reviewed withdrawal"
            )
        })?;
        if record_digest(record)? != *expected {
            return Err(format!(
                "{id} historical record differs from its pinned digest (canonical JSON without plan_recheck); a historical record is never rewritten"
            ));
        }
    }
    Ok(())
}

fn iso_utc_or_unknown(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        text == "unknown"
            || (text.len() == 20
                && text.bytes().enumerate().all(|(index, byte)| match index {
                    4 | 7 => byte == b'-',
                    10 => byte == b'T',
                    13 | 16 => byte == b':',
                    19 => byte == b'Z',
                    _ => byte.is_ascii_digit(),
                }))
    })
}

/// Whether a repository-relative evidence path is a present regular file without symlink
/// components (Ok(false) when absent, Err for symlinks, non-files or escapes).
fn evidence_file_present(root: &Path, relative: &str, field: &str) -> Result<bool, String> {
    let relative_path = safe_relative(relative, field)?;
    let mut path = root.to_path_buf();
    for component in relative_path.components() {
        path.push(component);
        match std::fs::symlink_metadata(&path) {
            Err(_) => return Ok(false),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("{field} contains a symlink component: {relative}"));
            }
            Ok(_) => {}
        }
    }
    validate_repo_file(root, relative, field)?;
    Ok(true)
}

/// plan §18.10 (v4.5 Q-50): the missing-evidence register is pinned by path and SHA-256 (the file
/// SHA-256 also by a reviewed constant, AG-200), names real record fields, and decides the three
/// local outcomes. `evidence_root` names the local evidence store holding out/; with it every out/
/// evidence path is judged there, without it an out/ path is only judged when present in `root`.
fn validate_missing_register(
    value: &Value,
    root: &Path,
    records: &BTreeMap<String, (String, Value)>,
    pins: &ReviewedPins,
    evidence_root: Option<&Path>,
) -> Result<(), String> {
    let pointer = value["evidence_missing_register"]
        .as_object()
        .ok_or("evidence_missing_register must be an object")?;
    exact_keys(
        pointer,
        &["path", "sha256"],
        &[],
        "evidence_missing_register",
    )?;
    if pointer["path"] != MISSING_REGISTER_PATH {
        return Err(format!(
            "evidence_missing_register.path must be {MISSING_REGISTER_PATH}"
        ));
    }
    lower_hex(&pointer["sha256"], 64, "evidence_missing_register.sha256")?;
    if pointer["sha256"] != pins.register_sha256.as_str() {
        return Err(format!(
            "evidence_missing_register.sha256 {} is not the reviewed register SHA-256 pinned in both checkers ({}); a register entry cannot change without both checkers",
            pointer["sha256"].as_str().unwrap_or_default(),
            pins.register_sha256
        ));
    }
    validate_repo_file(root, MISSING_REGISTER_PATH, "missing-evidence register")?;
    let bytes = std::fs::read(root.join(MISSING_REGISTER_PATH))
        .map_err(|error| format!("cannot read the missing-evidence register: {error}"))?;
    if pointer["sha256"] != evo_core::hash(&bytes).as_str() {
        return Err(
            "missing-evidence register SHA-256 differs from evidence_missing_register.sha256"
                .into(),
        );
    }
    let register = load_json_strict(
        std::str::from_utf8(&bytes).map_err(|_| "missing-evidence register is not UTF-8")?,
    )?;
    exact_keys(
        register
            .as_object()
            .ok_or("missing-evidence register must be an object")?,
        &["basis", "entries", "schema_version"],
        &[],
        "missing-evidence register",
    )?;
    if register["schema_version"] != MISSING_SCHEMA_VERSION {
        return Err(format!(
            "missing-evidence register schema_version must be {MISSING_SCHEMA_VERSION}"
        ));
    }
    nonempty_string(&register["basis"], "missing-evidence register basis")?;
    let mut registered: BTreeMap<(String, String), (Option<u64>, Option<String>)> = BTreeMap::new();
    let mut triples = BTreeSet::new();
    for entry in register["entries"]
        .as_array()
        .ok_or("missing-evidence register entries must be an array")?
    {
        let object = entry
            .as_object()
            .ok_or("missing-evidence entries must be objects")?;
        exact_keys(
            object,
            &[
                "expected_bytes",
                "expected_sha256",
                "field",
                "group",
                "missing_window",
                "path",
                "record_id",
            ],
            &["unknown_reason"],
            "missing-evidence entry",
        )?;
        let path = nonempty_string(&entry["path"], "missing-evidence path")?;
        safe_relative(path, "missing-evidence path")?;
        let field = nonempty_string(&entry["field"], "missing-evidence field")?;
        if !EVIDENCE_PATH_FIELDS.contains(&field) {
            return Err(format!(
                "missing-evidence field {field} is not an evidence path field"
            ));
        }
        let record_id = nonempty_string(&entry["record_id"], "missing-evidence record_id")?;
        let (_, record) = records
            .get(record_id)
            .ok_or_else(|| format!("missing-evidence entry names unknown record {record_id}"))?;
        if record[field] != path {
            return Err(format!(
                "missing-evidence entry {record_id} {field} does not match the record's path"
            ));
        }
        let size = match &entry["expected_bytes"] {
            Value::String(text) if text == "unknown" => None,
            other => Some(other.as_u64().ok_or_else(|| {
                format!("{record_id} {field} expected_bytes must be an integer or unknown")
            })?),
        };
        let digest = match &entry["expected_sha256"] {
            Value::String(text) if text == "unknown" => None,
            other => {
                lower_hex(other, 64, &format!("{record_id} {field} expected_sha256"))?;
                other.as_str().map(str::to_string)
            }
        };
        if size.is_none() || digest.is_none() {
            nonempty_string(
                &entry["unknown_reason"],
                &format!("{record_id} {field} unknown_reason"),
            )?;
        } else if object.contains_key("unknown_reason") {
            return Err(format!(
                "{record_id} {field} unknown_reason is only allowed with an unknown identity"
            ));
        }
        if field == "input_ref"
            && let Some(digest) = &digest
            && record["input_digest"] != digest.as_str()
        {
            return Err(format!(
                "{record_id} input_ref expected_sha256 must equal its input_digest"
            ));
        }
        let window = &entry["missing_window"];
        let window_ok = window == "unknown"
            || window.as_object().is_some_and(|object| {
                object.len() == 2
                    && iso_utc_or_unknown(&object.get("after").cloned().unwrap_or(Value::Null))
                    && iso_utc_or_unknown(&object.get("before").cloned().unwrap_or(Value::Null))
            });
        if !window_ok {
            return Err(format!("{record_id} {field} missing_window is malformed"));
        }
        nonempty_string(&entry["group"], &format!("{record_id} {field} group"))?;
        if registered
            .insert((record_id.to_string(), field.to_string()), (size, digest))
            .is_some()
        {
            return Err(format!("{record_id} {field} is registered twice"));
        }
        triples.insert((record_id.to_string(), field.to_string(), path.to_string()));
    }
    if triples != pins.missing {
        return Err(format!(
            "missing-evidence register entries differ from the reviewed register: missing={:?} extra={:?}; the (record_id, field, path) triples are pinned in both checkers",
            pins.missing.difference(&triples).collect::<Vec<_>>(),
            triples.difference(&pins.missing).collect::<Vec<_>>()
        ));
    }
    for (id, (_, record)) in records {
        for field in EVIDENCE_PATH_FIELDS {
            let path = nonempty_string(&record[*field], &format!("{id} {field}"))?;
            let in_store = path.split('/').next() == Some(LOCAL_EVIDENCE_STORE);
            // out/ paths are judged in the local evidence store when one is supplied (AG-200).
            let base = match evidence_root {
                Some(store) if in_store => store,
                _ => root,
            };
            let identity = registered.get(&(id.clone(), field.to_string()));
            if evidence_file_present(base, path, &format!("{id} {field}"))? {
                let read = || {
                    std::fs::read(base.join(path))
                        .map_err(|error| format!("cannot read {id} {field}: {error}"))
                };
                // A present input manifest must match the record's input_digest (Python parity).
                if *field == "input_ref"
                    && record["input_digest"] != evo_core::hash(&read()?).as_str()
                {
                    return Err(format!(
                        "{id}: local input manifest SHA-256 mismatch: {path} differs from the record's input_digest"
                    ));
                }
                if let Some((size, digest)) = identity {
                    let Some(digest) = digest else {
                        return Err(format!(
                            "{id} {field} is registered missing with unknown identity and has reappeared; it must be verified through another binding and registered as a new record"
                        ));
                    };
                    let body = read()?;
                    if evo_core::hash(&body) != *digest
                        || size.is_some_and(|size| size != body.len() as u64)
                    {
                        return Err(format!(
                            "{id} {field} is registered missing and has reappeared with different bytes or SHA-256"
                        ));
                    }
                }
            } else if identity.is_none() && (!in_store || evidence_root.is_some()) {
                return Err(format!(
                    "{id} {field} {path} is missing and not listed in the missing-evidence register"
                ));
            }
        }
    }
    Ok(())
}

/// plan §16.8 HSF01–HSF16 and HSP01 (V123).
fn validate_hsf_sources(value: &Value) -> Result<(), String> {
    let array = value.as_array().ok_or("hsf_sources must be an array")?;
    let mut by_id = BTreeMap::new();
    for entry in array {
        let id = nonempty_string(&entry["id"], "HSF id")?;
        if by_id.insert(id, entry).is_some() {
            return Err(format!("duplicate HSF id {id}"));
        }
    }
    let mut expected: Vec<(&str, Value, Value, Value, Value, Value, Value)> = EXPECTED_HSF
        .iter()
        .map(|(id, path, blob)| {
            (
                *id,
                Value::from(HARNESS2_REPO),
                Value::from(HARNESS2_COMMIT),
                Value::from(HARNESS2_TREE),
                Value::from(*path),
                Value::from(*blob),
                Value::from("Apache-2.0"),
            )
        })
        .collect();
    expected.push((
        "HSP01",
        Value::Null,
        Value::Null,
        Value::Null,
        Value::from(HSP01_PATH),
        Value::Null,
        Value::Null,
    ));
    if by_id.len() != expected.len() || array.len() != expected.len() {
        return Err("hsf_sources must be exactly HSF01–HSF16 and HSP01".into());
    }
    for (id, repository, commit, tree, path, blob, license) in expected {
        let entry = by_id
            .get(id)
            .ok_or_else(|| format!("missing fixed source {id}"))?;
        exact_keys(
            entry.as_object().ok_or("HSF entries must be objects")?,
            &[
                "blob_sha",
                "commit",
                "e_tasks",
                "id",
                "license",
                "path",
                "read_range",
                "repository",
                "tree",
            ],
            &[],
            id,
        )?;
        if entry["repository"] != repository
            || entry["commit"] != commit
            || entry["tree"] != tree
            || entry["path"] != path
            || entry["blob_sha"] != blob
            || entry["license"] != license
            || entry["read_range"] != "ledger"
        {
            return Err(format!(
                "{id} fixed Harness² source pin differs from plan §16.8"
            ));
        }
        if string_set(&entry["e_tasks"], &format!("{id} e_tasks"))? != split_set("E00 E16.5 E16.6")
        {
            return Err(format!(
                "{id} E mapping differs from the V123 responsibility"
            ));
        }
    }
    Ok(())
}

/// plan §2.9 HS01–HS08 with the four tracking axes of plan §18.5 (v4.5 Q-50).
fn validate_hs_borrowings(
    value: &Value,
    evidence_ids: &BTreeMap<String, String>,
) -> Result<(), String> {
    let array = value.as_array().ok_or("hs_borrowings must be an array")?;
    let mut by_id = BTreeMap::new();
    for entry in array {
        let id = nonempty_string(&entry["id"], "HS id")?;
        if by_id.insert(id, entry).is_some() {
            return Err(format!("duplicate HS id {id}"));
        }
    }
    if by_id.len() != EXPECTED_HS.len() || array.len() != EXPECTED_HS.len() {
        return Err("hs_borrowings must be exactly HS01–HS08".into());
    }
    for (id, section, refs, tasks, scenarios, adoption) in EXPECTED_HS {
        let entry = by_id
            .get(id)
            .ok_or_else(|| format!("missing borrowing {id}"))?;
        exact_keys(
            entry.as_object().ok_or("HS entries must be objects")?,
            &[
                "adoption",
                "e_tasks",
                "effect_status",
                "hsf_refs",
                "id",
                "implementation_status",
                "related_evidence_refs",
                "remaining",
                "section",
                "specification_status",
                "v_scenarios",
                "verification_status",
            ],
            &[],
            id,
        )?;
        let task_set = string_set(&entry["e_tasks"], &format!("{id} e_tasks"))?;
        if entry["section"] != *section
            || string_set(&entry["hsf_refs"], &format!("{id} hsf_refs"))? != split_set(refs)
            || task_set != split_set(tasks)
            || string_set(&entry["v_scenarios"], &format!("{id} v_scenarios"))?
                != split_set(scenarios)
            || entry["adoption"] != *adoption
        {
            return Err(format!("{id} borrowing trace chain differs from plan §2.9"));
        }
        if entry["specification_status"] != "included" || entry["effect_status"] != "not_claimed" {
            return Err(format!("{id} conflates specification or effect status"));
        }
        let implementation = nonempty_string(
            &entry["implementation_status"],
            &format!("{id} implementation_status"),
        )?;
        let verification = nonempty_string(
            &entry["verification_status"],
            &format!("{id} verification_status"),
        )?;
        if !ALLOWED_STATUSES.contains(&implementation) || !ALLOWED_STATUSES.contains(&verification)
        {
            return Err(format!("{id} has an unknown tracking status"));
        }
        if *adoption == "registered_only"
            && (implementation, verification)
                != ("explicitly_out_of_scope", "explicitly_out_of_scope")
        {
            return Err(format!(
                "{id} is registered only (plan §2.9, v4.5 Q-44) and is not implemented in this version"
            ));
        }
        if (implementation == "planned" && verification != "planned") || verification == "verified"
        {
            return Err(format!("{id} tracking axes overstate the borrowing"));
        }
        validate_evidence_refs(
            &entry["related_evidence_refs"],
            &format!("{id} related_evidence_refs"),
            &task_set,
            evidence_ids,
        )?;
        nonempty_string(&entry["remaining"], &format!("{id} remaining"))?;
    }
    Ok(())
}

fn validate_manifest(value: &Value, root: &Path) -> Result<(), String> {
    validate_manifest_with(value, root, &ReviewedPins::reviewed())
}

fn validate_manifest_with(value: &Value, root: &Path, pins: &ReviewedPins) -> Result<(), String> {
    validate_manifest_with_evidence(value, root, pins, None)
}

/// The whole manifest check; `evidence_root` (the local evidence store holding out/) is supplied
/// only by the test of the real manifest, from RSIA_SUPPORT_SCOPE_EVIDENCE_ROOT (AG-200), so the
/// synthetic out/ paths of fixture records are never judged against a real store.
fn validate_manifest_with_evidence(
    value: &Value,
    root: &Path,
    pins: &ReviewedPins,
    evidence_root: Option<&Path>,
) -> Result<(), String> {
    if value["schema_version"] != "rsia.support_scope.v2"
        || value["plan_version"] != PLAN_VERSION
        || value["plan_sha256"] != PLAN_SHA
    {
        return Err(format!(
            "support index top level is not bound to the current plan {PLAN_VERSION}"
        ));
    }
    validate_project_license(value, root)?;
    validate_plan_lineage(&value["plan_lineage"])?;
    validate_field_semantics(&value["field_semantics"])?;
    if value["declaration"] != "subset_only"
        || value["not_full_route_complete"] != true
        || value["legacy_equivalence_unverified"] != true
        || value["dimensions"]["effect"] != "not_claimed"
    {
        return Err("support index overstates completion or effect".into());
    }
    let optional = string_set(&value["optional_disabled"], "optional_disabled")?;
    if optional != split_set("E17 E18") {
        return Err("E17 and E18 must remain explicitly disabled".into());
    }
    validate_trace_index_coverage(&value["trace_index_coverage"])?;
    let evidence_ids = validate_e_scopes(&value["e_scopes"], root)?;
    let e_ids = expected_e_ids();
    let so_ids = validate_so_sources(
        &value["traceability"]["so_sources"],
        root,
        &e_ids,
        &evidence_ids,
    )?;
    let v_ids = validate_v_scenarios(&value["traceability"]["v_scenarios"], &evidence_ids)?;
    validate_commitments(
        "B",
        &value["traceability"]["b_commitments"],
        root,
        &e_ids,
        &v_ids,
        &so_ids,
        &evidence_ids,
    )?;
    validate_commitments(
        "U",
        &value["traceability"]["u_commitments"],
        root,
        &e_ids,
        &v_ids,
        &so_ids,
        &evidence_ids,
    )?;
    validate_commitments(
        "K",
        &value["traceability"]["k_commitments"],
        root,
        &e_ids,
        &v_ids,
        &so_ids,
        &evidence_ids,
    )?;
    let records = collect_records(&value["e_scopes"]);
    validate_withdrawn_records(&value["withdrawn_records"], &records, pins)?;
    validate_missing_register(value, root, &records, pins, evidence_root)?;
    validate_hsf_sources(&value["traceability"]["hsf_sources"])?;
    validate_hs_borrowings(&value["traceability"]["hs_borrowings"], &evidence_ids)?;
    validate_record_pins(&records, pins)
}

/// The local evidence store for the test of the real manifest (plan §18.10, AG-200): the absolute
/// directory holding out/, read from RSIA_SUPPORT_SCOPE_EVIDENCE_ROOT; a set but unusable value
/// fails instead of silently skipping the judgment.
fn evidence_root_from_env() -> Option<PathBuf> {
    let store = PathBuf::from(std::env::var_os(EVIDENCE_ROOT_ENV)?);
    assert!(
        store.is_absolute() && store.is_dir(),
        "{EVIDENCE_ROOT_ENV} must name an existing absolute directory, got {}",
        store.display()
    );
    Some(store)
}

#[test]
fn support_scope_is_a_complete_but_non_normative_plan_derivative() {
    let root = repo_root();
    let value = load_manifest(&root);
    let evidence_root = evidence_root_from_env();
    match &evidence_root {
        Some(store) => eprintln!(
            "evidence judged against the local evidence store {}",
            store.display()
        ),
        None => eprintln!("evidence judgment not performed (set {EVIDENCE_ROOT_ENV})"),
    }
    validate_manifest_with_evidence(
        &value,
        &root,
        &ReviewedPins::reviewed(),
        evidence_root.as_deref(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(value["e_scopes"].as_object().unwrap().len(), 25);
    assert!(
        value["e_scopes"]
            .as_object()
            .unwrap()
            .values()
            .all(|entry| entry["status"] != "verified")
    );
}

#[test]
fn historical_records_keep_lineage_digests_but_top_level_must_be_current() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    let record = &baseline["e_scopes"]["E01"]["verified_subscopes"][0];
    assert_eq!(record["plan_version"], "v4.1");
    assert_eq!(record["plan_sha256"], PLAN_LINEAGE[0].2);
    validate_manifest(&baseline, &root).unwrap();

    // A record verified against the current plan states not_affected explicitly (plan §18.5).
    let (task, index) = record_position(&baseline, "E01.ag182_rerun_controller_acceptance");
    let rerun = &baseline["e_scopes"][&task]["verified_subscopes"][index];
    assert_eq!(rerun["plan_version"], PLAN_VERSION);
    assert_eq!(rerun["plan_recheck"], "not_affected");
    let mut implicit = baseline.clone();
    implicit["e_scopes"][&task]["verified_subscopes"][index]
        .as_object_mut()
        .unwrap()
        .remove("plan_recheck");
    let error = validate_manifest(&implicit, &root).expect_err("an implicit current marker");
    assert!(
        error.contains("must carry plan_recheck not_affected explicitly"),
        "{error}"
    );
    // Rebinding a historical record to the current plan rewrites it (V080.d digest pin).
    let mut current = load_manifest(&root);
    let record = &mut current["e_scopes"]["E01"]["verified_subscopes"][0];
    record["plan_version"] = Value::String(PLAN_VERSION.into());
    record["plan_sha256"] = Value::String(PLAN_SHA.into());
    record["plan_recheck"] = Value::String("not_affected".into());
    let error = validate_manifest(&current, &root).expect_err("a rebound historical record");
    assert!(
        error
            .contains("E01.controller_acceptance historical record differs from its pinned digest"),
        "{error}"
    );
    validate_manifest_with(&current, &root, &repinned(&current)).unwrap();

    let mut mismatched = load_manifest(&root);
    mismatched["e_scopes"]["E01"]["verified_subscopes"][0]["plan_version"] =
        Value::String(PLAN_VERSION.into());
    let error = validate_manifest(&mismatched, &root)
        .expect_err("a v4.5 version paired with the v4.1 digest was accepted");
    assert!(
        error.contains("not the registered digest of plan v4.5"),
        "{error}"
    );

    let mut outside = load_manifest(&root);
    outside["e_scopes"]["E01"]["verified_subscopes"][0]["plan_sha256"] =
        Value::String("a".repeat(64));
    assert!(
        validate_manifest(&outside, &root).is_err(),
        "a digest outside the lineage was accepted"
    );

    let mut unknown_version = load_manifest(&root);
    unknown_version["e_scopes"]["E01"]["verified_subscopes"][0]["plan_version"] =
        Value::String("v4.0".into());
    let error = validate_manifest(&unknown_version, &root)
        .expect_err("an unregistered plan version was accepted");
    assert!(
        error.contains("outside the registered plan lineage"),
        "{error}"
    );

    let mut stale_top = load_manifest(&root);
    stale_top["plan_version"] = Value::String("v4.1".into());
    stale_top["plan_sha256"] = Value::String(PLAN_LINEAGE[0].2.into());
    let error = validate_manifest(&stale_top, &root)
        .expect_err("a top level still bound to v4.1 was accepted");
    assert!(
        error.contains("not bound to the current plan v4.5"),
        "{error}"
    );
}

#[test]
fn rejects_pending_or_unknown_plan_recheck_markers() {
    let root = repo_root();
    let mut required = load_manifest(&root);
    required["e_scopes"]["E01"]["verified_subscopes"][0]["plan_recheck"] =
        Value::String("required".into());
    let error = validate_manifest(&required, &root)
        .expect_err("a record still requiring a recheck was accepted");
    assert!(
        error.contains("E01.controller_acceptance still requires a recheck"),
        "{error}"
    );

    for marker in ["improved", "rechecked_v4.4", "rechecked_v4.1"] {
        let mut unknown = load_manifest(&root);
        unknown["e_scopes"]["E01"]["verified_subscopes"][0]["plan_recheck"] =
            Value::String(marker.into());
        let error = validate_manifest(&unknown, &root).expect_err("an unregistered marker");
        assert!(
            error.contains("is not a registered marker"),
            "{marker}: {error}"
        );
    }

    for task in ["E17", "E18"] {
        let mut untouched = load_manifest(&root);
        untouched["e_scopes"][task]["verified_subscopes"][0]
            .as_object_mut()
            .unwrap()
            .remove("plan_recheck");
        validate_manifest(&untouched, &root).unwrap();
    }

    for task in PLAN_RECHECK_TASKS.iter().copied().filter(|task| {
        !load_manifest(&root)["e_scopes"][*task]["verified_subscopes"]
            .as_array()
            .unwrap()
            .is_empty()
    }) {
        let mut downgraded = load_manifest(&root);
        let record = &mut downgraded["e_scopes"][task]["verified_subscopes"][0];
        assert_eq!(record["plan_recheck"], PLAN_RECHECKED);
        record["plan_recheck"] = Value::String("not_affected".into());
        assert!(
            validate_manifest(&downgraded, &root).is_err(),
            "{task}: a v4.1 record of a touched task without a recheck was accepted"
        );
        for previous in ["rechecked_v4.2", "rechecked_v4.3", "rechecked_v4.4"] {
            let mut previous_recheck = load_manifest(&root);
            previous_recheck["e_scopes"][task]["verified_subscopes"][0]["plan_recheck"] =
                Value::String(previous.into());
            assert!(
                validate_manifest(&previous_recheck, &root).is_err(),
                "{task}: the previous-plan marker {previous} was accepted in place of a v4.5 recheck"
            );
        }
        let mut missing = load_manifest(&root);
        missing["e_scopes"][task]["verified_subscopes"][0]
            .as_object_mut()
            .unwrap()
            .remove("plan_recheck");
        assert!(
            validate_manifest(&missing, &root).is_err(),
            "{task}: a v4.1 record of a touched task with no marker was accepted"
        );
    }
}

#[test]
fn rejects_missing_or_unregistered_plan_lineage() {
    let root = repo_root();
    let mut missing = load_manifest(&root);
    missing.as_object_mut().unwrap().remove("plan_lineage");
    assert!(validate_manifest(&missing, &root).is_err());

    let mut reversed = load_manifest(&root);
    reversed["plan_lineage"].as_array_mut().unwrap().reverse();
    assert!(validate_manifest(&reversed, &root).is_err());

    let mut truncated = load_manifest(&root);
    truncated["plan_lineage"].as_array_mut().unwrap().remove(0);
    assert!(validate_manifest(&truncated, &root).is_err());

    let mut wrong_digest = load_manifest(&root);
    wrong_digest["plan_lineage"][1]["sha256"] = Value::String(PLAN_LINEAGE[0].2.into());
    assert!(validate_manifest(&wrong_digest, &root).is_err());

    let mut extra_key = load_manifest(&root);
    extra_key["plan_lineage"][1]["effect"] = Value::String("improved".into());
    assert!(validate_manifest(&extra_key, &root).is_err());

    let mut extended = load_manifest(&root);
    extended["plan_lineage"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "version": "v4.6",
            "file": "future.md",
            "sha256": "b".repeat(64),
        }));
    assert!(validate_manifest(&extended, &root).is_err());

    let mut without_v44 = load_manifest(&root);
    without_v44["plan_lineage"]
        .as_array_mut()
        .unwrap()
        .remove(3);
    assert!(
        validate_manifest(&without_v44, &root).is_err(),
        "a lineage skipping the never-bound v4.4 was accepted"
    );
}

#[test]
fn v45_recheck_tasks_are_the_literal_union_of_the_plan_landing_points() {
    // plan §19.6 (as corrected by v4.5) ∪ §19.7; "E16" is the parent key only (plan §18.10).
    let actual: BTreeSet<String> = PLAN_RECHECK_TASKS
        .iter()
        .map(|task| task.to_string())
        .collect();
    assert_eq!(
        actual,
        split_set("E00 E01 E02 E03 E04 E05 E06 E07 E08 E09 E10 E13 E16 E16.1 E16.3 E16.5 E16.6")
    );
    for outside in ["E11", "E12", "E14", "E15", "E16.2", "E16.4", "E17", "E18"] {
        assert!(!PLAN_RECHECK_TASKS.contains(&outside), "{outside}");
    }
    let v43: BTreeSet<String> = V43_RECHECK_TASKS
        .iter()
        .map(|task| task.to_string())
        .collect();
    let mut all_but_optional = expected_e_ids();
    all_but_optional.remove("E17");
    all_but_optional.remove("E18");
    assert_eq!(v43, all_but_optional);
    assert_eq!(RELATION_MIN_VERSION, "v4.5");
}

#[test]
fn v43_scenarios_are_registered_without_execution_credit_and_reject_tampering() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    let mapping = expected_v_to_e();
    assert_eq!(mapping.len(), V_SCENARIO_COUNT);
    assert_eq!(mapping["V107"], split_set("E02 E16 E16.4"));
    let mut unexecuted = baseline.clone();
    unexecuted["traceability"]["v_scenarios"][119]["status"] = serde_json::json!("verified");
    unexecuted["traceability"]["v_scenarios"][119]["verification_status"] =
        serde_json::json!("verified");
    unexecuted["traceability"]["v_scenarios"][119]["related_evidence_refs"] = serde_json::json!([]);
    unexecuted["traceability"]["v_scenarios"][119]["remaining"] = serde_json::json!("");
    assert!(validate_manifest(&unexecuted, &root).is_err());
    let mut missing = baseline.clone();
    missing["traceability"]["v_scenarios"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(validate_manifest(&missing, &root).is_err());
    let mut duplicate = baseline.clone();
    let last = duplicate["traceability"]["v_scenarios"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    duplicate["traceability"]["v_scenarios"]
        .as_array_mut()
        .unwrap()
        .push(last);
    assert!(validate_manifest(&duplicate, &root).is_err());
    let mut wrong_owner = baseline;
    wrong_owner["traceability"]["v_scenarios"][119]["e_tasks"] = serde_json::json!(["E17"]);
    assert!(validate_manifest(&wrong_owner, &root).is_err());
}

#[test]
fn e09_scenarios_include_the_v097_ablation_obligation() {
    assert!(expected_e_scenarios()["E09"].contains("V097"));
    assert!(expected_v_to_e()["V097"].contains("E09"));
    let root = repo_root();
    let mut without_scenario = load_manifest(&root);
    without_scenario["e_scopes"]["E09"]["scenarios"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item.as_str() != Some("V097"));
    assert!(validate_manifest(&without_scenario, &root).is_err());

    let mut without_task = load_manifest(&root);
    let v097 = without_task["traceability"]["v_scenarios"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["id"] == "V097")
        .unwrap();
    v097["e_tasks"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item.as_str() != Some("E09"));
    assert!(validate_manifest(&without_task, &root).is_err());
}

#[test]
fn rejects_missing_e16_parent_and_optional_extensions() {
    let root = repo_root();
    for missing in ["E16", "E17", "E18"] {
        let mut value = load_manifest(&root);
        value["e_scopes"].as_object_mut().unwrap().remove(missing);
        assert!(
            validate_manifest(&value, &root).is_err(),
            "missing {missing} was accepted"
        );
    }
}

#[test]
fn rejects_unknown_status_and_verified_without_execution_evidence() {
    let root = repo_root();
    let mut unknown = load_manifest(&root);
    unknown["e_scopes"]["E17"]["status"] = Value::String("verified_optional".into());
    assert!(validate_manifest(&unknown, &root).is_err());

    let mut unsupported_verified = load_manifest(&root);
    assert!(
        unsupported_verified["e_scopes"]["E15"]["verified_subscopes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    unsupported_verified["e_scopes"]["E15"]["status"] = Value::String("verified".into());
    unsupported_verified["e_scopes"]["E15"]["remaining"] = Value::Array(vec![]);
    assert!(validate_manifest(&unsupported_verified, &root).is_err());
}

#[test]
fn rejects_verified_record_missing_input_or_actual_result() {
    let root = repo_root();
    for field in ["input_digest", "input_ref", "actual_result"] {
        let mut value = load_manifest(&root);
        value["e_scopes"]["E01"]["verified_subscopes"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            validate_manifest(&value, &root).is_err(),
            "missing {field} was accepted"
        );
    }
}

#[test]
fn rejects_failed_execution_and_wrong_per_pr_merge_sha() {
    let root = repo_root();
    let mut failed = load_manifest(&root);
    failed["e_scopes"]["E01"]["verified_subscopes"][0]["exit_code"] = serde_json::json!(101);
    assert!(validate_manifest(&failed, &root).is_err());

    let mut wrong_merge = load_manifest(&root);
    wrong_merge["e_scopes"]["E03"]["verified_subscopes"][0]["merged_sha"] =
        Value::String("59afd1663a0d5f5212077dd92beaaf7920cde0ff".into());
    assert!(validate_manifest(&wrong_merge, &root).is_err());
}

#[test]
fn rejects_well_formed_but_wrong_so_pin_and_mapping() {
    let root = repo_root();
    for field in ["blob_sha", "commit"] {
        let mut value = load_manifest(&root);
        value["traceability"]["so_sources"][0][field] =
            Value::String("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
        assert!(
            validate_manifest(&value, &root).is_err(),
            "wrong SO {field} was accepted"
        );
    }
    let mut mapping = load_manifest(&root);
    mapping["traceability"]["so_sources"][0]["e_tasks"] = serde_json::json!(["E99"]);
    assert!(validate_manifest(&mapping, &root).is_err());
}

#[test]
fn rejects_broken_commitment_and_v_trace_links() {
    let root = repo_root();
    let mut b = load_manifest(&root);
    b["traceability"]["b_commitments"][0]["e_tasks"] = serde_json::json!(["E18"]);
    assert!(validate_manifest(&b, &root).is_err());

    let mut v = load_manifest(&root);
    v["traceability"]["v_scenarios"][0]["e_tasks"] = serde_json::json!(["E18"]);
    assert!(validate_manifest(&v, &root).is_err());

    let mut hidden_gap = load_manifest(&root);
    hidden_gap["traceability"]["k_commitments"][4]["implementation_status"] =
        Value::String("implemented_not_verified".into());
    assert!(
        validate_manifest(&hidden_gap, &root).is_err(),
        "K05 cannot hide its missing consolidation consumer as implementation-complete"
    );
}

#[test]
fn rejects_fake_target_extra_command_and_path_escape() {
    let root = repo_root();
    let mut fake = load_manifest(&root);
    fake["e_scopes"]["E01"]["verified_subscopes"][0]["test_entry"] =
        Value::String("crates/evo-core/tests/not_real.rs".into());
    fake["e_scopes"]["E01"]["verified_subscopes"][0]["command"] =
        Value::String("cargo test --locked --offline -p evo-core --test not_real".into());
    assert!(validate_manifest(&fake, &root).is_err());

    let mut extra = load_manifest(&root);
    extra["e_scopes"]["E01"]["verified_subscopes"][0]["command"] = Value::String(
        "cargo test --locked --offline -p evo-core --test evaluation_v41 && echo pass".into(),
    );
    assert!(validate_manifest(&extra, &root).is_err());

    let mut escape = load_manifest(&root);
    escape["e_scopes"]["E01"]["implementation_files"][0] = Value::String("../outside.rs".into());
    assert!(validate_manifest(&escape, &root).is_err());
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_evidence_file() {
    use std::os::unix::fs::symlink;
    let unique = format!("rsia-support-scope-{}", std::process::id());
    let temp = std::env::temp_dir().join(unique);
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(temp.join("inside")).unwrap();
    std::fs::write(temp.join("outside"), b"evidence").unwrap();
    symlink(temp.join("outside"), temp.join("inside/link")).unwrap();
    let error = validate_repo_file(&temp, "inside/link", "fixture")
        .expect_err("a symlinked evidence file must be rejected");
    assert!(error.contains("symlink"));

    std::fs::create_dir_all(temp.join("real-directory")).unwrap();
    std::fs::write(temp.join("real-directory/evidence"), b"evidence").unwrap();
    symlink(temp.join("real-directory"), temp.join("directory-link")).unwrap();
    let error = validate_repo_file(&temp, "directory-link/evidence", "fixture")
        .expect_err("a parent directory symlink must be rejected even inside the root");
    assert!(error.contains("symlink component"));
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn planned_scope_can_omit_files_only_with_explicit_remaining_reason() {
    let root = repo_root();
    let mut value = load_manifest(&root);
    assert_eq!(value["e_scopes"]["E15"]["status"], "planned");
    value["e_scopes"]["E15"]["implementation_files"] = Value::Array(vec![]);
    assert!(
        !value["e_scopes"]["E15"]["remaining"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    validate_manifest(&value, &root).unwrap();

    let mut started = load_manifest(&root);
    assert_eq!(started["e_scopes"]["E18"]["status"], "in_progress");
    started["e_scopes"]["E18"]["implementation_files"] = Value::Array(vec![]);
    assert!(
        validate_manifest(&started, &root).is_err(),
        "a started scope without implementation files was accepted"
    );
}

fn well_formed_execution() -> Value {
    serde_json::json!({
        "runner": "ctrl_verify.sh",
        "command": "cargom test --workspace --locked --no-fail-fast",
        "scope": "workspace",
        "exit_code": 0,
        "evidence": "out/fixture/ctrl-fixture.log.test",
    })
}

/// Validates a synthetic record of PR 142 (above the threshold) that carries the given `execution`.
fn execution_result(execution: Value) -> Result<(), String> {
    let record = serde_json::json!({ "execution": execution });
    validate_execution("X.record", &record, 142)
}

#[test]
fn field_semantics_must_be_present_and_equal_the_pinned_text() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    validate_manifest(&baseline, &root).unwrap();
    let pinned = expected_field_semantics();
    assert_eq!(pinned.len(), 17);
    assert_eq!(
        baseline["field_semantics"].as_object().unwrap().len(),
        pinned.len()
    );

    let mut missing = load_manifest(&root);
    missing.as_object_mut().unwrap().remove("field_semantics");
    let error = validate_manifest(&missing, &root)
        .expect_err("a manifest without field_semantics was accepted");
    assert!(
        error.contains("field_semantics must be an object"),
        "{error}"
    );

    for replacement in [
        Value::Null,
        serde_json::json!([]),
        serde_json::json!("command means executed"),
    ] {
        let mut value = load_manifest(&root);
        value["field_semantics"] = replacement;
        assert!(validate_manifest(&value, &root).is_err());
    }

    for (key, _) in &pinned {
        let mut dropped = load_manifest(&root);
        dropped["field_semantics"]
            .as_object_mut()
            .unwrap()
            .remove(*key);
        assert!(
            validate_manifest(&dropped, &root).is_err(),
            "field_semantics without {key} was accepted"
        );
        for replacement in [
            Value::Null,
            Value::String(String::new()),
            Value::String("the command that was executed and passed".into()),
        ] {
            let mut redefined = load_manifest(&root);
            redefined["field_semantics"][*key] = replacement;
            let error = validate_manifest(&redefined, &root)
                .expect_err("a redefined field meaning was accepted");
            assert!(
                error.contains(&format!("field_semantics.{key} differs")),
                "{error}"
            );
        }
    }

    let mut extended = load_manifest(&root);
    extended["field_semantics"]["effect"] = Value::String("improved".into());
    assert!(validate_manifest(&extended, &root).is_err());
}

#[test]
fn execution_is_required_above_pr_141_and_optional_up_to_it() {
    assert_eq!(EXECUTION_REQUIRED_ABOVE_PR, 141);
    let bare = serde_json::json!({});
    validate_execution("X.record", &bare, 141).expect("execution must stay optional up to PR 141");
    validate_execution("X.record", &bare, 40).expect("older records are not backfilled");
    for number in [142, 143, 999] {
        let error = validate_execution("X.record", &bare, number)
            .expect_err("a record above PR 141 without execution was accepted");
        assert!(
            error.contains(&format!("PR #{number} is above #141")),
            "{error}"
        );
    }

    for runner in EXECUTION_RUNNERS {
        for scope in EXECUTION_SCOPES {
            let mut execution = well_formed_execution();
            execution["runner"] = Value::String((*runner).into());
            execution["scope"] = Value::String((*scope).into());
            execution_result(execution).unwrap();
        }
    }

    // Optional is not unchecked: a malformed execution is rejected for older PRs as well.
    let mut malformed = well_formed_execution();
    malformed["exit_code"] = Value::String("0".into());
    let record = serde_json::json!({ "execution": malformed });
    assert!(validate_execution("X.record", &record, 141).is_err());
}

#[test]
fn execution_rejects_malformed_objects() {
    for not_an_object in [
        Value::Null,
        serde_json::json!([]),
        serde_json::json!("ctrl_verify.sh"),
    ] {
        let error =
            execution_result(not_an_object).expect_err("a non-object execution was accepted");
        assert!(error.contains("execution must be an object"), "{error}");
    }

    let mut unknown_key = well_formed_execution();
    unknown_key["verdict"] = Value::String("pass".into());
    let error = execution_result(unknown_key).expect_err("an unknown execution key was accepted");
    assert!(error.contains("keys differ"), "{error}");

    for key in EXECUTION_KEYS {
        let mut missing = well_formed_execution();
        missing.as_object_mut().unwrap().remove(*key);
        let error = execution_result(missing)
            .expect_err("an execution without a required key was accepted");
        assert!(error.contains("keys differ"), "{key}: {error}");
    }

    for (field, invalid) in [
        ("runner", Value::String("manual".into())),
        ("runner", Value::Null),
        ("runner", serde_json::json!(["ctrl_verify.sh"])),
        ("scope", Value::String("all".into())),
        ("scope", serde_json::json!({})),
    ] {
        let mut execution = well_formed_execution();
        execution[field] = invalid;
        let error =
            execution_result(execution).expect_err("an unregistered runner or scope was accepted");
        assert!(
            error.contains(&format!("{field} must be one of")),
            "{error}"
        );
    }

    for invalid in [
        serde_json::json!(""),
        serde_json::json!("   "),
        serde_json::json!(["cargom", "test"]),
        Value::Null,
    ] {
        let mut execution = well_formed_execution();
        execution["command"] = invalid;
        assert!(execution_result(execution).is_err());
    }

    for invalid in [serde_json::json!(""), Value::Null, serde_json::json!(7)] {
        let mut execution = well_formed_execution();
        execution["evidence"] = invalid;
        assert!(execution_result(execution).is_err());
    }
}

#[test]
fn execution_command_must_not_repeat_the_declared_single_target_form() {
    for copied in [
        "cargo test --locked --offline -p evo-core --test evaluation_v41",
        "cargo test --locked --offline",
        "  cargo   test --locked   --offline -p evo-core --lib hosts::tests  ",
        "cargo test\t--locked --offline -p evo-core --test evaluation_v41",
    ] {
        let mut execution = well_formed_execution();
        execution["command"] = Value::String(copied.into());
        let error = execution_result(execution)
            .expect_err("the declared target copied as the executed command was accepted");
        assert!(error.contains("literally executed command"), "{error}");
    }
    for actual in [
        "cargom test --workspace --locked --no-fail-fast",
        "cargo test --locked -p evo-core --test evaluation_v41",
        "cargo test --workspace --locked",
    ] {
        let mut execution = well_formed_execution();
        execution["command"] = Value::String(actual.into());
        execution_result(execution).unwrap();
    }
}

#[test]
fn execution_exit_code_must_be_an_integer_but_may_record_a_real_nonzero_value() {
    for invalid in [
        serde_json::json!("0"),
        serde_json::json!(0.0),
        serde_json::json!(1.5),
        serde_json::json!(true),
        serde_json::json!(false),
        Value::Null,
        serde_json::json!([]),
        serde_json::json!({}),
    ] {
        let mut execution = well_formed_execution();
        execution["exit_code"] = invalid;
        let error = execution_result(execution).expect_err("a non-integer exit code was accepted");
        assert!(
            error.contains("exit_code must be the integer exit code"),
            "{error}"
        );
    }
    // E00: real exit codes. A workspace run that also hits unrelated failing binaries exits
    // nonzero, and recording that honestly must stay possible.
    for code in [0_i64, 1, 101, 134, -1] {
        let mut execution = well_formed_execution();
        execution["exit_code"] = serde_json::json!(code);
        execution_result(execution).unwrap();
    }
}

#[test]
fn production_consumer_accepts_only_registered_values() {
    validate_production_consumer("X.record", &serde_json::json!({})).unwrap();
    for value in PRODUCTION_CONSUMER_VALUES {
        let record = serde_json::json!({ "production_consumer": value });
        validate_production_consumer("X.record", &record).unwrap();
    }
    for invalid in [
        serde_json::json!("yes"),
        serde_json::json!("library_only"),
        serde_json::json!(""),
        serde_json::json!("WIRED"),
        Value::Null,
        serde_json::json!(true),
        serde_json::json!(1),
        serde_json::json!(["wired"]),
        serde_json::json!({}),
    ] {
        let record = serde_json::json!({ "production_consumer": invalid });
        let error = validate_production_consumer("X.record", &record)
            .expect_err("an unregistered production_consumer was accepted");
        assert!(
            error.contains("production_consumer must be one of"),
            "{error}"
        );
    }
}

#[test]
fn new_record_fields_are_validated_through_the_manifest() {
    let root = repo_root();
    let mut malformed = load_manifest(&root);
    malformed["e_scopes"]["E01"]["verified_subscopes"][0]["execution"] =
        Value::String("ctrl_verify.sh".into());
    let error = validate_manifest(&malformed, &root)
        .expect_err("a malformed execution on an older record was accepted");
    assert!(error.contains("execution must be an object"), "{error}");

    let mut well_formed = load_manifest(&root);
    well_formed["e_scopes"]["E01"]["verified_subscopes"][0]["execution"] = well_formed_execution();
    // Structurally valid on an older record, but adding it rewrites a pinned historical record.
    let error = validate_manifest(&well_formed, &root).expect_err("a rewritten historical record");
    assert!(error.contains("differs from its pinned digest"), "{error}");
    validate_manifest_with(&well_formed, &root, &repinned(&well_formed)).unwrap();

    let mut unregistered = load_manifest(&root);
    unregistered["e_scopes"]["E01"]["verified_subscopes"][0]["production_consumer"] =
        Value::String("maybe".into());
    let error = validate_manifest(&unregistered, &root)
        .expect_err("an unregistered production_consumer was accepted");
    assert!(
        error.contains("production_consumer must be one of"),
        "{error}"
    );

    for value in PRODUCTION_CONSUMER_VALUES {
        let mut registered = load_manifest(&root);
        registered["e_scopes"]["E01"]["verified_subscopes"][0]["production_consumer"] =
            Value::String((*value).into());
        validate_manifest_with(&registered, &root, &repinned(&registered)).unwrap();
    }
}

#[test]
fn project_license_rejects_unreviewed_or_malformed_manifest_binding() {
    let root = repo_root();
    let value = load_manifest(&root);
    validate_project_license(&value, &root).unwrap();
    for replacement in [
        Value::Null,
        serde_json::json!({"status":"MIT","path":"LICENSE","sha256":"164d42969d38c372e7cd85fe0daf749b6e8409fbd28afd36d1d2b098c5e9debb"}),
        serde_json::json!({"status":"source_available_personal_noncommercial_free","path":"../LICENSE","sha256":"164d42969d38c372e7cd85fe0daf749b6e8409fbd28afd36d1d2b098c5e9debb"}),
        serde_json::json!({"status":"source_available_personal_noncommercial_free","path":"LICENSE","sha256":true}),
        serde_json::json!({"status":"source_available_personal_noncommercial_free","path":"LICENSE","sha256":"164d42969d38c372e7cd85fe0daf749b6e8409fbd28afd36d1d2b098c5e9debb","extra":false}),
    ] {
        let mut changed = value.clone();
        changed["project_license"] = replacement;
        assert!(validate_manifest(&changed, &root).is_err());
    }
}

#[test]
fn project_license_raw_json_rejects_duplicates_at_every_depth() {
    for raw in [
        r#"{"project_license":{},"project_license":{}}"#,
        r#"{"project_license":{"status":"MIT","status":"source_available_personal_noncommercial_free"}}"#,
        r#"[{"sha256":"a","sha256":"b"}]"#,
        r#"{"valid":true} {"trailing":true}"#,
    ] {
        assert!(load_json_strict(raw).is_err(), "accepted: {raw}");
    }
    assert_eq!(
        load_json_strict(r#"{"scalars":[true,null,-1,2,0.5,"text"]}"#).unwrap(),
        serde_json::json!({"scalars":[true,null,-1,2,0.5,"text"]})
    );
}

#[test]
fn support_evidence_rejects_own_merge_even_with_reviewed_pr_metadata() {
    let root = repo_root();
    let mut value = load_manifest(&root);
    validate_manifest(&value, &root).unwrap();
    let record = &mut value["e_scopes"]["E01"]["verified_subscopes"][0];
    record["source_sha"] = record["merged_sha"].clone();
    let error = validate_manifest(&value, &root).unwrap_err();
    assert!(
        error.contains("must differ from its own source_sha"),
        "{error}"
    );
}

#[test]
fn support_evidence_namespace_uses_the_longest_registered_task() {
    for (id, task) in [
        ("E01.real_record", "E01"),
        ("E16.real_record", "E16"),
        ("E16.5.real_record", "E16.5"),
    ] {
        validate_evidence_namespace(id, task).unwrap();
    }
    for (id, task) in [
        ("E16.5.borrowed", "E16"),
        ("E01.borrowed", "E02"),
        ("E160.borrowed", "E16"),
        ("E01.bad/name", "E01"),
        ("E01.é", "E01"),
    ] {
        assert!(
            validate_evidence_namespace(id, task).is_err(),
            "accepted {id} for {task}"
        );
    }
}

#[test]
fn support_trace_groups_reject_known_evidence_from_an_unmapped_task() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    validate_manifest(&baseline, &root).unwrap();
    for group in [
        "v_scenarios",
        "b_commitments",
        "u_commitments",
        "k_commitments",
        "so_sources",
    ] {
        let mut value = baseline.clone();
        let entry = value["traceability"][group]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| {
                !entry["e_tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|task| task == "E01")
            })
            .expect("a real mapping outside E01");
        let field = if group == "so_sources" {
            "verification_evidence_refs"
        } else {
            "related_evidence_refs"
        };
        if entry.get(field).is_none() {
            entry[field] = serde_json::json!([]);
        }
        entry[field]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!("E01.controller_acceptance"));
        let error = validate_manifest(&value, &root).unwrap_err();
        assert!(
            error.contains("owned by unrelated task E01"),
            "{group}: {error}"
        );
    }
}

#[test]
fn support_evidence_refs_require_unique_known_owners_and_allow_empty_so() {
    let root = repo_root();
    let mut value = load_manifest(&root);
    for entry in value["traceability"]["so_sources"].as_array_mut().unwrap() {
        entry["verification_evidence_refs"] = serde_json::json!([]);
    }
    validate_manifest(&value, &root).unwrap();
    let owners = validate_e_scopes(&value["e_scopes"], &root).unwrap();
    let tasks = split_set("E01");
    validate_evidence_refs(
        &serde_json::json!(["E01.controller_acceptance"]),
        "refs",
        &tasks,
        &owners,
    )
    .unwrap();
    for bad in [
        serde_json::json!(["E01.missing"]),
        serde_json::json!(["E01.controller_acceptance", "E01.controller_acceptance"]),
    ] {
        assert!(validate_evidence_refs(&bad, "refs", &tasks, &owners).is_err());
    }
}

#[test]
fn support_test_entry_ownership_is_independent_of_evidence_namespace() {
    let root = repo_root();
    let mut value = load_manifest(&root);
    validate_manifest(&value, &root).unwrap();
    let other = value["e_scopes"]["E03"]["verified_subscopes"][0].clone();
    let record = &mut value["e_scopes"]["E01"]["verified_subscopes"][0];
    record["test_entry"] = other["test_entry"].clone();
    record["command"] = other["command"].clone();
    let error = validate_manifest(&value, &root).unwrap_err();
    assert!(
        error.contains("not part of the task's implementation crate/files"),
        "{error}"
    );
    let entry = "crates/evo-core/tests/evaluation_v41.rs";
    assert!(test_entry_belongs_to_task(
        entry,
        Path::new("crates/evo-core"),
        &[serde_json::json!(entry)]
    ));
    assert!(test_entry_belongs_to_task(
        entry,
        Path::new("crates/evo-core"),
        &[serde_json::json!("crates/evo-core/src/holdout.rs")]
    ));
    assert!(!test_entry_belongs_to_task(
        entry,
        Path::new("crates/evo-core"),
        &[serde_json::json!("crates/evo-core-extra/src/holdout.rs")]
    ));
}

struct WorkspaceMapFixture {
    root: PathBuf,
}

impl WorkspaceMapFixture {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "rsia-support-workspace-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        Self { root }
    }

    fn write(&self, relative: &str, body: &str) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn member(&self, path: &str, name: &str) {
        self.write(
            &format!("{path}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\n"),
        );
        self.write(&format!("{path}/tests/target.rs"), "// fixture target\n");
    }
}

impl Drop for WorkspaceMapFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn workspace_mapping_uses_declared_member_not_package_directory_guess() {
    let fixture = WorkspaceMapFixture::new();
    fixture.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"components/core-implementation\"]\n",
    );
    fixture.member("components/core-implementation", "evo-core");
    fixture.write(
        "components/core-implementation/src/lib.rs",
        "pub mod evaluator;\n",
    );
    fixture.write(
        "components/core-implementation/src/evaluator.rs",
        "// module fixture\n",
    );
    // Inspected data must never supply the executable metadata helper.
    fixture.write(
        "scripts/check_support_scope.py",
        "raise AssertionError('untrusted fixture helper')\n",
    );
    let packages = workspace_packages(&fixture.root).unwrap();
    assert_eq!(
        packages["evo-core"],
        Path::new("components/core-implementation")
    );
    for (command, expected) in [
        (
            "cargo test --locked --offline -p evo-core --test target",
            "components/core-implementation/tests/target.rs",
        ),
        (
            "cargo test --locked --offline -p evo-core --lib evaluator::tests",
            "components/core-implementation/src/evaluator.rs",
        ),
    ] {
        let resolved = resolve_test_command(&fixture.root, command, &packages).unwrap();
        assert_eq!(resolved.target, Path::new(expected));
        assert!(test_entry_belongs_to_task(
            expected,
            &resolved.member,
            &[serde_json::json!(
                "components/core-implementation/src/lib.rs"
            )]
        ));
        assert!(!test_entry_belongs_to_task(
            expected,
            &resolved.member,
            &[serde_json::json!(
                "components/core-implementation-extra/src/lib.rs"
            )]
        ));
    }
}

#[test]
fn workspace_mapping_rejects_undeclared_package_with_real_old_directory() {
    let fixture = WorkspaceMapFixture::new();
    fixture.member("crates/evo-core", "evo-core");
    fixture.member("components/other", "other");
    fixture.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/evo-core\"]\n",
    );
    let command = "cargo test --locked --offline -p evo-core --test target";
    resolve_test_command(
        &fixture.root,
        command,
        &workspace_packages(&fixture.root).unwrap(),
    )
    .unwrap();
    fixture.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"components/other\"]\n",
    );
    let error = resolve_test_command(
        &fixture.root,
        command,
        &workspace_packages(&fixture.root).unwrap(),
    )
    .err()
    .expect("an undeclared real directory was borrowed");
    assert!(error.contains("not a declared workspace member"), "{error}");
}

#[test]
fn workspace_mapping_rejects_duplicate_ambiguous_or_escaping_metadata() {
    let fixture = WorkspaceMapFixture::new();
    fixture.member("components/one", "evo-core");
    fixture.member("components/two", "evo-core");
    for members in [
        "[\"components/one\", \"components/one\"]",
        "[\"components/one\", \"components/two\"]",
        "[\"../outside\"]",
        "[\"components/*\"]",
    ] {
        fixture.write("Cargo.toml", &format!("[workspace]\nmembers = {members}\n"));
        assert!(
            workspace_packages(&fixture.root).is_err(),
            "accepted {members}"
        );
    }
    for raw in [
        r#"{}"#,
        r#"[]"#,
        r#"{"evo-core":1}"#,
        r#"{"evo-core":"components/one","evo-core":"components/one"}"#,
        r#"{"evo-core":"components/one","other":"components/one"}"#,
        r#"{"evo-core":"../outside"}"#,
        r#"{"evo-core":"/outside"}"#,
        r#"{"evo-core":"components//one"}"#,
        r#"{"unknown":null}"#,
    ] {
        assert!(
            decode_workspace_packages(raw, &fixture.root).is_err(),
            "accepted {raw}"
        );
    }
}

fn record_position(value: &Value, id: &str) -> (String, usize) {
    for (task, entry) in value["e_scopes"].as_object().unwrap() {
        for (index, record) in entry["verified_subscopes"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            if record["id"] == id {
                return (task.clone(), index);
            }
        }
    }
    panic!("no record {id}");
}

fn withdrawn_entry_mut<'a>(value: &'a mut Value, id: &str) -> &'a mut Value {
    value["withdrawn_records"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["id"] == id)
        .unwrap()
}

/// Re-pins the reviewed V080.d tables (record digests, withdrawn tuples, relation pairs) to a
/// deliberately changed fixture so that a test isolates the rule it is about; the reviewed pins
/// themselves are exercised by `withdrawn_tuples_relation_pairs_and_historical_records_are_pinned`.
fn repinned(value: &Value) -> ReviewedPins {
    let records = collect_records(&value["e_scopes"]);
    let mut pins = ReviewedPins::reviewed();
    pins.records = pins
        .records
        .keys()
        .filter_map(|id| {
            records
                .get(id)
                .map(|(_, record)| (id.clone(), record_digest(record).unwrap()))
        })
        .collect();
    pins.withdrawn = value["withdrawn_records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["id"].as_str().unwrap().to_string(),
                (
                    pull_request_number(&entry["withdrawn_in_pr"], "fixture").unwrap(),
                    entry["clause"].as_str().unwrap().to_string(),
                    entry["owner_task"].as_str().unwrap().to_string(),
                    entry["status"].as_str().unwrap().to_string(),
                ),
            )
        })
        .collect();
    pins.relations = relation_pairs(&records);
    pins
}

fn relation_pairs(
    records: &BTreeMap<String, (String, Value)>,
) -> BTreeSet<(String, String, String)> {
    let mut pairs = BTreeSet::new();
    for (id, (_, record)) in records {
        for kind in ["supersedes", "completes"] {
            for old in record[kind].as_array().into_iter().flatten() {
                pairs.insert((
                    kind.to_string(),
                    id.clone(),
                    old.as_str().unwrap().to_string(),
                ));
            }
        }
    }
    pairs
}

fn set_marker(value: &mut Value, id: &str, marker: Option<&str>) {
    let (task, index) = record_position(value, id);
    let record = value["e_scopes"][&task]["verified_subscopes"][index]
        .as_object_mut()
        .unwrap();
    match marker {
        Some(marker) => {
            record.insert("plan_recheck".into(), Value::String(marker.into()));
        }
        None => {
            record.remove("plan_recheck");
        }
    }
}

#[test]
fn v45_recheck_markers_follow_the_machine_form_and_name_a_later_plan() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    validate_manifest(&baseline, &root).unwrap();
    assert_eq!(PLAN_RECHECKED, "rechecked_v4.5");
    assert_eq!(
        PLAN_RECHECK_VALUES.iter().copied().collect::<BTreeSet<_>>(),
        [
            "not_affected",
            "required",
            "rechecked_v4.2",
            "rechecked_v4.3",
            "rechecked_v4.5"
        ]
        .into_iter()
        .collect()
    );
    // Review R-01: the 11 v4.1/v4.2 records outside the union keep rechecked_v4.3, and deleting
    // that marker fails again (probe f1 is E12.ag025).
    let records = collect_records(&baseline["e_scopes"]);
    let kept: Vec<&String> = records
        .iter()
        .filter(|(_, (task, record))| {
            !PLAN_RECHECK_TASKS.contains(&task.as_str())
                && V43_RECHECK_FROM.contains(&record["plan_version"].as_str().unwrap())
                && V43_RECHECK_TASKS.contains(&task.as_str())
        })
        .map(|(id, _)| id)
        .collect();
    assert_eq!(kept.len(), 11);
    assert!(kept.contains(&&"E12.ag025_curriculum_cleanup".to_string()));
    for id in kept {
        assert_eq!(records[id].1["plan_recheck"], "rechecked_v4.3", "{id}");
        let mut deleted = baseline.clone();
        set_marker(&mut deleted, id, None);
        let error = validate_manifest(&deleted, &root).expect_err("a deleted v4.3 marker");
        assert!(
            error.contains(&format!("{id} belongs to"))
                && error
                    .contains("but was rechecked by v4.3, so it keeps plan_recheck rechecked_v4.3"),
            "{error}"
        );
    }
    // Probe f2 and siblings: E11.controller cannot claim v4.5, v4.4 or v4.2, nor drop to not_affected.
    for (marker, expected) in [
        (
            "rechecked_v4.5",
            "E11.controller_acceptance plan_recheck rechecked_v4.5 is valid only inside the v4.5 recheck union",
        ),
        (
            "rechecked_v4.4",
            "E11.controller_acceptance plan_recheck rechecked_v4.4 is not a registered marker",
        ),
        ("rechecked_v4.2", "so it keeps plan_recheck rechecked_v4.3"),
        ("not_affected", "so it keeps plan_recheck rechecked_v4.3"),
    ] {
        let mut value = baseline.clone();
        set_marker(&mut value, "E11.controller_acceptance", Some(marker));
        let error = validate_manifest(&value, &root).expect_err("a false E11 marker");
        assert!(error.contains(expected), "{marker}: {error}");
    }
    // E17/E18 were in no recheck set: an implicit not_affected is fine, a recheck claim is not.
    for task in ["E17", "E18"] {
        let id = baseline["e_scopes"][task]["verified_subscopes"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        for (marker, expected) in [
            (
                "rechecked_v4.2",
                "names a recheck no registered revision applied",
            ),
            (
                "rechecked_v4.3",
                "names a recheck no registered revision applied",
            ),
            (
                "rechecked_v4.5",
                "is valid only inside the v4.5 recheck union",
            ),
        ] {
            let mut value = baseline.clone();
            set_marker(&mut value, &id, Some(marker));
            let error = validate_manifest(&value, &root).expect_err("a false E17/E18 marker");
            assert!(error.contains(expected), "{id} {marker}: {error}");
        }
    }
    // A v4.3 record outside the union keeps not_affected; a non-later or v4.5 marker fails.
    let host = "E16.4.ag126_v43_host_surface_record";
    for (marker, expected) in [
        ("rechecked_v4.2", "does not name a plan later"),
        ("rechecked_v4.3", "does not name a plan later"),
        (
            "rechecked_v4.5",
            "is valid only inside the v4.5 recheck union",
        ),
    ] {
        let mut own = baseline.clone();
        set_marker(&mut own, host, Some(marker));
        let error = validate_manifest(&own, &root).expect_err("a false v4.3 marker");
        assert!(error.contains(expected), "{marker}: {error}");
    }
    let mut explicit = baseline.clone();
    set_marker(&mut explicit, host, Some("not_affected"));
    validate_manifest(&explicit, &root).unwrap();
    let (task, index) = record_position(&baseline, host);
    for invalid in [Value::Null, serde_json::json!(true), serde_json::json!([])] {
        let mut value = baseline.clone();
        value["e_scopes"][&task]["verified_subscopes"][index]["plan_recheck"] = invalid;
        let error = validate_manifest(&value, &root).expect_err("a non-string marker");
        assert!(
            error.contains("plan_recheck must be a non-empty string"),
            "{error}"
        );
    }
    // Probe e: a union record rolled back to the v4.3 marker is rejected.
    let mut rolled_back = baseline;
    set_marker(
        &mut rolled_back,
        "E09.ag039_legal_actions",
        Some("rechecked_v4.3"),
    );
    let error = validate_manifest(&rolled_back, &root).expect_err("rolled back marker");
    assert!(error.contains("was not rechecked"), "{error}");
}

#[test]
fn current_records_state_not_affected_and_relations_need_v45() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    let records = collect_records(&baseline["e_scopes"]);
    let current: Vec<&Value> = records
        .values()
        .map(|(_, record)| record)
        .filter(|record| record["plan_version"] == PLAN_VERSION)
        .collect();
    assert_eq!(current.len(), 33);
    assert!(
        current
            .iter()
            .all(|record| record["plan_recheck"] == "not_affected"
                && record["pr"] == "https://github.com/acosmi/RSIAgent/pull/200")
    );
    let backfills: Vec<&Value> = records
        .values()
        .map(|(_, record)| record)
        .filter(|record| {
            let pr = pull_request_number(&record["pr"], "pr").unwrap();
            [178, 180, 187, 188, 189, 191, 192, 194, 195, 197, 198, 199].contains(&pr)
        })
        .collect();
    assert_eq!(backfills.len(), 12);
    assert!(
        backfills
            .iter()
            .all(|record| record["plan_version"] == "v4.3"
                && record["plan_sha256"] == PLAN_LINEAGE[2].2
                && record["plan_recheck"] == PLAN_RECHECKED)
    );
    let rerun = "E01.ag182_rerun_controller_acceptance";
    for (marker, expected) in [
        (
            None,
            "must carry plan_recheck not_affected explicitly (plan §18.5)",
        ),
        (Some("required"), "still requires a recheck"),
        (Some("rechecked_v4.5"), "does not name a plan later"),
    ] {
        let mut value = baseline.clone();
        set_marker(&mut value, rerun, marker);
        let error = validate_manifest(&value, &root).expect_err("a wrong current marker");
        assert!(error.contains(expected), "{marker:?}: {error}");
    }
    // Probe r1: a backfilled v4.3 record of the union without its v4.5 marker.
    let mut r1 = baseline.clone();
    set_marker(&mut r1, "E05.ag122_ga18_bound_cycle_validation", None);
    let error = validate_manifest(&r1, &root).expect_err("an unmarked backfill");
    assert!(
        error.contains("E05.ag122_ga18_bound_cycle_validation belongs to E05, which the v4.5 revision touches, but was not rechecked"),
        "{error}"
    );
    // Review R-07: a record carrying supersedes/completes cannot be rebound to an earlier plan.
    for id in [
        rerun,
        "E10.ag182_replay_v3_controller_acceptance",
        "E16.6.ag182_plan_v45_index_binding",
    ] {
        let mut value = baseline.clone();
        let (task, index) = record_position(&value, id);
        let record = &mut value["e_scopes"][&task]["verified_subscopes"][index];
        record["plan_version"] = Value::String("v4.3".into());
        record["plan_sha256"] = Value::String(PLAN_LINEAGE[2].2.into());
        record["plan_recheck"] = Value::String(PLAN_RECHECKED.into());
        let error = validate_manifest(&value, &root).expect_err("a relation on an older plan");
        assert!(
            error.contains(&format!(
                "{id} carries supersedes or completes, which exist since plan v4.5"
            )),
            "{error}"
        );
    }
    // Review R-08: completes stays inside the record's own E task.
    let mut foreign = baseline;
    let (task, index) = record_position(&foreign, rerun);
    foreign["e_scopes"][&task]["verified_subscopes"][index]["completes"] =
        serde_json::json!(["E00.controller_acceptance"]);
    let error = validate_manifest(&foreign, &root).expect_err("a cross-task completes");
    assert!(
        error.contains(&format!("{rerun} completes E00.controller_acceptance of task E00; a re-run belongs to the completed record's own E task")),
        "{error}"
    );
}

#[test]
fn withdrawn_tuples_relation_pairs_and_historical_records_are_pinned() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    let pins = ReviewedPins::reviewed();
    assert_eq!(pins.withdrawn.len(), 24);
    assert_eq!(repinned(&baseline).withdrawn, pins.withdrawn);
    let records = collect_records(&baseline["e_scopes"]);
    assert_eq!(relation_pairs(&records), pins.relations);
    assert_eq!(
        pins.relations
            .iter()
            .filter(|(kind, _, _)| kind == "supersedes")
            .count(),
        7
    );
    assert_eq!(
        pins.relations
            .iter()
            .filter(|(kind, _, _)| kind == "completes")
            .count(),
        25
    );
    // AG-200 pins every current record: the 93 retained historical records and the 45 records added
    // by AG-182 (12 backfills and the 33 PR #200 records carrying the backfilled merge 02a135aa).
    assert_eq!(pins.records.len(), 138);
    assert_eq!(
        pins.records.keys().collect::<BTreeSet<_>>(),
        records.keys().collect::<BTreeSet<_>>()
    );
    for (id, digest) in &pins.records {
        assert_eq!(record_digest(&records[id].1).unwrap(), *digest, "{id}");
    }
    let added = records
        .values()
        .filter(|(_, record)| {
            let pr = pull_request_number(&record["pr"], "pr").unwrap();
            [
                178, 180, 187, 188, 189, 191, 192, 194, 195, 197, 198, 199, 200,
            ]
            .contains(&pr)
        })
        .count();
    assert_eq!(added, 45);
    assert_eq!(
        record_digest(&serde_json::json!({"b": 1, "a": "§", "plan_recheck": "x"})).unwrap(),
        evo_core::hash("{\"a\":\"§\",\"b\":1}".as_bytes())
    );
    // Probe g1 and siblings: a reviewed withdrawn tuple cannot be rewritten field by field.
    for (field, invalid) in [
        ("owner_task", "AG-999"),
        ("clause", "§9.9"),
        ("status", "replaced_by_binding"),
        (
            "withdrawn_in_pr",
            "https://github.com/acosmi/RSIAgent/pull/165",
        ),
    ] {
        let mut value = baseline.clone();
        withdrawn_entry_mut(&mut value, "E05.ag052_versioned_batch_verdicts")[field] =
            Value::String(invalid.into());
        let error = validate_manifest(&value, &root).expect_err("a rewritten withdrawn tuple");
        assert!(
            error.contains("E05.ag052_versioned_batch_verdicts withdrawn entry")
                && error.contains("differs from the reviewed tuple (200, \"§3.4\", \"AG-192\", \"pending_recheck\")"),
            "{field}: {error}"
        );
    }
    // Review h3: dropping a supersedes and reverting the entry to pending_recheck.
    let mut h3 = baseline.clone();
    let (task, index) = record_position(&h3, "E10.ag182_replay_v3_controller_acceptance");
    h3["e_scopes"][&task]["verified_subscopes"][index]
        .as_object_mut()
        .unwrap()
        .remove("supersedes");
    withdrawn_entry_mut(&mut h3, "E10.controller_acceptance")["status"] =
        Value::String("pending_recheck".into());
    let error = validate_manifest(&h3, &root).expect_err("a reverted supersession");
    assert!(
        error.contains("E10.controller_acceptance withdrawn entry"),
        "{error}"
    );
    // Probe h1: a re-run that drops its completes; an unreviewed pair is extra.
    let rerun = "E01.ag182_rerun_controller_acceptance";
    let mut h1 = baseline.clone();
    let (task, index) = record_position(&h1, rerun);
    h1["e_scopes"][&task]["verified_subscopes"][index]
        .as_object_mut()
        .unwrap()
        .remove("completes");
    let error = validate_manifest(&h1, &root).expect_err("a dropped completes");
    assert!(
        error.contains("supersedes/completes pairs differ from the reviewed relation pairs: missing=[(\"completes\", \"E01.ag182_rerun_controller_acceptance\", \"E01.controller_acceptance\")] extra=[]"),
        "{error}"
    );
    let mut extra = baseline.clone();
    extra["e_scopes"][&task]["verified_subscopes"][index]["completes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!("E01.ag057_explicit_sample_registration"));
    let error = validate_manifest(&extra, &root).expect_err("an unreviewed relation pair");
    assert!(
        error.contains("missing=[] extra=[(\"completes\", \"E01.ag182_rerun_controller_acceptance\", \"E01.ag057_explicit_sample_registration\")]"),
        "{error}"
    );
    // Probe k1: one character of a historical record's actual_result.
    let mut k1 = baseline.clone();
    let (task, index) = record_position(&k1, "E12.ag050_curriculum_priority");
    let text = k1["e_scopes"][&task]["verified_subscopes"][index]["actual_result"]
        .as_str()
        .unwrap()
        .to_string();
    let changed = format!(
        "{}{}",
        &text[..text.len() - 1],
        if text.ends_with('.') { ";" } else { "." }
    );
    k1["e_scopes"][&task]["verified_subscopes"][index]["actual_result"] = Value::String(changed);
    let error = validate_manifest(&k1, &root).expect_err("a rewritten historical record");
    assert!(
        error.contains(
            "E12.ag050_curriculum_priority historical record differs from its pinned digest"
        ),
        "{error}"
    );
    // A pinned record that silently leaves the index (one nothing else references).
    let mut removed = baseline;
    let (task, index) = record_position(&removed, "E02.ag065_host_surface_name_coverage");
    removed["e_scopes"][&task]["verified_subscopes"]
        .as_array_mut()
        .unwrap()
        .remove(index);
    let error = validate_manifest(&removed, &root).expect_err("a removed historical record");
    assert!(
        error.contains("E02.ag065_host_surface_name_coverage pinned historical record is missing"),
        "{error}"
    );
}

#[test]
fn withdrawn_records_are_the_reviewed_list_and_never_revive() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    let withdrawn = baseline["withdrawn_records"].as_array().unwrap();
    assert_eq!(withdrawn.len(), EXPECTED_WITHDRAWN.len());
    assert_eq!(withdrawn.len(), 24);

    // Probe a: revive E09.ag033 inside verified_subscopes.
    let mut revived = baseline.clone();
    let mut clone = revived["e_scopes"]["E09"]["verified_subscopes"][0].clone();
    clone["id"] = Value::String("E09.ag033_run_next_trusted_gate".into());
    clone["log_path"] = Value::String("out/fixture/revived.log".into());
    revived["e_scopes"]["E09"]["verified_subscopes"]
        .as_array_mut()
        .unwrap()
        .push(clone);
    let error = validate_manifest(&revived, &root).expect_err("a revived withdrawn id");
    assert!(error.contains("is never revived"), "{error}");

    let mut dropped = baseline.clone();
    dropped["withdrawn_records"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["id"] != "E05.ag080_local_scoped_acceptance");
    let error = validate_manifest(&dropped, &root).expect_err("a dropped withdrawn entry");
    assert!(error.contains("reviewed withdrawn list"), "{error}");

    let mut duplicated = baseline.clone();
    let first = duplicated["withdrawn_records"][0].clone();
    duplicated["withdrawn_records"]
        .as_array_mut()
        .unwrap()
        .push(first);
    assert!(validate_manifest(&duplicated, &root).is_err());

    for (field, invalid, expected) in [
        (
            "status",
            serde_json::json!("withdrawn"),
            "withdrawn status must be one of",
        ),
        ("status", Value::Null, "status must be a non-empty string"),
        (
            "owner_task",
            serde_json::json!("AG187"),
            "owner_task must be a registered task number",
        ),
        (
            "owner_task",
            serde_json::json!("AG-18"),
            "owner_task must be a registered task number",
        ),
        (
            "clause",
            serde_json::json!("3.4"),
            "clause must cite the plan section",
        ),
        (
            "withdrawn_in_pr",
            serde_json::json!("#165"),
            "withdrawn_in_pr must be a pull request URL",
        ),
        (
            "withdrawn_in_pr",
            serde_json::json!("https://github.com/acosmi/RSIAgent/pull/999"),
            "no reviewed merge status for PR 999",
        ),
    ] {
        let mut value = baseline.clone();
        withdrawn_entry_mut(&mut value, "E05.ag052_versioned_batch_verdicts")[field] = invalid;
        let error = validate_manifest(&value, &root).expect_err("an invalid withdrawn field");
        assert!(error.contains(expected), "{field}: {error}");
    }
    let mut extra_key = baseline.clone();
    withdrawn_entry_mut(&mut extra_key, "E05.ag052_versioned_batch_verdicts")["verdict"] =
        Value::String("improved".into());
    assert!(validate_manifest(&extra_key, &root).is_err());
    let mut missing_key = baseline;
    withdrawn_entry_mut(&mut missing_key, "E05.ag052_versioned_batch_verdicts")
        .as_object_mut()
        .unwrap()
        .remove("owner_task");
    assert!(validate_manifest(&missing_key, &root).is_err());
}

/// A synthetic re-verification of the withdrawn E05.ag101 record (same task, own identity, bound
/// to the current plan); callers re-pin it with `repinned`.
fn with_synthetic_supersession(baseline: &Value) -> (Value, usize) {
    let mut value = baseline.clone();
    withdrawn_entry_mut(&mut value, "E05.ag101_streaming_verdicts_v42")["status"] =
        Value::String("superseded".into());
    let (task, index) = record_position(&value, "E05.ag182_rerun_controller_acceptance");
    let mut record = value["e_scopes"][&task]["verified_subscopes"][index].clone();
    record["id"] = Value::String("E05.fixture_reverification".into());
    record["log_path"] = Value::String("out/fixture/reverification.log".into());
    record["source_sha"] = Value::String("e".repeat(40));
    record.as_object_mut().unwrap().remove("completes");
    record["supersedes"] = serde_json::json!(["E05.ag101_streaming_verdicts_v42"]);
    let records = value["e_scopes"]["E05"]["verified_subscopes"]
        .as_array_mut()
        .unwrap();
    records.push(record);
    let index = records.len() - 1;
    (value, index)
}

#[test]
fn supersedes_and_completes_point_at_the_right_records() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    let (healthy, index) = with_synthetic_supersession(&baseline);
    let error = validate_manifest(&healthy, &root).expect_err("an unreviewed supersession");
    assert!(
        error.contains("E05.ag101_streaming_verdicts_v42 withdrawn entry"),
        "{error}"
    );
    let pins = repinned(&healthy);
    validate_manifest_with(&healthy, &root, &pins).unwrap();

    // Probe b: supersedes pointing at a non-existent, non-withdrawn, pending or foreign id.
    for (target, expected) in [
        ("E99.does_not_exist", "which is not a withdrawn record"),
        (
            "E05.controller_acceptance",
            "which is not a withdrawn record",
        ),
        (
            "E05.ag052_versioned_batch_verdicts",
            "its withdrawn status is not superseded",
        ),
        (
            "E04.ag054_typed_predispatch",
            "its withdrawn status is not superseded",
        ),
    ] {
        let mut value = healthy.clone();
        value["e_scopes"]["E05"]["verified_subscopes"][index]["supersedes"] =
            serde_json::json!([target]);
        let error = validate_manifest_with(&value, &root, &pins).expect_err("a wrong supersedes");
        assert!(error.contains(expected), "{target}: {error}");
    }
    let mut foreign = healthy.clone();
    withdrawn_entry_mut(&mut foreign, "E04.ag054_typed_predispatch")["status"] =
        Value::String("superseded".into());
    let foreign_pins = repinned(&foreign);
    foreign["e_scopes"]["E05"]["verified_subscopes"][index]["supersedes"] =
        serde_json::json!(["E04.ag054_typed_predispatch"]);
    let error =
        validate_manifest_with(&foreign, &root, &foreign_pins).expect_err("a foreign supersedes");
    assert!(error.contains("of another E task"), "{error}");
    // A withdrawn entry marked superseded must be superseded by a verified record.
    let mut orphaned = healthy.clone();
    orphaned["e_scopes"]["E05"]["verified_subscopes"][index]["supersedes"] = serde_json::json!([]);
    let error =
        validate_manifest_with(&orphaned, &root, &pins).expect_err("an orphaned superseded entry");
    assert!(
        error.contains("no verified record supersedes it"),
        "{error}"
    );

    let rerun = "E01.ag182_rerun_controller_acceptance";
    let (task, index) = record_position(&baseline, rerun);
    for (field, list, expected) in [
        (
            "completes",
            serde_json::json!(["E09.ag033_run_next_trusted_gate"]),
            "completes the withdrawn record",
        ),
        (
            "completes",
            serde_json::json!(["E01.not_a_record"]),
            "which is not a current verified record",
        ),
        (
            "completes",
            serde_json::json!([rerun]),
            "names the record itself",
        ),
        (
            "supersedes",
            serde_json::json!([rerun]),
            "names the record itself",
        ),
        (
            "completes",
            serde_json::json!("E01.controller_acceptance"),
            "completes must be an array",
        ),
        (
            "completes",
            serde_json::json!(["E01.controller_acceptance", "E01.controller_acceptance"]),
            "contains duplicate",
        ),
        (
            "completes",
            serde_json::json!(["bad id"]),
            "lists an invalid record id",
        ),
        (
            "supersedes",
            serde_json::json!(["E01.controller_acceptance"]),
            "cannot both supersede and complete",
        ),
    ] {
        let mut value = baseline.clone();
        value["e_scopes"][&task]["verified_subscopes"][index][field] = list.clone();
        let error = validate_manifest(&value, &root).expect_err("a wrong relation field");
        assert!(error.contains(expected), "{field} {list}: {error}");
    }
    // A fresh current record completing another record of its task is valid once reviewed.
    let mut fresh = baseline.clone();
    let mut record = fresh["e_scopes"][&task]["verified_subscopes"][index].clone();
    record["id"] = Value::String("E01.fixture_completion".into());
    record["log_path"] = Value::String("out/fixture/completion.log".into());
    record["source_sha"] = Value::String("f".repeat(40));
    record["completes"] = serde_json::json!(["E01.ag057_explicit_sample_registration"]);
    record["note"] = Value::String("completion fixture".into());
    fresh["e_scopes"][&task]["verified_subscopes"]
        .as_array_mut()
        .unwrap()
        .push(record);
    validate_manifest_with(&fresh, &root, &repinned(&fresh)).unwrap();
    for note in [Value::Null, serde_json::json!(""), serde_json::json!(["x"])] {
        let mut value = baseline.clone();
        value["e_scopes"][&task]["verified_subscopes"][index]["note"] = note;
        let error = validate_manifest(&value, &root).expect_err("a malformed note");
        assert!(error.contains("note must be a non-empty string"), "{error}");
    }
}

struct RegisterFixture {
    root: PathBuf,
}

impl RegisterFixture {
    fn new(register: &[u8]) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "rsia-support-register-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("reports")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join(MISSING_REGISTER_PATH), register).unwrap();
        std::fs::write(
            root.join("docs/implementation-ledger.md"),
            b"ledger fixture\n",
        )
        .unwrap();
        Self { root }
    }

    fn write(&self, relative: &str, body: &[u8]) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
}

impl Drop for RegisterFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

/// A labelled edit of the missing-evidence register used by the structural rejection cases.
type RegisterMutation = (&'static str, Box<dyn Fn(&mut Value)>);

fn register_bytes(root: &Path) -> Vec<u8> {
    std::fs::read(root.join(MISSING_REGISTER_PATH)).unwrap()
}

fn pinned(value: &Value, register: &[u8]) -> Value {
    let mut value = value.clone();
    value["evidence_missing_register"]["sha256"] = Value::String(evo_core::hash(register));
    value
}

/// Reviewed pins whose register SHA-256 is re-pinned to `register`, as a reviewed register update of
/// both checkers would do, so that a test isolates the rule it is about (AG-200).
fn register_pins(register: &[u8]) -> ReviewedPins {
    let mut pins = ReviewedPins::reviewed();
    pins.register_sha256 = evo_core::hash(register);
    pins
}

/// A synthetic local evidence store (a temporary directory holding out/) for the evidence-root
/// judgment (AG-200); the real out/ is neither read nor written by these tests.
struct EvidenceStoreFixture {
    root: PathBuf,
}

impl EvidenceStoreFixture {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "rsia-support-evidence-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn write(&self, relative: &str, body: &[u8]) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn remove(&self, relative: &str) {
        std::fs::remove_file(self.root.join(relative)).unwrap();
    }
}

impl Drop for EvidenceStoreFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn fixture_evidence(path: &str) -> Vec<u8> {
    format!("fixture evidence {path}\n").into_bytes()
}

/// Verified records by id with their owning task, as `collect_records` returns them.
type RecordMap = BTreeMap<String, (String, Value)>;
/// (record id, evidence field, path) of an evidence path written into a synthetic store.
type StorePath = (String, String, String);

/// Fills `store` with every out/ evidence path that is not registered missing; each unregistered
/// input_ref gets fixture bytes and its records the matching input_digest (registered inputs keep
/// their pinned digest). Returns the records to judge and the unregistered out/ triples.
fn complete_store(
    records: &RecordMap,
    register: &Value,
    store: &EvidenceStoreFixture,
) -> (RecordMap, Vec<StorePath>) {
    let registered: BTreeSet<(String, String)> = register["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["record_id"].as_str().unwrap().to_string(),
                entry["field"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let mut judged = records.clone();
    let mut written = Vec::new();
    for (id, (_, record)) in judged.iter_mut() {
        for field in EVIDENCE_PATH_FIELDS {
            let path = record[*field].as_str().unwrap().to_string();
            if path.split('/').next() != Some(LOCAL_EVIDENCE_STORE)
                || registered.contains(&(id.clone(), field.to_string()))
            {
                continue;
            }
            let body = fixture_evidence(&path);
            store.write(&path, &body);
            if *field == "input_ref" {
                record["input_digest"] = Value::String(evo_core::hash(&body));
            }
            written.push((id.clone(), field.to_string(), path));
        }
    }
    (judged, written)
}

#[test]
fn missing_evidence_register_is_pinned_and_decides_local_outcomes() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    let records = collect_records(&baseline["e_scopes"]);
    let original = register_bytes(&root);
    let register: Value = load_json_strict(std::str::from_utf8(&original).unwrap()).unwrap();
    let entries = register["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 42);
    let reviewed = ReviewedPins::reviewed();
    assert_eq!(evo_core::hash(&original), EXPECTED_MISSING_REGISTER_SHA256);
    assert_eq!(
        baseline["evidence_missing_register"]["sha256"],
        EXPECTED_MISSING_REGISTER_SHA256
    );
    let healthy = RegisterFixture::new(&original);
    validate_missing_register(&baseline, &healthy.root, &records, &reviewed, None).unwrap();

    // Probe c: dropping a registered path changes the pinned register.
    let mut dropped: Value = register.clone();
    dropped["entries"].as_array_mut().unwrap().remove(0);
    let dropped_bytes = serde_json::to_vec_pretty(&dropped).unwrap();
    let fixture = RegisterFixture::new(&dropped_bytes);
    let error = validate_missing_register(&baseline, &fixture.root, &records, &reviewed, None)
        .expect_err("an unpinned register edit");
    assert!(error.contains("SHA-256 differs"), "{error}");
    // Probe c2: the same drop with a refreshed pointer fails on the reviewed register SHA-256
    // (AG-200), and re-pinned in both checkers still on the reviewed triples.
    let error = validate_missing_register(
        &pinned(&baseline, &dropped_bytes),
        &fixture.root,
        &records,
        &reviewed,
        None,
    )
    .expect_err("a refreshed pointer");
    assert!(
        error.contains("is not the reviewed register SHA-256 pinned in both checkers"),
        "{error}"
    );
    let error = validate_missing_register(
        &pinned(&baseline, &dropped_bytes),
        &fixture.root,
        &records,
        &register_pins(&dropped_bytes),
        None,
    )
    .expect_err("a dropped triple");
    assert!(
        error.contains("missing-evidence register entries differ from the reviewed register")
            && error.contains("extra=[]"),
        "{error}"
    );
    // Probe c3: drop one entry and register another real path; the count stays 42.
    let mut swapped: Value = register.clone();
    swapped["entries"].as_array_mut().unwrap().remove(0);
    let (task, index) = record_position(&baseline, "E01.ag182_rerun_controller_acceptance");
    let other = &baseline["e_scopes"][&task]["verified_subscopes"][index];
    swapped["entries"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": other["log_path"],
            "record_id": other["id"],
            "field": "log_path",
            "expected_bytes": "unknown",
            "expected_sha256": "unknown",
            "unknown_reason": "probe c3",
            "missing_window": "unknown",
            "group": "probe_c3",
        }));
    assert_eq!(swapped["entries"].as_array().unwrap().len(), 42);
    let swapped_bytes = serde_json::to_vec_pretty(&swapped).unwrap();
    let fixture = RegisterFixture::new(&swapped_bytes);
    let error = validate_missing_register(
        &pinned(&baseline, &swapped_bytes),
        &fixture.root,
        &records,
        &reviewed,
        None,
    )
    .expect_err("a refreshed pointer");
    assert!(
        error.contains("is not the reviewed register SHA-256 pinned in both checkers"),
        "{error}"
    );
    let error = validate_missing_register(
        &pinned(&baseline, &swapped_bytes),
        &fixture.root,
        &records,
        &register_pins(&swapped_bytes),
        None,
    )
    .expect_err("a swapped triple");
    assert!(
        error.contains("missing-evidence register entries differ from the reviewed register")
            && error.contains("E01.ag182_rerun_controller_acceptance"),
        "{error}"
    );

    // A registered path reappearing: unknown identity always fails; a known identity must match.
    let unknown = entries
        .iter()
        .find(|entry| entry["expected_sha256"] == "unknown")
        .unwrap();
    let fixture = RegisterFixture::new(&original);
    fixture.write(
        unknown["path"].as_str().unwrap(),
        b"restored from somewhere\n",
    );
    let error = validate_missing_register(&baseline, &fixture.root, &records, &reviewed, None)
        .expect_err("an unknown-identity path reappeared");
    assert!(error.contains("unknown identity"), "{error}");
    let known = entries
        .iter()
        .find(|entry| entry["expected_sha256"] != "unknown" && entry["field"] == "log_path")
        .unwrap();
    let fixture = RegisterFixture::new(&original);
    fixture.write(known["path"].as_str().unwrap(), b"different bytes\n");
    let error = validate_missing_register(&baseline, &fixture.root, &records, &reviewed, None)
        .expect_err("a mismatched path reappeared");
    assert!(error.contains("different bytes or SHA-256"), "{error}");
    let body = b"exactly the registered bytes\n";
    let mut matching: Value = register.clone();
    for entry in matching["entries"].as_array_mut().unwrap() {
        if entry["path"] == known["path"] {
            entry["expected_sha256"] = Value::String(evo_core::hash(body));
            entry["expected_bytes"] = serde_json::json!(body.len());
        }
    }
    let matching_bytes = serde_json::to_vec_pretty(&matching).unwrap();
    let fixture = RegisterFixture::new(&matching_bytes);
    fixture.write(known["path"].as_str().unwrap(), body);
    // Rewriting the registered identity and refreshing the pointer fails on the reviewed register
    // SHA-256 (AG-200, review S-1); only a reviewed register update re-pinned in both checkers lets
    // the restored path pass.
    let error = validate_missing_register(
        &pinned(&baseline, &matching_bytes),
        &fixture.root,
        &records,
        &reviewed,
        None,
    )
    .expect_err("a rewritten identity with a refreshed pointer");
    assert!(
        error.contains("is not the reviewed register SHA-256 pinned in both checkers"),
        "{error}"
    );
    validate_missing_register(
        &pinned(&baseline, &matching_bytes),
        &fixture.root,
        &records,
        &register_pins(&matching_bytes),
        None,
    )
    .unwrap();
    // Probe c4: an unknown identity replaced by a forged SHA-256 and size, the forged file restored,
    // the pointer refreshed; re-pinned as a reviewed register it would pass, so the constant decides.
    let forged_body = b"forged evidence\n";
    let mut forged: Value = register.clone();
    let mut forged_path = String::new();
    for entry in forged["entries"].as_array_mut().unwrap() {
        if entry["expected_sha256"] == "unknown" && entry["expected_bytes"] == "unknown" {
            entry["expected_sha256"] = Value::String(evo_core::hash(forged_body));
            entry["expected_bytes"] = serde_json::json!(forged_body.len());
            entry.as_object_mut().unwrap().remove("unknown_reason");
            forged_path = entry["path"].as_str().unwrap().to_string();
            break;
        }
    }
    let forged_bytes = serde_json::to_vec_pretty(&forged).unwrap();
    let fixture = RegisterFixture::new(&forged_bytes);
    fixture.write(&forged_path, forged_body);
    let error = validate_missing_register(
        &pinned(&baseline, &forged_bytes),
        &fixture.root,
        &records,
        &reviewed,
        None,
    )
    .expect_err("a forged identity (probe c4)");
    assert!(
        error.contains("is not the reviewed register SHA-256 pinned in both checkers"),
        "{error}"
    );
    validate_missing_register(
        &pinned(&baseline, &forged_bytes),
        &fixture.root,
        &records,
        &register_pins(&forged_bytes),
        None,
    )
    .unwrap();
    // Probe c5: a known identity downgraded to unknown (with a reason), the pointer refreshed.
    let mut downgraded: Value = register.clone();
    for entry in downgraded["entries"].as_array_mut().unwrap() {
        if entry["expected_sha256"] != "unknown" && entry["expected_bytes"] != "unknown" {
            entry["expected_sha256"] = serde_json::json!("unknown");
            entry["expected_bytes"] = serde_json::json!("unknown");
            entry["unknown_reason"] = serde_json::json!("probe c5");
            break;
        }
    }
    let downgraded_bytes = serde_json::to_vec_pretty(&downgraded).unwrap();
    let fixture = RegisterFixture::new(&downgraded_bytes);
    let error = validate_missing_register(
        &pinned(&baseline, &downgraded_bytes),
        &fixture.root,
        &records,
        &reviewed,
        None,
    )
    .expect_err("a downgraded identity (probe c5)");
    assert!(
        error.contains("is not the reviewed register SHA-256 pinned in both checkers"),
        "{error}"
    );
    validate_missing_register(
        &pinned(&baseline, &downgraded_bytes),
        &fixture.root,
        &records,
        &register_pins(&downgraded_bytes),
        None,
    )
    .unwrap();

    // A tracked evidence path that is missing and unregistered fails in any checkout.
    let mut untracked = baseline.clone();
    let (task, index) = record_position(&untracked, "E01.ag057_explicit_sample_registration");
    untracked["e_scopes"][&task]["verified_subscopes"][index]["record_source"] =
        Value::String("docs/never_written.md".into());
    let error = validate_missing_register(
        &untracked,
        &healthy.root,
        &collect_records(&untracked["e_scopes"]),
        &reviewed,
        None,
    )
    .expect_err("an unregistered missing tracked path");
    assert!(
        error.contains("not listed in the missing-evidence register"),
        "{error}"
    );

    // Structural rejections, each re-pinned so only the named defect remains.
    let mutations: Vec<RegisterMutation> = vec![
        (
            "names unknown record",
            Box::new(|r: &mut Value| {
                r["entries"][0]["record_id"] = serde_json::json!("E01.nothing")
            }),
        ),
        (
            "does not match the record's path",
            Box::new(|r: &mut Value| {
                r["entries"][0]["path"] = serde_json::json!("out/fixture/other.log")
            }),
        ),
        (
            "is not an evidence path field",
            Box::new(|r: &mut Value| r["entries"][0]["field"] = serde_json::json!("scope")),
        ),
        (
            "is registered twice",
            Box::new(|r: &mut Value| {
                let first = r["entries"][0].clone();
                r["entries"].as_array_mut().unwrap().push(first);
            }),
        ),
        (
            "unknown_reason must be a non-empty string",
            Box::new(|r: &mut Value| {
                for entry in r["entries"].as_array_mut().unwrap() {
                    if entry["expected_sha256"] == "unknown" {
                        entry.as_object_mut().unwrap().remove("unknown_reason");
                        break;
                    }
                }
            }),
        ),
        (
            "unknown_reason is only allowed with an unknown identity",
            Box::new(|r: &mut Value| {
                for entry in r["entries"].as_array_mut().unwrap() {
                    if entry["expected_sha256"] != "unknown" && entry["expected_bytes"] != "unknown"
                    {
                        entry["unknown_reason"] = serde_json::json!("not needed");
                        break;
                    }
                }
            }),
        ),
        (
            "missing_window is malformed",
            Box::new(|r: &mut Value| {
                r["entries"][0]["missing_window"] =
                    serde_json::json!({"after": "yesterday", "before": "unknown"})
            }),
        ),
        (
            "expected_bytes must be an integer or unknown",
            Box::new(|r: &mut Value| r["entries"][0]["expected_bytes"] = serde_json::json!(-1)),
        ),
        (
            "schema_version must be",
            Box::new(|r: &mut Value| {
                r["schema_version"] = serde_json::json!("rsia.evidence_missing.v0")
            }),
        ),
        (
            "missing-evidence entry keys differ",
            Box::new(|r: &mut Value| r["entries"][0]["restored"] = serde_json::json!(true)),
        ),
    ];
    // Each label is the expected error substring (review R-09).
    for (label, mutate) in mutations {
        let mut changed = register.clone();
        mutate(&mut changed);
        let bytes = serde_json::to_vec_pretty(&changed).unwrap();
        let fixture = RegisterFixture::new(&bytes);
        let error = validate_missing_register(
            &pinned(&baseline, &bytes),
            &fixture.root,
            &records,
            &register_pins(&bytes),
            None,
        )
        .expect_err("a structural register defect");
        assert!(error.contains(label), "{label}: {error}");
    }
    let mut moved = baseline.clone();
    moved["evidence_missing_register"]["path"] = Value::String("reports/other.json".into());
    let error = validate_missing_register(&moved, &healthy.root, &records, &reviewed, None)
        .expect_err("a moved register");
    assert!(
        error.contains("evidence_missing_register.path must be"),
        "{error}"
    );
    let mut unpinned = baseline;
    unpinned
        .as_object_mut()
        .unwrap()
        .remove("evidence_missing_register");
    assert!(validate_manifest(&unpinned, &root).is_err());
}

#[test]
fn missing_evidence_register_judges_out_paths_against_an_evidence_store() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    let original = register_bytes(&root);
    let register: Value = load_json_strict(std::str::from_utf8(&original).unwrap()).unwrap();
    let reviewed = ReviewedPins::reviewed();
    let checkout = RegisterFixture::new(&original);
    let store = EvidenceStoreFixture::new();
    let (records, written) =
        complete_store(&collect_records(&baseline["e_scopes"]), &register, &store);
    assert!(!written.is_empty());
    // A complete store: every unregistered out/ path present, the 42 registered paths absent.
    validate_missing_register(
        &baseline,
        &checkout.root,
        &records,
        &reviewed,
        Some(store.root.as_path()),
    )
    .unwrap();

    // An absent out/ path outside the register fails once the store is supplied; without the store
    // it is not judged.
    let (_, _, victim) = &written[0];
    store.remove(victim);
    let error = validate_missing_register(
        &baseline,
        &checkout.root,
        &records,
        &reviewed,
        Some(store.root.as_path()),
    )
    .expect_err("an unregistered missing out/ path");
    assert!(
        error.contains(victim.as_str())
            && error.contains("is missing and not listed in the missing-evidence register"),
        "{error}"
    );
    validate_missing_register(&baseline, &checkout.root, &records, &reviewed, None).unwrap();
    store.write(victim, &fixture_evidence(victim));

    // A registered path reappearing in the store: an unknown identity always fails, a known one must
    // match; the checkout alone does not see either.
    let entries = register["entries"].as_array().unwrap();
    let unknown = entries
        .iter()
        .find(|entry| entry["expected_sha256"] == "unknown")
        .unwrap()["path"]
        .as_str()
        .unwrap();
    store.write(unknown, b"restored from somewhere\n");
    let error = validate_missing_register(
        &baseline,
        &checkout.root,
        &records,
        &reviewed,
        Some(store.root.as_path()),
    )
    .expect_err("an unknown-identity path reappeared in the store");
    assert!(error.contains("unknown identity"), "{error}");
    validate_missing_register(&baseline, &checkout.root, &records, &reviewed, None).unwrap();
    store.remove(unknown);
    let known = entries
        .iter()
        .find(|entry| entry["expected_sha256"] != "unknown" && entry["field"] == "log_path")
        .unwrap()["path"]
        .as_str()
        .unwrap();
    store.write(known, b"different bytes\n");
    let error = validate_missing_register(
        &baseline,
        &checkout.root,
        &records,
        &reviewed,
        Some(store.root.as_path()),
    )
    .expect_err("a mismatched path reappeared in the store");
    assert!(error.contains("different bytes or SHA-256"), "{error}");
    store.remove(known);

    // A present unregistered input manifest must match its record's input_digest (Python parity).
    let (_, _, input) = written
        .iter()
        .find(|(_, field, _)| field == "input_ref")
        .unwrap();
    store.write(input, b"tampered input manifest\n");
    let error = validate_missing_register(
        &baseline,
        &checkout.root,
        &records,
        &reviewed,
        Some(store.root.as_path()),
    )
    .expect_err("a tampered input manifest");
    assert!(
        error.contains("local input manifest SHA-256 mismatch"),
        "{error}"
    );
    store.write(input, &fixture_evidence(input));
    validate_missing_register(
        &baseline,
        &checkout.root,
        &records,
        &reviewed,
        Some(store.root.as_path()),
    )
    .unwrap();

    // Probe c3 against the store (one evidence file deleted and registered in place of another
    // entry, the count unchanged): the refreshed pointer fails on the reviewed register SHA-256,
    // re-pinned on the reviewed triples, and with every pin refreshed the dropped entry is an
    // unregistered missing out/ path that the store judgment rejects.
    let (record_id, field, path) = written
        .iter()
        .find(|(_, _, path)| written.iter().filter(|(_, _, other)| other == path).count() == 1)
        .unwrap()
        .clone();
    store.remove(&path);
    let mut swapped = register.clone();
    let dropped = swapped["entries"].as_array_mut().unwrap().remove(0);
    swapped["entries"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": path,
            "record_id": record_id,
            "field": field,
            "expected_bytes": "unknown",
            "expected_sha256": "unknown",
            "unknown_reason": "store probe",
            "missing_window": "unknown",
            "group": "store_probe",
        }));
    assert_eq!(swapped["entries"].as_array().unwrap().len(), 42);
    let swapped_bytes = serde_json::to_vec_pretty(&swapped).unwrap();
    let swapped_checkout = RegisterFixture::new(&swapped_bytes);
    let manifest = pinned(&baseline, &swapped_bytes);
    let error = validate_missing_register(
        &manifest,
        &swapped_checkout.root,
        &records,
        &reviewed,
        Some(store.root.as_path()),
    )
    .expect_err("a refreshed pointer");
    assert!(
        error.contains("is not the reviewed register SHA-256 pinned in both checkers"),
        "{error}"
    );
    let mut all_repinned = register_pins(&swapped_bytes);
    let error = validate_missing_register(
        &manifest,
        &swapped_checkout.root,
        &records,
        &all_repinned,
        Some(store.root.as_path()),
    )
    .expect_err("a swapped triple");
    assert!(
        error.contains("missing-evidence register entries differ from the reviewed register"),
        "{error}"
    );
    all_repinned.missing = swapped["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["record_id"].as_str().unwrap().to_string(),
                entry["field"].as_str().unwrap().to_string(),
                entry["path"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let error = validate_missing_register(
        &manifest,
        &swapped_checkout.root,
        &records,
        &all_repinned,
        Some(store.root.as_path()),
    )
    .expect_err("the dropped entry is an unregistered missing path");
    assert!(
        error.contains(dropped["path"].as_str().unwrap())
            && error.contains("is missing and not listed in the missing-evidence register"),
        "{error}"
    );
}

#[test]
fn harness2_borrowings_and_sources_match_the_plan_tables() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    validate_manifest(&baseline, &root).unwrap();
    assert_eq!(
        baseline["traceability"]["hsf_sources"]
            .as_array()
            .unwrap()
            .len(),
        17
    );
    assert_eq!(
        baseline["traceability"]["hs_borrowings"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    for (index, field, invalid) in [
        (0, "commit", serde_json::json!("a".repeat(40))),
        (0, "tree", serde_json::json!("b".repeat(40))),
        (3, "blob_sha", serde_json::json!("c".repeat(40))),
        (3, "path", serde_json::json!("harness2/core/other.py")),
        (1, "license", serde_json::json!("MIT")),
        (
            1,
            "repository",
            serde_json::json!("https://github.com/google/harness2"),
        ),
        (2, "read_range", serde_json::json!("全文")),
        (2, "e_tasks", serde_json::json!(["E00"])),
        (16, "blob_sha", serde_json::json!("d".repeat(40))),
    ] {
        let mut value = baseline.clone();
        value["traceability"]["hsf_sources"][index][field] = invalid;
        assert!(
            validate_manifest(&value, &root).is_err(),
            "HSF {index} {field}"
        );
    }
    let mut missing = baseline.clone();
    missing["traceability"]["hsf_sources"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(validate_manifest(&missing, &root).is_err());
    for (index, field, invalid) in [
        (0, "section", serde_json::json!("§6.7.5")),
        (0, "hsf_refs", serde_json::json!(["HSF04"])),
        (2, "e_tasks", serde_json::json!(["E09"])),
        (6, "v_scenarios", serde_json::json!(["V052"])),
        (4, "adoption", serde_json::json!("adopted")),
        (4, "implementation_status", serde_json::json!("planned")),
        (0, "verification_status", serde_json::json!("verified")),
        (0, "effect_status", serde_json::json!("improved")),
        (0, "remaining", serde_json::json!("")),
        (
            0,
            "related_evidence_refs",
            serde_json::json!(["E01.controller_acceptance"]),
        ),
        (0, "benefit", serde_json::json!("improved")),
    ] {
        let mut value = baseline.clone();
        value["traceability"]["hs_borrowings"][index][field] = invalid;
        assert!(
            validate_manifest(&value, &root).is_err(),
            "HS {index} {field}"
        );
    }
}

#[test]
fn v121_to_v123_and_v048_follow_the_v45_responsibility_table() {
    let mapping = expected_v_to_e();
    assert_eq!(mapping["V121"], split_set("E03 E07 E09 E13 E16"));
    assert_eq!(mapping["V122"], split_set("E01 E07 E09 E16"));
    assert_eq!(mapping["V123"], split_set("E00 E16 E16.5 E16.6"));
    assert_eq!(mapping["V048"], split_set("E02 E05"));
    assert!(
        !mapping["V081"].contains("E06"),
        "E06 only touches V081 (Q-49)"
    );
    let root = repo_root();
    let baseline = load_manifest(&root);
    for (vid, drop) in [
        ("V121", "E13"),
        ("V122", "E01"),
        ("V123", "E16.5"),
        ("V048", "E05"),
    ] {
        // Probe d (V121/E13) and its siblings.
        let mut value = baseline.clone();
        let entry = value["traceability"]["v_scenarios"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry["id"] == vid)
            .unwrap();
        entry["e_tasks"]
            .as_array_mut()
            .unwrap()
            .retain(|task| task != drop);
        let error = validate_manifest(&value, &root).expect_err("a dropped responsibility");
        assert!(
            error.contains("E responsibility mapping differs"),
            "{vid}: {error}"
        );
    }
    let mut v081 = baseline;
    let entry = v081["traceability"]["v_scenarios"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["id"] == "V081")
        .unwrap();
    entry["e_tasks"]
        .as_array_mut()
        .unwrap()
        .push(Value::String("E06".into()));
    assert!(validate_manifest(&v081, &root).is_err());
}

#[test]
fn stage8_gate_scripts_are_registered_execution_runners() {
    for runner in ["ctrl_verify_v43.sh", "ctrl_verify_v45.sh"] {
        let mut execution = well_formed_execution();
        execution["runner"] = Value::String(runner.into());
        execution_result(execution).unwrap();
    }
    for runner in [
        "ctrl_verify_v44.sh",
        "ctrl_verify_v46.sh",
        "codex-r7-ctrl_verify.sh",
    ] {
        let mut execution = well_formed_execution();
        execution["runner"] = Value::String(runner.into());
        assert!(execution_result(execution).is_err(), "{runner}");
    }
}

#[test]
fn trace_index_coverage_spans_v123_and_the_harness2_ranges() {
    let root = repo_root();
    let baseline = load_manifest(&root);
    for (task, range) in [
        ("E16.6", "V001–V123"),
        ("E16.6", "HSP01"),
        ("E00", "V087–V123"),
        ("E00", "HSF01–HSF16"),
    ] {
        let mut value = baseline.clone();
        value["trace_index_coverage"][task]["ranges"]
            .as_array_mut()
            .unwrap()
            .retain(|item| item != range);
        assert!(validate_manifest(&value, &root).is_err(), "{task} {range}");
    }
}
