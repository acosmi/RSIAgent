#!/usr/bin/env python3
"""Read-only validator for the frozen rsia.support_scope.v2 derivative.

The checker never executes manifest commands, starts subprocesses, accesses the
network, or writes repository files. Exit 0 means structure only; it is not an
acceptance, effect, license, host, deployment, or merge decision.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

PROJECT_LICENSE_SHA256 = "164d42969d38c372e7cd85fe0daf749b6e8409fbd28afd36d1d2b098c5e9debb"
PROJECT_LICENSE_README = 'RSIAgent is source-available under a research- and community-friendly non-commercial license (`source_available_personal_noncommercial_free`). The root `LICENSE` governs all grants: personal learning, academic and scientific research, educational use, benchmarking, and internal non-commercial technical evaluation (by individuals, universities, research institutions, non-profits, or engineering teams) are free of charge; open-source forking, academic reproduction artifacts, and non-commercial community sharing of modifications are permitted provided the original copyright notice and `LICENSE` are retained; deploying RSIAgent in commercial production operations, offering it as a paid or managed cloud/SaaS service, or embedding/distributing it in commercial products or revenue-generating services requires prior written authorization from Acosmi (fushihua@acosmi.com, https://acosmi.com). Third-party licenses and notices remain applicable to their corresponding material. Workspace crates remain unpublished.'
PROJECT_LICENSE_MEMBERS = ['apps/rsia', 'crates/evo-core', 'crates/evo-engine', 'crates/evo-http', 'crates/evo-mcp', 'crates/evo-storage']

PLAN_VERSION = "v4.5"
PLAN_SHA256 = "f6f9b1e44f3b1f364c0db8720f7eaec490ed7a03ddc053de8bfbab65326e6164"
# Registered plan lineage (plan §18.9–§18.11), oldest first; the last entry is the current binding. The manifest
# top level and the external plan file must be the current version. Historical evidence records keep the
# plan_version/plan_sha256 they were verified against and are accepted only as a registered pair. v4.4 was never
# bound by the index on its own; the v4.5 rebind registers it in the lineage (plan §18.11).
PLAN_LINEAGE = {
    "v4.1": {"file": "RSIAgent-v4.1定稿-工程实施方案-2026-09-19.md", "sha256": "45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150"},
    "v4.2": {"file": "RSIAgent-v4.2定稿-工程实施方案-2026-09-30.md", "sha256": "70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455"},
    "v4.3": {"file": "RSIAgent-v4.3定稿-工程实施方案-2026-10-12.md", "sha256": "73dd00bf5ef6e9757d0c391c9a8be4ca92b6e06a750aacb542817f82cbd208a7"},
    "v4.4": {"file": "RSIAgent-v4.4定稿-工程实施方案-2026-10-23.md", "sha256": "45ae29d71396f77dffc3baab7fed50564334734623bcec15f267778ddebeb496"},
    "v4.5": {"file": "RSIAgent-v4.5定稿-工程实施方案-2026-10-24.md", "sha256": "f6f9b1e44f3b1f364c0db8720f7eaec490ed7a03ddc053de8bfbab65326e6164"},
}
# Per-record recheck marker (plan §18.10 machine form, v4.5 Q-50; §18.5). "required" is a pending obligation and never
# passes, and a rechecked_<version> marker must name a lineage version later than the record's own plan_version. The
# expected marker follows from the record (FIELD_SEMANTICS["plan_recheck"]): a record bound to the current plan carries
# not_affected explicitly (§18.5); an older record of the v4.5 recheck union carries PLAN_RECHECKED; a v4.1/v4.2 record of
# a task outside that union keeps V43_RECHECKED when v4.3 rechecked its task, because a record outside the current union
# keeps its historical value (§18.10); every other record keeps not_affected (explicit or absent). v4.4 was never bound
# by the index on its own, so rechecked_v4.4 is not a marker.
PLAN_RECHECKED = "rechecked_v4.5"
V43_RECHECKED = "rechecked_v4.3"
PLAN_RECHECK_VALUES = {"not_affected", "required", "rechecked_v4.2", V43_RECHECKED, PLAN_RECHECKED}
# Exact E-task union of the landing points of plan §19.6 (as corrected by v4.5) and §19.7 (plan §18.11); the keys
# match literally, so "E16" is the parent key only and is not expanded to E16.1–E16.6 (plan §18.10, v4.5 Q-50).
PLAN_RECHECK_TASKS = {"E00", "E01", "E02", "E03", "E04", "E05", "E06", "E07", "E08", "E09", "E10", "E13", "E16", "E16.1", "E16.3", "E16.5", "E16.6"}
# Tasks rechecked by the v4.3 revision (AG-124/#165): every task except the optional extensions E17/E18.
V43_RECHECK_TASKS = {"E00", "E01", "E02", "E03", "E04", "E05", "E06", "E07", "E08", "E09", "E10", "E11", "E12", "E13", "E14", "E15", "E16", "E16.1", "E16.2", "E16.3", "E16.4", "E16.5", "E16.6"}
V43_RECHECK_FROM = {"v4.1", "v4.2"}
# supersedes/completes exist since plan v4.5 (§18.10 v4.5 supplement); a record carrying them is bound to v4.5 or later.
RELATION_MIN_VERSION = "v4.5"
SCHEMA_VERSION = "rsia.support_scope.v2"
STATUSES = {"planned", "in_progress", "blocked", "implemented_not_verified", "verified", "explicitly_out_of_scope"}
SKILLOPT_REPO = "https://github.com/microsoft/SkillOpt.git"
SKILLOPT_COMMIT = "79124b37e9a6371e13b753f8bcd7adb1e493ade1"
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
PR_RE = re.compile(r"^https://github\.com/acosmi/RSIAgent/pull/(\d+)$")
IDENT_RE = re.compile(r"^[A-Za-z0-9_-]+$")
# Progress order used for the parent/child gate rule. blocked and in_progress are the same level
# ("started, unfinished"); explicitly_out_of_scope has no rank and neither constrains nor is constrained.
STATUS_RANK = {"planned": 0, "blocked": 1, "in_progress": 1, "implemented_not_verified": 2, "verified": 3}
# Optional per-record PR state. When present it must agree with merged_sha (merged <=> merged_sha set).
PR_STATES = {"draft", "open", "closed", "merged"}
# Records of pull requests numbered above this value (fifth round onward) must carry `execution`; earlier records may
# omit it and are not backfilled or guessed.
EXECUTION_REQUIRED_ABOVE_PR = 141
# Machine fields must not read as execution records when they are not (plan E16.6: a derived index that conflicts with
# the body is rejected, and E00: real exit codes only). `command`, `exit_code` and `log_path` describe the declared
# single target; the optional record-level `execution` object states what was literally run. The meaning of every such
# field is pinned here and must be restated verbatim in the manifest's top-level `field_semantics`.
FIELD_SEMANTICS = {
    "command": "declared single-target checker command; syntax validated, not necessarily the literally executed command",
    "exit_code": "exit code of the controller gate run that covered the declared target",
    "log_path": "local evidence file containing that target's result (controller test log or acceptance receipt)",
    "execution": (
        f"optional record-level object, required when the pull request number is above {EXECUTION_REQUIRED_ABOVE_PR}, stating what was actually run: "
        "runner (ctrl_verify.sh, codex_task or other), command (the literally executed command), scope (workspace or single_target), "
        "exit_code (the integer exit code of that command) and evidence (the log_path or the identifier of the summary line inside the "
        "acceptance receipt); command, exit_code and log_path of the record keep describing the declared target; "
        "earlier records are neither backfilled nor guessed"
    ),
    "production_consumer": (
        "optional record-level marker: none_library_only means the recorded module has no production caller in engine/http/mcp/apps "
        "as of the commit named in the record scope; wired means a production caller exists; an absent marker means this was not assessed"
    ),
    # Appended by the v4.5 rebind (plan §18.10 machine form, v4.5 Q-50); the five definitions above are unchanged.
    "execution_runners": (
        "execution.runner additionally accepts the stage-8 gate scripts ctrl_verify_v43.sh and ctrl_verify_v45.sh; "
        "the runner names stated in the execution definition keep their meaning"
    ),
    "supersedes": (
        "optional record-level array of withdrawn record ids that this re-verification replaces; every id must be listed in withdrawn_records with "
        "status superseded and belong to the same E task; a record carrying supersedes is bound to plan v4.5 or later; the replaced record is neither "
        "rewritten nor revived"
    ),
    "completes": (
        "optional record-level array of still-valid record ids of the same E task whose evidence this new record completes (for example a re-run of a "
        "record with registered missing evidence); a record carrying completes is bound to plan v4.5 or later; the completed records stay unchanged "
        "and keep their own credit"
    ),
    "note": "optional record-level free-text note (for example a category or the task that will complete the record); it carries no verification credit",
    "withdrawn_records": (
        "top-level list of records withdrawn from verified_subscopes because a revision changed the behavior they asserted: id, withdrawn_in_pr, "
        "clause, owner_task and status (pending_recheck, superseded or replaced_by_binding); every entry is pinned as a reviewed (id, "
        "withdrawn_in_pr, clause, owner_task, status) tuple in both checkers and a listed id never reappears in verified_subscopes"
    ),
    "evidence_missing_register": (
        "top-level path and SHA-256 of the missing-evidence register, whose (record_id, field, path) entries are pinned in both checkers: an absent "
        "registered path is reported as missing, or as registered, not checked when it lies in the local evidence store and that store is not "
        "supplied; an absent path outside the register fails once its location is checked (outside out/ always, inside out/ with the local evidence "
        "store); a registered path that reappears fails unless its registered bytes and SHA-256 match (a path registered with unknown identity always "
        "fails on reappearance)"
    ),
    # Appended by the AG-182 rework (review R-01/R-03/R-06/R-07): the recheck rule and the V080.d pins.
    "plan_recheck": (
        "per-record recheck marker of the plan §18.10 machine form: a record bound to the current plan carries not_affected explicitly; a record "
        "bound to an earlier plan carries rechecked_v4.5 when its task is in the v4.5 recheck union, keeps rechecked_v4.3 when it was verified "
        "against v4.1 or v4.2 and its task is outside that union but was rechecked by v4.3 (every task except E17 and E18), and otherwise keeps "
        "not_affected (explicit or absent); rechecked_v4.5 is valid only inside the v4.5 recheck union, rechecked_v4.4 is never valid because v4.4 "
        "was never bound by the index on its own, and required never passes"
    ),
    "record_pins": (
        "both checkers pin the reviewed supersedes and completes pairs (new record id to old record id) and, for every historical record retained "
        "from the previous index binding, the SHA-256 of its canonical JSON (keys sorted, no insignificant whitespace, UTF-8) without plan_recheck; a "
        "missing, extra or rewritten pin fails; the records added by an index pull request are pinned by the next index pull request, after the merge "
        "of the adding pull request is backfilled"
    ),
}
EXECUTION_KEYS = {"runner", "command", "scope", "exit_code", "evidence"}
EXECUTION_RUNNERS = {"ctrl_verify.sh", "ctrl_verify_v43.sh", "ctrl_verify_v45.sh", "codex_task", "other"}
EXECUTION_SCOPES = {"workspace", "single_target"}
# `execution.command` must be what was run, not the declared single-target command copied a second time.
DECLARED_COMMAND_PREFIX = "cargo test --locked --offline"
PRODUCTION_CONSUMER_VALUES = {"wired", "none_library_only"}
# Closed key sets per object type, derived from reports/support-scope.json (rsia.support_scope.v2).
# Keys that the checker reads but the current manifest does not yet use (completion_evidence_refs,
# verification_evidence_refs, pr_state) are listed so legitimate progress stays registrable.
ROOT_KEYS = {
    "schema_version", "plan_version", "plan_sha256", "plan_lineage", "derived_index_notice", "field_semantics", "declaration",
    "not_full_route_complete", "legacy_equivalence_unverified", "dimensions", "blocked", "explicitly_out_of_scope_until_revised",
    "optional_disabled", "overall_remaining", "e_scopes", "traceability", "trace_index_coverage", "project_license",
    "withdrawn_records", "evidence_missing_register",
}
LINEAGE_KEYS = {"version", "file", "sha256"}
DIMENSION_KEYS = {"engineering", "host", "model", "effect", "deploy"}
COVERAGE_KEYS = {"purpose", "ranges"}
SCOPE_KEYS = {"title", "status", "status_reason", "implementation_files", "scenarios", "verified_subscopes", "remaining", "completion_evidence_refs"}
EVIDENCE_KEYS = {
    "id", "description", "plan_version", "plan_sha256", "plan_recheck", "source_sha", "pr", "merged_sha", "pr_state", "test_entry", "command",
    "input_digest", "exit_code", "log_path", "actual_result", "scope", "risk", "rollback", "record_source", "input_ref",
    "execution", "production_consumer", "supersedes", "completes", "note",
}
TRACEABILITY_KEYS = {"b_commitments", "u_commitments", "k_commitments", "so_sources", "v_scenarios", "hs_borrowings", "hsf_sources"}
COMMITMENT_KEYS = {
    "id", "specification_status", "sections", "sources", "e_tasks", "v_scenarios", "implementation_status", "verification_status",
    "effect_status", "local_artifacts", "related_evidence_refs", "remaining", "completion_evidence_refs",
}
SO_SOURCE_KEYS = {
    "id", "repository", "commit", "path", "blob_sha", "read_range", "e_tasks", "license", "license_status", "verification_status",
    "local_use", "verification_evidence_refs", "completion_evidence_refs",
}
LOCAL_USE_KEYS = {"mode", "artifacts"}
V_SCENARIO_KEYS = {"id", "specification_status", "e_tasks", "status", "verification_status", "effect_status", "related_evidence_refs", "remaining", "completion_evidence_refs"}
V_SCENARIO_COUNT = 123

# Withdrawn records (plan §18.10, v4.5 Q-50): the pending-recheck list. Every (id, withdrawn_in_pr, clause, owner_task,
# status) tuple is reviewed and pinned here and in crates/evo-core/tests/support_scope.rs, so dropping or rewriting an entry
# fails; ids never return to verified_subscopes.
WITHDRAWN_KEYS = {"id", "withdrawn_in_pr", "clause", "owner_task", "status"}
WITHDRAWN_STATUSES = {"pending_recheck", "superseded", "replaced_by_binding"}
OWNER_TASK_RE = re.compile(r"^AG-\d{3}[a-z]?$")
EXPECTED_WITHDRAWN = {
    # withdrawn by PR #165 (AG-124, v4.3 recheck) and PR #169 (AG-128, E04.ag054 correction)
    "E10.controller_acceptance": (165, "§7.4.2–§7.4.3", "AG-182", "superseded"),
    "E10.replay_management_controller_acceptance": (165, "§7.4.2–§7.4.3", "AG-182", "superseded"),
    "E10.ag064_replay_episode_recovery": (165, "§7.4.2–§7.4.3", "AG-182", "superseded"),
    "E10.ag103_replay_fairness_waits_legal_set_v42": (165, "§7.4.2–§7.4.3", "AG-182", "superseded"),
    "E08.ag037_economic_replay_cleanup": (165, "§7.4.2–§7.4.3", "AG-182", "superseded"),
    "E09.ag074_local_scoped_acceptance": (165, "§7.4.2–§7.4.3", "AG-182", "superseded"),
    "E16.6.ag012_plan_v42_index_binding": (165, "§18.9", "AG-124", "replaced_by_binding"),
    "E04.ag097_broker_close_before_refuse": (165, "§11.3.1/§11.6", "AG-119", "pending_recheck"),
    "E04.ag105_broker_bound_recovery_close_v42": (165, "§11.3.1/§11.6", "AG-119", "pending_recheck"),
    "E08.ag046_write_side_revocation": (165, "§11.3.1", "AG-119", "pending_recheck"),
    "E08.ag059_manifest_prewrite_gate": (165, "§11.3.1", "AG-119", "pending_recheck"),
    "E04.ag054_typed_predispatch": (169, "§7.3.1", "AG-132", "pending_recheck"),
    "E06.controller_acceptance": (165, "§5.5.2", "AG-169", "pending_recheck"),
    "E13.controller_acceptance": (165, "§5.5.2", "AG-169", "pending_recheck"),
    "E16.4.real_store_rejection_controller_acceptance": (165, "§5.5.2", "AG-169", "pending_recheck"),
    "E09.ag042_recovery_counting": (165, "§7.2.3", "AG-171", "pending_recheck"),
    "E13.ag028_consolidation_trigger": (165, "§6.1.2", "AG-139", "pending_recheck"),
    "E16.1.durable_import_controller_acceptance": (165, "§11.2.1", "AG-148", "pending_recheck"),
    "E02.ag062_required_improver_baselines": (165, "§5.1.2", "AG-135b", "pending_recheck"),
    # withdrawn by the v4.5 rebind (AG-182): v4.4 Q-29/Q-32 and v4.5 Q-57 changed the asserted behavior
    "E09.ag033_run_next_trusted_gate": (200, "§6.7.4", "AG-187", "pending_recheck"),
    "E16.6.ag131_v43_project_license_metadata": (200, "§2.8.1", "AG-182", "superseded"),
    "E05.ag052_versioned_batch_verdicts": (200, "§3.4", "AG-192", "pending_recheck"),
    "E05.ag080_local_scoped_acceptance": (200, "§3.4", "AG-192", "pending_recheck"),
    "E05.ag101_streaming_verdicts_v42": (200, "§3.4", "AG-192", "pending_recheck"),
}
EXPECTED_WITHDRAWN_IDS = frozenset(EXPECTED_WITHDRAWN)
# Reviewed relation pairs (kind, new record id, old record id) of plan §18.10 / V080.d: 7 supersedes and 25 completes.
EXPECTED_RELATIONS = {
    ("supersedes", "E08.ag182_replay_v3_economic_cleanup", "E08.ag037_economic_replay_cleanup"),
    ("supersedes", "E09.ag182_replay_v3_legal_prefix", "E09.ag074_local_scoped_acceptance"),
    ("supersedes", "E10.ag182_replay_v3_controller_acceptance", "E10.controller_acceptance"),
    ("supersedes", "E10.ag182_replay_v3_episode_recovery", "E10.ag064_replay_episode_recovery"),
    ("supersedes", "E10.ag182_replay_v3_fairness_waits_legal_set", "E10.ag103_replay_fairness_waits_legal_set_v42"),
    ("supersedes", "E10.ag182_replay_v3_management_controller_acceptance", "E10.replay_management_controller_acceptance"),
    ("supersedes", "E16.6.ag182_v45_project_license_metadata", "E16.6.ag131_v43_project_license_metadata"),
    ("completes", "E00.ag182_rerun_controller_acceptance", "E00.controller_acceptance"),
    ("completes", "E01.ag182_rerun_controller_acceptance", "E01.controller_acceptance"),
    ("completes", "E02.ag182_rerun_ag092_local_scoped_acceptance", "E02.ag092_local_scoped_acceptance"),
    ("completes", "E02.ag182_rerun_ag093_local_scoped_acceptance", "E02.ag093_local_scoped_acceptance"),
    ("completes", "E02.ag182_rerun_controller_acceptance", "E02.controller_acceptance"),
    ("completes", "E03.ag182_rerun_controller_acceptance", "E03.controller_acceptance"),
    ("completes", "E04.ag182_rerun_ag077_local_scoped_acceptance", "E04.ag077_local_scoped_acceptance"),
    ("completes", "E04.ag182_rerun_controller_acceptance", "E04.controller_acceptance"),
    ("completes", "E05.ag182_rerun_ag083_partition_admission_rejection_atomicity", "E05.ag083_partition_admission_rejection_atomicity"),
    ("completes", "E05.ag182_rerun_ag087_local_scoped_acceptance", "E05.ag087_local_scoped_acceptance"),
    ("completes", "E05.ag182_rerun_ag088_local_scoped_acceptance", "E05.ag088_local_scoped_acceptance"),
    ("completes", "E05.ag182_rerun_ag089_local_scoped_acceptance", "E05.ag089_local_scoped_acceptance"),
    ("completes", "E05.ag182_rerun_ag090_local_scoped_acceptance", "E05.ag090_local_scoped_acceptance"),
    ("completes", "E05.ag182_rerun_ag091_local_scoped_acceptance", "E05.ag091_local_scoped_acceptance"),
    ("completes", "E05.ag182_rerun_controller_acceptance", "E05.controller_acceptance"),
    ("completes", "E06.ag182_rerun_ag084_local_scoped_acceptance", "E06.ag084_local_scoped_acceptance"),
    ("completes", "E07.ag182_rerun_controller_acceptance", "E07.controller_acceptance"),
    ("completes", "E08.ag182_rerun_ag075_local_scoped_acceptance", "E08.ag075_local_scoped_acceptance"),
    ("completes", "E08.ag182_rerun_controller_acceptance", "E08.controller_acceptance"),
    ("completes", "E09.ag182_rerun_controller_acceptance", "E09.controller_acceptance"),
    ("completes", "E11.ag182_rerun_controller_acceptance", "E11.controller_acceptance"),
    ("completes", "E12.ag182_rerun_controller_acceptance", "E12.controller_acceptance"),
    ("completes", "E16.1.ag182_rerun_ag070_local_scoped_acceptance", "E16.1.ag070_local_scoped_acceptance"),
    ("completes", "E16.1.ag182_rerun_ag076_local_scoped_acceptance", "E16.1.ag076_local_scoped_acceptance"),
    ("completes", "E16.6.ag182_rerun_ag086_local_scoped_acceptance", "E16.6.ag086_local_scoped_acceptance"),
}
# V080.d "旧记录被改写": SHA-256 of the canonical JSON (keys sorted, no insignificant whitespace, UTF-8) without plan_recheck of each of
# the 93 historical records retained from the v4.3 index binding (base a389c4e3). The 45 records added by AG-182 are pinned by the
# next index pull request, after the merge of PR #200 is backfilled (FIELD_SEMANTICS["record_pins"]).
EXPECTED_RECORD_DIGESTS = {
    "E00.controller_acceptance": "1d8594a791d51459e7272f8c97bc17df016df597bd85b696ce2b2379313ee80a",
    "E00.ag099_step_future_default_test_stack": "ff982495a71a566a1a2f30c43a74ebce147321b32a65979fcc6ea9c9bfd229fa",
    "E01.controller_acceptance": "074a9cd7508f9ba3a13f2bc5f0894740ae57c8593ecc7d411c9e9d64d3990501",
    "E01.ag057_explicit_sample_registration": "1661bc81322d2817fcf025437d354b083a2e5b9fa83db9c07ab3a4a24b9b9bfe",
    "E02.controller_acceptance": "e394666658305fca71b0c7d631aa1b323dea92fd8cd555845ee74165640704c1",
    "E02.ag055_v1_wire_goldens": "906e3945c9b4fd329f7e99bee4f8300e886fc6571363fd22bd9acfab5ad5215a",
    "E02.ag065_host_surface_name_coverage": "c17b10748a3d1938f470192799bb72ca377bb117ea013c642b5d878120a40199",
    "E02.ag092_local_scoped_acceptance": "29bde3c9b96dc0ed4209cae7fce85ccb53aada9374ef38073eaa65ad51176c5c",
    "E02.ag093_local_scoped_acceptance": "2847402ce6df60554ac63bc9c6e557194b768c56f53f7eae2111c60fea2c86e6",
    "E02.ag126_v43_host_surface_duplicates": "4a6d22bd80043575aabd11caeff4dd92028912bd6ce033cb3116a49f4d64efde",
    "E03.controller_acceptance": "724c6e819805aaff53c31acf981b14f4ed45c66cbf30e879388ba30c095250ca",
    "E03.ag016_development_receipts": "2291dcc8302da2ddb57e19d0cadf992f145853d612c72e25497185201d90f916",
    "E03.ag079_local_scoped_acceptance": "0d6187cdd07d2302d608afb46d8c0dcec7d9ba718f69098cb6e32d51c63d641a",
    "E04.controller_acceptance": "f49955f820927e62185e48ed3e36e6daf97a1b518b21b7ec2b65662ce3bb9cbb",
    "E04.ag082_local_scoped_acceptance": "4f3bfad1a3766ec3f05185061823c5162c31e22b49406caf38c427302e19f976",
    "E04.ag077_local_scoped_acceptance": "5544b5435969d00c5edb1dc62fdb738e0b2d16ef16f86bb40160150e4bb446ed",
    "E04.ag121_v43_redacted_bound_request_identity": "e8e3528768d23ab3fdb673a819cb981345f12794e0d8280fb09d1624f93d0cbd",
    "E05.controller_acceptance": "561829b705c81d4754ede69684bb680c264411f28b7ccbca92c9c70d3b1a1c3a",
    "E05.ag083_partition_admission_rejection_atomicity": "a583638461b587598d2d1de13848710ef6e0268dde1685b91d95b3fa26278125",
    "E05.ag087_local_scoped_acceptance": "a826a4fd3234faaa99bcc00b54907549131ed911e9a2c226ba23b17fc3098c67",
    "E05.ag088_local_scoped_acceptance": "bf0c6a100434e38626ec4e66dc3cc763fa6864bb51210ecf972eda19477d3989",
    "E05.ag089_local_scoped_acceptance": "b488a11ca760fcfae119f7972142f0ffaab5ea0c1f3f11032fcb0d074e343b27",
    "E05.ag090_local_scoped_acceptance": "f8b4e7a1999f1426eb8098d9bf43d027a0e3940b41f79fbfbe26d468f0fa8694",
    "E05.ag091_local_scoped_acceptance": "3effa80c484dbb8810011f7501887f7f1bc4c611b407758d4d0d7ebed6c59a6d",
    "E05.ag095_canonical_grader_receipt_digest": "924974f332b9cf7bcb5138b631f90360ec724f57c8f392eaba01d5efa7e79640",
    "E05.ag109_external_host_authority_bound_ticket": "4254c96216040460b6f65594e7343a24416901afbdd079c4055bb781f3a37e5c",
    "E05.ag100_formal_candidate_dependency_binding_v42": "fb3d5e5739db8d1160bcae324927994183fd9daefccae406c0bd8587176b392d",
    "E05.ag118_late_receipts_after_final_report_v42": "1db33e283844a4f0f2b852ef99e1c58b772a2dea0805d65e1beeb7e87486cecb",
    "E06.ag084_local_scoped_acceptance": "15f35447ee36c450ad8fa9bd3f95dd4e587ff2b758bd1591e9b88abf43eb3cdb",
    "E06.ag116_edit_compiled_produce_review_v42": "74dc866a916baa251ba3283b6318403ae416564798946bb7a7e9cdfe7c99f5ba",
    "E07.controller_acceptance": "ac6314300943f30c0e27913b978be822f9387d553006e5d6e16f62b60d517f41",
    "E07.ag015_curriculum_step_consumer": "6e61f463ac09aa20e2c5d993d2c6d74d093093370955cc78f52ae46b938601d8",
    "E07.ag017_exploration_start_consumer": "aebd4d7fc8c56b3f39da0759083899fe36203a1044a39a9e22ae1bc343be2e54",
    "E07.ag027_management_cleanup_closure": "6af732292546dcb863e8c1a0c155de4a127109711d262b21a9b68524d2b496f9",
    "E07.ag044_submit_upstream_gate": "c673295669527dfd97da6fe6f46ee4ab02f7500a6afac11c40eb98f6027ec39e",
    "E07.ag108_candidate_review": "ad759df1c7f9182e866d8f1ccaf0cf9dfc0ce3b72faedf9c0889c4b3ffa19123",
    "E08.controller_acceptance": "c62759622bb999c43cb7ebf7bb40f67360e3a7b6421fe2caeddad1338e544991",
    "E08.ag036_revocation_source_collision": "fae74423b0845f16d88ae3378495089a087a2639230ae1f7ceab97f21ed5d122",
    "E08.ag035_cleanup_fixpoint": "2204561a86ab1cb38dc2b99844e229bfd063bb564e416f77e5be21e6b8ffdf25",
    "E08.ag038_budget_revoke_gate": "2bd2b2fa39b9f5566f572502b6bc6c9191d76c02eaa1ffe7c33eaa51a1fecc17",
    "E08.ag043_late_response_revoked": "ddefd6ba46b238eb37dda20f805a57019d0b6ea86b55958c5141754f7b30aa66",
    "E08.ag045_late_stage_fact_redaction": "2e57c45b6d4c8679531ae431d5728f87edc299664ccce772db057a5dd72d1bba",
    "E08.ag053_bounded_dependency_snapshots": "4a38d2bdfb2e3e241dddf8fe73d4a5f6f5e1ef879b6653321e0b0c4ba761b363",
    "E08.ag068_native_authority_cleanup": "37f7628c981293b236f058ac277063d4a062b6982f21ab9eb7c45bcf411ea77d",
    "E08.ag069_local_scoped_acceptance": "b83833fdeea7603cdfaacf17075d84dd645d8da683faca0ddff9fe6d5e69cc02",
    "E08.ag073_local_scoped_acceptance": "0424d968409ba37abd2ea887353ca3bae11da1e1312e29492a111263b7724459",
    "E08.ag075_local_scoped_acceptance": "24452482075fe9e8132119ccf6448e9ea4b9dbc03bd16bd1a24596cc9cb45df3",
    "E08.ag096_unreadable_bound_request_isolated": "a26967f7fedd6681424cadd4fa2508c73ebf77aa7a098f72c97863c668148a86",
    "E08.ag107_budget_ref_missing_fail_closed_v42": "a0a679df3d49ca1bd3ce663a3bf83b427c1cfb2cc2d42c690477deac0d597ed6",
    "E08.ag106_budget_startup_recovery_v42": "32f8e7825843418114547ad3f3d95fdc8872afa5663d6dd8ae3eed2ba9549744",
    "E08.ag104_restore_prefix_migration_upgrade_v42": "755f4769045cf640f44ad391eddb152e7a2c561c65b94e0aa51363115fe8a02a",
    "E08.ag117_formal_call_revoke_cleanup_v42": "e105db3dc22aa5e1eefbdf18fae36b01fadca3858e5f06c2e1aa1ceb5b1066a5",
    "E09.controller_acceptance": "f5ce8f536ed777d9534b2514867708d8b7ea8718be0e07c7184a262a7035c617",
    "E09.ag021_skill_group_jobs": "12c6f148b56305cd068652cecaa4bea1b9f5cb98fcc0540d680b8f8084c575bb",
    "E09.ag024_exploration_cleanup_edges": "7076ba1d4f789db61481c97e527aa1e50c4801db91d7b1b9416e86f8fc52fe60",
    "E09.ag026_practice_attempt_sets": "9f5054d227dbd8a2851f5709c3aa2365bd09909e9656d45cb0c66ff56b4828af",
    "E09.ag039_legal_actions": "26c2b8b3a7c4700ac2e10fce9ba2f7675cc348d7fb26feffe94720c7bed67b71",
    "E09.ag040_exploration_registration": "d940e29be319c6e21569d7d68df0977bb67bb4302942e398b70aa97f0e188d8b",
    "E09.ag041_postpaid_convergence": "384514f590f119120bbcab247d565328f27484cefe7d77df7fd0e1c308b82cc2",
    "E09.ag048_terminal_classes": "7c21051db6efae69f59f43de82fd099c9950cfd39c1267709c0fbbfef70b3f5c",
    "E09.ag103_fairness_waits_legal_set_v42": "0132841ac9a6043c5d32db733a7a9b8aff8270762fde2acba30c63043c656398",
    "E10.ag032_replay_redacted_reads": "a957960111997b98119c09151e57e4e949a35a6e8d312d2b6f373a09f9d4104b",
    "E11.controller_acceptance": "da5e5ea97acd78efeded949a10a9548a1617972bf576851777e94d1815ba8678",
    "E12.controller_acceptance": "86e83f3766896892230f8e092d2941223b7dfa00f3c87657ca3544b4377358a6",
    "E12.ag025_curriculum_cleanup": "e7848f07c68cc83acd4b5aa491db6a624b2e904dd52922c329920f99dbb64683",
    "E12.ag050_curriculum_priority": "178dcb3f3445b5ce93b6f6dbb421aaceb7bf4886dfe2906f1ce748fda4b1956e",
    "E13.ag022_consolidation_closure": "7a7440e4247874584b1b0177ec063175fad1b5f59fa0054c7469d383086f6a4d",
    "E13.ag030_consolidation_budget_stage": "36b9fc48a26ea31f0a4e12b779938232ce01b1e8602a7f231d87cc9b919e81e4",
    "E13.ag031_bounded_terminal_category": "77481ced5879ebab6c1fad3223385f6346eb3bcc38b5347446f4f26687564c27",
    "E13.ag081_local_scoped_acceptance": "3c0a40f2c518b1f89dc053088c57b6316c3e5000f23c60680bb5b353b6af3c34",
    "E14.ag019_exploration_policy_inheritance": "6ac6910902a56230da0de48ef8a421fed311979ed655312ffe2156f6689c9c02",
    "E14.ag034_metatrial_fork": "70570be81dc7f69159de7b9ab6b8698b69421357af209fa993254761ce08f681",
    "E14.ag058_current_description_boundaries": "968ace7f370ef39e775d0ae4bcb225e3a20bc27046b1c25077f208ac49212289",
    "E16.1.ag076_local_scoped_acceptance": "1db84fb47a4dd266efe79ff2d6cd62f7b300a3ea2526e307bc70adaf5a94bd12",
    "E16.1.ag070_local_scoped_acceptance": "f593c9ee97df37e97c455d4e70b0c1feafe6bd31911375f64327a6cb740d0eaf",
    "E16.2.durable_staging_publish_controller_acceptance": "adcb1f3a66b3a675cd4aa3e31a2c49c4e734f83042422740093650a24d3f4413",
    "E16.2.ag071_known_marker_utf8_blocking": "21554a03c30f5e2c6124aa7a9abf0ad598df1f736f144a4cc3ded4e42be539b8",
    "E16.2.ag085_utf8_privacy_boundary_regressions": "2cef286c6f53f1fa34318b4cca3b559676d2a369e46038557239ea997062f70b",
    "E16.3.seed_install_reset_controller_acceptance": "54785b9628e46df095c5eda46b691af653d93a90828c26c655b44f1995179faa",
    "E16.3.ag127_v43_seed_classification": "1705c3600e0c1a14370dd4c580837cc31f0fbfdffe254a53e052991aad0e56f8",
    "E16.4.ag066_local_stdio_harness": "4e6c261717492dc027fcfaafc2133ad37fe32a7496f47d945cf5dcd9d28a1617",
    "E16.4.ag126_v43_host_surface_record": "f5f66a5cadb290764631e415b3eddeb60cdbce534e777229382078a7359f528a",
    "E16.5.gate_functions_controller_acceptance": "cd8c6699299b7d6159b2d0fd0f7d530d6b987dfd1ddf0ae58b4c5fdacec7b802",
    "E16.5.ag014_capacity_gates_wired": "efe04bb278bd8ffdc4ec7da6d955fd312609b88a08188e4acf394d6236ec8faa",
    "E16.5.ag018_startup_recovery_gate": "bd15e029359e9afafd6d70a6560ebf33df1bfafdab24dec2f61e2bbba6ef2d74",
    "E16.5.ag029_restore_coverage": "13224ab8a4ce6aa16305738272f61c5d8ee3d7322ac7c61fc670375acb6be038",
    "E16.5.ag051_economic_restore": "ca1b560d5c179f8de71df3b770c63c99ef95146a683dcae79c5cca020f7072fa",
    "E16.5.ag125_v43_single_writer": "bc602ffe5639d5aaa0db5e54629a0237c30fd2dbba2d0957ff99a18ec4283d1f",
    "E16.5.ag129_v43_system_clock_conversion": "a9f3d6efb7173b6ca5bb1bd64946a1919212aeb90213076fc9f92d680af842f9",
    "E16.5.ag129_v43_broker_first_clock_refusal": "3505db766eb5baa3ca918da282110c07c64a448942776484451bdb409cb32349",
    "E16.6.ag086_local_scoped_acceptance": "a55a8e65cb303d4c683a356ce7ccb3922c7480ed1041722775506b5880c8c537",
    "E17.disabled_rejection_controller_acceptance": "6046a7d9fe6f644c02b88ad8b52ccd2a25aa16078563031332fb1e15c9084442",
    "E18.disabled_rejection_controller_acceptance": "75279240c222bbd5e5a65656afa30f7cb0b186253f95ec496d3b27f04acee0a1",
}

# Missing-evidence register (plan §18.10 "本地缺失的证据路径", v4.5 Q-50).
DEFAULT_KNOWN_MISSING = "reports/evidence-missing.json"
REGISTER_POINTER_KEYS = {"path", "sha256"}
MISSING_SCHEMA_VERSION = "rsia.evidence_missing.v1"
MISSING_REGISTER_KEYS = {"schema_version", "basis", "entries"}
MISSING_ENTRY_KEYS = {"path", "record_id", "field", "expected_bytes", "expected_sha256", "unknown_reason", "missing_window", "group"}
MISSING_WINDOW_KEYS = {"after", "before"}
# Reviewed (record_id, field, path) entries of the register (42), pinned here and in the Rust test so that removing one entry
# and registering another (same count, re-pinned SHA-256) fails in both checkers.
EXPECTED_MISSING_TRIPLES = {
    ("E00.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e00-directed-test.log"),
    ("E00.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e00.json"),
    ("E01.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e01-tests.log"),
    ("E01.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e01-input.json"),
    ("E02.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e02-tests.log"),
    ("E02.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e02-formatted-input.json"),
    ("E02.ag092_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG092-existing-current27df8c2-fixed39d2d86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E02.ag093_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG093-existing-currentc04850d-fixed49806d7-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r2-20261011.json"),
    ("E03.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e03-final-tests.log"),
    ("E03.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e03-final-input.json"),
    ("E04.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e04-storage-final.log"),
    ("E04.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e04-final-input.json"),
    ("E04.ag077_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG077-existing-PR125-current529-fixed26d3-ROOT-fresh12-actual-current-main-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E05.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e05-final-streaming.log"),
    ("E05.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e05-final-input.json"),
    ("E05.controller_acceptance", "record_source", "out/implementation-v4.1-20260919/controller-e05-acceptance.json"),
    ("E05.ag083_partition_admission_rejection_atomicity", "log_path", "out/scratch/ag-083-impl/current-fixed-single-clippy-repair-full8-direct-root-run-r1-user-continue-20261003/full-workspace-MAIN-original-native.json"),
    ("E05.ag087_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG087-existing-current77152-fixedfde3dd4-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E05.ag088_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG088-existing-current8cbf82c-fixede538b96-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E05.ag089_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG089-existing-currentbd20099-fixed6040bf3-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E05.ag090_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG090-existing-currentc9aa71e-fixeda994443-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E05.ag091_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG091-existing-current1f1b593-fixed730f242-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E06.ag084_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG084-existing-current349-fixed3c5-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E07.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e07-service.log"),
    ("E07.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e07-protocol-input.json"),
    ("E07.controller_acceptance", "record_source", "out/implementation-v4.1-20260919/controller-e07-protocol-acceptance.json"),
    ("E08.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e08-storage-tests.log"),
    ("E08.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e08-final-input.json"),
    ("E08.controller_acceptance", "record_source", "out/implementation-v4.1-20260919/controller-e08-acceptance.json"),
    ("E08.ag075_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG075-existing-current1132-fixed8a92e57-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E09.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e09-engine.log"),
    ("E09.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e09-input.json"),
    ("E09.controller_acceptance", "record_source", "out/implementation-v4.1-20260919/controller-e09-acceptance.json"),
    ("E11.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e11-core.log"),
    ("E11.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e11-final-input.json"),
    ("E11.controller_acceptance", "record_source", "out/implementation-v4.1-20260919/controller-e11-acceptance.json"),
    ("E12.controller_acceptance", "log_path", "out/implementation-v4.1-20260919/controller-e12-engine.log"),
    ("E12.controller_acceptance", "input_ref", "out/implementation-v4.1-20260919/controller-e12-final-input.json"),
    ("E12.controller_acceptance", "record_source", "out/implementation-v4.1-20260919/controller-e12-acceptance.json"),
    ("E16.1.ag076_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG076-existing-currentAA51-fixed1355e95-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E16.1.ag070_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG070-existing-current3ad-fixed887d97a-ROOT-standard8-head38-parent36-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
    ("E16.6.ag086_local_scoped_acceptance", "log_path", "out/audit-20260930/qa/takeover-20261002/AG086-existing-currentAA831-fixed908eb85-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json"),
}
EVIDENCE_PATH_FIELDS = ("input_ref", "log_path", "record_source")
UNKNOWN = "unknown"
ISO_UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
# The local evidence store is the untracked out/ tree of the primary checkout (.gitignore /out/). A clone or worktree
# does not carry it, so the absence of an out/ path is only judged when --evidence-root names the directory holding
# that tree; a present path, and any path outside out/, is always checked.
LOCAL_EVIDENCE_STORE = "out"

# Harness² borrowings HS01–HS08 (plan §2.9 table) and fixed sources HSF01–HSF16/HSP01 (plan §16.8 table).
HS_KEYS = {
    "id", "section", "hsf_refs", "e_tasks", "v_scenarios", "adoption", "specification_status", "implementation_status",
    "verification_status", "effect_status", "related_evidence_refs", "remaining",
}
HS_ADOPTIONS = {"adopted", "registered_only"}
EXPECTED_HS = {
    "HS01": ("§3.1.1/§6.7.5", "HSF04 HSF05", "E07 E09", "V122", "adopted"),
    "HS02": ("§6.7.2/§6.7.5", "HSF06", "E09", "V122", "adopted"),
    "HS03": ("§3.6/§6.7.5", "HSF05", "E09 E13", "V091 V122", "adopted"),
    "HS04": ("§5.8/§11.5", "HSF07 HSF09", "E03 E09", "V090", "adopted"),
    "HS05": ("§6.7.3", "HSF05", "E09", "V122", "registered_only"),
    "HS06": ("§3.2/§6.3", "HSF08", "E03 E05", "V090", "registered_only"),
    "HS07": ("§6.3", "HSF10", "E03 E16.1", "V052 V088", "adopted"),
    "HS08": ("§3.5/§3.6", "HSF11 HSF12", "E04 E07", "V096", "adopted"),
}
HSF_KEYS = {"id", "repository", "commit", "tree", "path", "blob_sha", "license", "read_range", "e_tasks"}
HARNESS2_REPO = "https://github.com/google-research/harness2"
HARNESS2_COMMIT = "1cfe254e8d25b15f37048c3730134686abe60335"
HARNESS2_TREE = "edc756a4d03a2826a248115482084a176e60e751"
HARNESS2_LICENSE = "Apache-2.0"
HSF_READ_RANGE = "ledger"  # actual read ranges are registered in the ledger (plan §16.8, §18.10 action contract)
HSF_E_TASKS = {"E00", "E16.5", "E16.6"}  # V123 source/license responsibility (plan §13.5)
EXPECTED_HSF = {
    "HSF01": ("README.md", "a5e91206948cd3d5bbed30c2a951923020e780d2"),
    "HSF02": ("LICENSE", "d645695673349e3947e8e5ae42332d0ac3164cd7"),
    "HSF03": ("docs/EXPERIMENTS.md", "bd52e1506037d172a03c128427881d5342bbace7"),
    "HSF04": ("harness2/core/recursion.py", "dda6b98cd5af100b8941f33cbb079462299c5280"),
    "HSF05": ("harness2/core/selection.py", "2cf04f50e35fd82d7abaea279a83b4a7eb4c2193"),
    "HSF06": ("harness2/core/prompts.py", "9ccacf23b64ac15b06a1939f26224960f56d757c"),
    "HSF07": ("harness2/core/library.py", "39e50684b4044e483a3c16a447958044d4b5e6de"),
    "HSF08": ("harness2/core/context.py", "72f2868b06c13e30c031cb91c0f04dcd80293ec9"),
    "HSF09": ("harness2/core/schedule.py", "f91f450b1d01011982069f4b41e6e3a77abcc9cc"),
    "HSF10": ("harness2/substrate/opencode.py", "ad0a2ea4d59bda362838c76912e43b6f7c333254"),
    "HSF11": ("harness2/core/report.py", "b7ecf6e42a1061218b0eb8cde9c441c136e28e72"),
    "HSF12": ("harness2/core/ledger.py", "319a6697758d05294dab9f42e8f8700759d7e6f6"),
    "HSF13": ("harness2/core/tree.py", "c9bf1341226aac448f6ec82038cb6d3896799412"),
    "HSF14": ("harness2/core/proposer.py", "4a235075cae72059fa701001f31bf4505cbbc3ba"),
    "HSF15": ("harness2/demo.py", "487de02d44941754d955ad8a4b0a9e0e6f841db5"),
    "HSF16": ("harness2/cli/run_sweep.py", "0876f4e68f3b92a4cf9d6838223c00e2ef960d27"),
}
# HSP01 is the paper, not a repository object: repository/commit/tree/blob_sha/license are null.
HSP01_PATH = "arXiv:2610.14224v1，Section 2.2.1与Appendix F"

EXPECTED_E_SCENARIOS = {
    "E00": "V001 V071 V072 V073 V080 V098 V113 V116 V123",
    "E01": "V010 V011 V012 V013 V084 V085 V096 V097 V103 V114 V118 V122",
    "E02": "V002 V003 V005 V009 V043 V044 V045 V046 V047 V048 V060 V062 V078 V079 V081 V082 V083 V084 V085 V086 V087 V088 V089 V090 V091 V092 V093 V094 V095 V096 V100 V101 V107 V108 V109 V120",
    "E03": "V004 V005 V006 V017 V051 V052 V054 V055 V056 V057 V058 V087 V088 V089 V090 V094 V096 V106 V108 V121",
    "E04": "V007 V008 V009 V038 V083 V085 V086 V087 V093 V094 V096 V101 V103 V106 V110 V111 V112",
    "E05": "V010 V011 V012 V013 V028 V048 V084 V085 V090 V091 V094 V097 V101 V104 V114",
    "E06": "V014 V015 V016 V017 V047 V049 V050 V059 V064 V070 V075 V077 V078 V084 V085 V089 V091 V094 V095 V096 V100 V108 V119",
    "E07": "V003 V010 V014 V015 V016 V042 V047 V059 V070 V077 V087 V088 V089 V091 V096 V097 V112 V114 V121 V122",
    "E08": "V017 V018 V038 V056 V069 V075 V081 V082 V083 V084 V085 V086 V090 V092 V094 V096 V099 V100 V102 V110 V111",
    "E09": "V019 V020 V022 V024 V025 V081 V086 V088 V089 V090 V091 V093 V094 V095 V096 V097 V100 V105 V106 V121 V122",
    "E10": "V020 V021 V022 V023 V025 V026 V027 V081 V086 V094 V096 V104 V105 V106",
    "E11": "V022 V026 V027 V028 V042 V081 V094 V096 V097 V104 V118",
    "E12": "V029 V030 V031 V034 V082 V083 V093 V094 V096 V109",
    "E13": "V016 V032 V037 V038 V082 V084 V087 V090 V091 V094 V096 V097 V100 V109 V121",
    "E14": "V015 V033 V034 V035 V036 V086 V090 V092 V094 V096 V097 V118 V119",
    "E15": "V026 V033 V035 V036 V042 V085 V092 V096 V097 V118",
    "E16": "V081 V082 V083 V084 V085 V086 V087 V088 V089 V090 V091 V092 V093 V094 V095 V096 V097 V098 V099 V100 V101 V102 V103 V104 V105 V106 V107 V108 V109 V110 V111 V112 V113 V114 V115 V116 V117 V118 V119 V120 V121 V122 V123",
    "E16.1": "V005 V006 V017 V051 V052 V053 V054 V055 V056 V076 V087 V090 V098 V117",
    "E16.2": "V017 V039 V066 V067 V068 V069 V070 V075 V091 V092 V098 V102 V117",
    "E16.3": "V014 V018 V038 V063 V064 V065 V078 V091 V092 V098 V115",
    "E16.4": "V002 V003 V009 V037 V039 V060 V061 V062 V077 V087 V091 V098 V101 V107 V108",
    "E16.5": "V007 V008 V017 V018 V037 V038 V039 V066 V069 V073 V075 V081 V082 V083 V084 V085 V086 V089 V090 V092 V094 V096 V098 V099 V101 V102 V104 V110 V113 V116 V120 V123",
    "E16.6": "V001 V039 V042 V071 V072 V073 V074 V080 V098 V116 V123",
    "E17": "V040",
    "E18": "V041",
}
EXPECTED_E_SCENARIOS = {key: set(value.split()) for key, value in EXPECTED_E_SCENARIOS.items()}
EXPECTED_E_IDS = set(EXPECTED_E_SCENARIOS)
EXPECTED_COMMITMENTS = {
    "B01": ("§5.2", "RH01 RH02", "E02 E06", "V044 V048 V079"), "B02": ("§5.3", "RH03", "E02", "V043 V045 V046 V078"),
    "B03": ("§5.4 §6.5", "RH04", "E02 E06 E07", "V047 V049 V050 V059 V077"), "B04": ("§5.5", "RH05", "E02 E16.4", "V060 V061 V062"),
    "B05": ("§6.3", "RH06", "E03", "V051 V052 V055 V056"), "B06": ("§6.4", "RH07", "E03 E14", "V054 V057 V058 V070"),
    "B07": ("§5.6 §6.3", "RH08", "E03 E16.1", "V052 V053 V054 V056 V076"), "B08": ("§11.1", "RH09", "E06 E16.3", "V063 V064 V065 V078"),
    "B09": ("§6.5 §9", "RH01 RH07", "E06 E14 E15", "V015 V033 V036 V070"), "B10": ("§11.2–§11.3", "RH01", "E08 E16.2 E16.6", "V066 V067 V068 V069 V073 V075"),
    "U01": ("§7.1 §7.5", "", "E02 E09 E10 E11", "V025 V028 V081"), "U02": ("§7.5.1–§7.5.3", "", "E10 E11", "V021 V023 V027 V081"),
    "U03": ("§8.1", "", "E12 E13", "V029 V034 V082"), "U04": ("§8.2–§8.4", "", "E04 E12", "V007 V030 V031 V083"),
    "U05": ("§3.2 §10", "", "E01 E05 E06", "V010 V011 V084"), "U06": ("§3.4.1 §6.6", "", "E01 E04 E05", "V012 V013 V085"),
    "U07": ("§7.1.1 §7.2.1", "", "E09", "V020 V024 V086"), "U08": ("§1.4 §12 §14.2", "", "E00 E05 E09 E10 E12 E16.6", "V042 V062 V080 V081 V082 V083 V084 V085 V086"),
    "K01": ("§6.4.1 §5.8", "SO01 SO06", "E02 E03 E04 E07 E13", "V087"), "K02": ("§6.7.1–§6.7.2", "SO02 SO03", "E03 E09", "V088"),
    "K03": ("§5.3.1 §5.8", "SO04 SO05", "E02 E03 E06 E09", "V089"), "K04": ("§6.7.3 §5.8 §11.5", "SO01 SO02", "E03 E05 E08 E09 E13", "V090 V094"),
    "K05": ("§11.5 §10.2", "SO01 SO07 SO10", "E05 E06 E09 E13", "V091"), "K06": ("§9.1 §3.1.1", "SO01 SO08", "E14 E15", "V092 V097"),
    "K07": ("§8.5 §7.4.1", "SO12 SO13", "E04 E09 E10 E12", "V093 V094"), "K08": ("§7.7", "SO11 SO14", "E06 E09", "V095"),
    "K09": ("§3.1.1 §3.6 §6.7.4", "SO13 SO15 SO16", "E01 E04 E07 E11 E15", "V096 V097"), "K10": ("§2.7–§2.8 §16.5–§16.6 §18.7", "SO09 SO10 SO18", "E00 E05 E06 E16", "V091 V098"),
}
EXPECTED_SO = {
    "SO01": ("skillopt/engine/trainer.py", "520a90eb8af422368c8e498b8487504bb8f6c279", "1–240、1270–2110", "E03 E05 E09 E13 E14"),
    "SO02": ("skillopt/gradient/reflect.py", "df68c9b76de83b2465b267b04310399ec5f3cec8", "300–700", "E03 E09"),
    "SO03": ("skillopt/gradient/aggregate.py", "14f0cd978f35afe579913c9b5677a803bbce9e70", "全文", "E03 E09"),
    "SO04": ("skillopt/optimizer/clip.py", "a2ed965f7c81c18426e82e2fd49e589b0952a92f", "全文", "E02 E03 E09"),
    "SO05": ("skillopt/optimizer/skill.py", "65d574157f47e9b3f415d4db94b0e5a9c09e4826", "全文", "E02 E03 E06 E09"),
    "SO06": ("skillopt/optimizer/skill_aware.py", "de39427ed0cc85fa1e716363a22140c91090b2d6", "全文", "E03 E07 E13"),
    "SO07": ("skillopt/optimizer/slow_update.py", "9f8dd52606c3ada4fea1e6c0f72ec38f8a5139e8", "1–260", "E09 E13"),
    "SO08": ("skillopt/optimizer/meta_skill.py", "6e34ff10d90655bb620d3e63b1d2041338616a5c", "全文", "E14 E15"),
    "SO09": ("skillopt/evaluation/gate.py", "10461a98756ebacbf7d5a8bbdc67657950e2c199", "全文", "E05 E06"),
    "SO10": ("configs/_base_/default.yaml", "4fe64088dc8011a2121335155d4a79ae7f90e9f5", "全文；本轮重读90–150", "E00 E03 E05 E13"),
    "SO11": ("skillopt_sleep/consolidate.py", "5b80b4a9825fb5b5a79b04fdad4c655d5f3228f6", "全文", "E05 E06 E09"),
    "SO12": ("skillopt_sleep/rollout.py", "f889333b3eecb83693a7005154dedadccbdc07f2", "全文", "E09 E12"),
    "SO13": ("skillopt_sleep/replay.py", "1502a71cf9069b8ea23293acbb080225cdc51de2", "全文", "E04 E09 E10 E12"),
    "SO14": ("skillopt_sleep/multi_skill.py", "32247639849e2c327ab7a1b7b3258dc7c35f53d4", "全文", "E06 E09"),
    "SO15": ("skillopt_sleep/budget.py", "48875ca0f01cf099967ca2b5a45891e903645e36", "全文", "E01 E04 E11"),
    "SO16": ("docs/sleep/RESULTS.md", "441ffa28f8644631c275456c7ccf4e75fd1d8c99", "全文", "E01 E07 E15"),
    "SO17": ("docs/guide/training-loop.md", "30393dca9533de90e0cd0ff95f26eb327bfec0ff", "全文", "E03 E09"),
    "SO18": ("LICENSE", "cf7bcb2ba475e33648180599c0f83c425f1705d9", "全文；本轮重读及固定字节校验", "E00 E16.2 E16.5"),
}

class CheckerError(Exception):
    pass

def sha256_of_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()

def no_duplicate_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise CheckerError(f"duplicate JSON object key: {key}")
        result[key] = value
    return result

def load_json_strict(raw: str) -> Any:
    try:
        return json.loads(raw, object_pairs_hook=no_duplicate_object)
    except CheckerError:
        raise
    except (json.JSONDecodeError, TypeError) as exc:
        raise CheckerError(f"invalid JSON: {exc}") from exc

def safe_relative(value: Any, what: str) -> str:
    if not isinstance(value, str) or not value:
        raise CheckerError(f"{what}: path must be a non-empty string")
    if value.startswith(("/", "\\")) or re.match(r"^[A-Za-z]:", value) or "\\" in value:
        raise CheckerError(f"{what}: absolute/drive/backslash path is forbidden: {value!r}")
    if any(part in ("", ".", "..") for part in value.split("/")):
        raise CheckerError(f"{what}: path has traversal or non-normal segment: {value!r}")
    return value

def inspect_relative(repo_root: Path, value: Any, what: str, required: bool) -> Path | None:
    rel = safe_relative(value, what)
    current = repo_root
    for part in rel.split("/"):
        current = current / part
        if current.is_symlink():
            raise CheckerError(f"{what}: path contains a symlink component: {rel!r}")
        if not current.exists():
            if required:
                raise CheckerError(f"{what}: path does not exist: {rel!r}")
            return None
    if not current.is_file():
        raise CheckerError(f"{what}: path is not a regular file: {rel!r}")
    try:
        current.resolve(strict=True).relative_to(repo_root.resolve(strict=True))
    except (OSError, ValueError) as exc:
        raise CheckerError(f"{what}: path escapes repository: {rel!r}") from exc
    return current


def validate_project_license_metadata(repo_root: Path) -> dict[str, Any]:
    """Read the closed project-license contract; this never grants release rights."""
    def read_text(relative: str) -> str:
        path = inspect_relative(repo_root, relative, "project license " + relative, True)
        try:
            return path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as exc:
            raise CheckerError(f"project license cannot read {relative}: {exc}") from exc

    try:
        cargo = tomllib.loads(read_text("Cargo.toml"))
        workspace = cargo.get("workspace")
        if not isinstance(workspace, dict):
            raise CheckerError("project license workspace must be a table")
        members = workspace.get("members")
        if not isinstance(members, list) or any(not isinstance(x, str) for x in members) or sorted(members) != PROJECT_LICENSE_MEMBERS:
            raise CheckerError("project license requires the exact six workspace members")
        package = workspace.get("package")
        if not isinstance(package, dict) or package.get("publish") is not False or package.get("license-file") != "LICENSE" or "license" in package:
            raise CheckerError("project license root must use LICENSE and publish=false")
        for member in members:
            package = tomllib.loads(read_text(f"{member}/Cargo.toml")).get("package")
            if not isinstance(package, dict) or "license" in package:
                raise CheckerError(f"project license invalid package {member}")
            for field in ("license-file", "publish"):
                inherited = package.get(field)
                if not isinstance(inherited, dict) or set(inherited) != {"workspace"} or inherited["workspace"] is not True:
                    raise CheckerError(f"project license {member}.{field} must inherit workspace=true")
    except tomllib.TOMLDecodeError as exc:
        raise CheckerError(f"project license invalid Cargo TOML: {exc}") from exc
    path = inspect_relative(repo_root, "LICENSE", "project LICENSE", True)
    try:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as exc:
        raise CheckerError(f"project LICENSE unreadable: {exc}") from exc
    if digest != PROJECT_LICENSE_SHA256:
        raise CheckerError("project LICENSE is not the reviewed license text")
    readme = read_text("README.md").replace("\r\n", "\n")
    sections = list(re.finditer(r"^## License[ \t]*$", readme, flags=re.MULTILINE))
    if len(sections) != 1:
        raise CheckerError("project license requires one README License section")
    tail = readme[sections[0].end():]
    # The reviewed License section is final; later headings cannot hide grants.
    if tail.strip() != PROJECT_LICENSE_README:
        raise CheckerError("project license README section differs from the reviewed text")
    return {"schema_version": "rsia.project_license_metadata.v1", "status": "source_available_personal_noncommercial_free", "path": "LICENSE", "sha256": digest, "members": sorted(members)}

def validate_project_license(manifest: Any, repo_root: Path) -> None:
    if not isinstance(manifest, dict):
        raise CheckerError("project license manifest must be an object")
    license = manifest.get("project_license")
    if not isinstance(license, dict) or set(license) != {"status", "path", "sha256"}:
        raise CheckerError("project_license must contain only status/path/sha256")
    if license["status"] != "source_available_personal_noncommercial_free" or license["path"] != "LICENSE" or not isinstance(license["sha256"], str) or not HEX64.fullmatch(license["sha256"]):
        raise CheckerError("project_license fields are invalid")
    actual = validate_project_license_metadata(repo_root)
    if any(license[field] != actual[field] for field in license):
        raise CheckerError("project_license does not match actual repository metadata")

def string_set(value: Any, what: str) -> set[str]:
    if not isinstance(value, list) or any(not isinstance(item, str) or not item for item in value):
        raise CheckerError(f"{what}: must be an array of non-empty strings")
    if len(value) != len(set(value)):
        raise CheckerError(f"{what}: duplicate id")
    return set(value)

def nonempty(value: Any, what: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise CheckerError(f"{what}: must be a non-empty string")
    return value

def known_keys(value: Any, allowed: set[str], what: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise CheckerError(f"{what}: must be an object")
    unknown = sorted(set(value) - allowed)
    if unknown:
        raise CheckerError(f"{what}: unknown key(s) {unknown} are not part of the {SCHEMA_VERSION} schema and cannot carry claims")
    return value

def workspace_packages(repo_root: Path) -> dict[str, tuple[Path, dict[str, Any]]]:
    root_manifest = inspect_relative(repo_root, "Cargo.toml", "workspace Cargo.toml", True)
    try:
        workspace = tomllib.loads(root_manifest.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise CheckerError(f"workspace Cargo.toml is invalid: {exc}") from exc
    table = workspace.get("workspace")
    if not isinstance(table, dict):
        raise CheckerError("workspace Cargo.toml has no workspace table")
    members = table.get("members")
    if not isinstance(members, list) or not members:
        raise CheckerError("workspace Cargo.toml has no explicit members")
    result = {}
    seen_members = set()
    for member in members:
        rel = safe_relative(member, "workspace member")
        if rel in seen_members:
            raise CheckerError("workspace Cargo.toml has a duplicate member")
        seen_members.add(rel)
        if any(char in rel for char in "*?["):
            raise CheckerError("workspace glob members are unsupported by this finite checker")
        cargo = inspect_relative(repo_root, f"{rel}/Cargo.toml", "member Cargo.toml", True)
        try:
            data = tomllib.loads(cargo.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as exc:
            raise CheckerError(f"invalid member Cargo.toml {rel}: {exc}") from exc
        package = data.get("package")
        if not isinstance(package, dict):
            raise CheckerError(f"member Cargo.toml {rel} has no package table")
        name = package.get("name")
        if not isinstance(name, str) or not IDENT_RE.fullmatch(name) or name in result:
            raise CheckerError(f"invalid or duplicate package name in {rel}")
        result[name] = (Path(rel), data)
    return result

def resolve_cargo_target(repo_root: Path, command: Any, packages: dict[str, tuple[Path, dict[str, Any]]]) -> str:
    if not isinstance(command, str):
        raise CheckerError("evidence command must be a string")
    tokens = command.split()
    if len(tokens) != 8 or tokens[:5] != ["cargo", "test", "--locked", "--offline", "-p"]:
        raise CheckerError("command is outside the finite single-target cargo test grammar")
    package, mode, target = tokens[5], tokens[6], tokens[7]
    if package not in packages:
        raise CheckerError(f"command names undeclared workspace package {package!r}")
    member, cargo = packages[package]
    if mode == "--test":
        if not IDENT_RE.fullmatch(target):
            raise CheckerError("integration target name is invalid")
        explicit = {item.get("name"): item.get("path", f"tests/{item.get('name')}.rs") for item in cargo.get("test", []) if isinstance(item, dict)}
        if cargo.get("package", {}).get("autotests", True):
            rel = explicit.get(target, f"tests/{target}.rs")
        elif target in explicit:
            rel = explicit[target]
        else:
            raise CheckerError(f"integration target {target!r} is not declared")
        full = (member / safe_relative(rel, "test target path")).as_posix()
    elif mode == "--lib":
        if not target.endswith("::tests"):
            raise CheckerError("--lib filter must end in ::tests")
        module = target[:-7]
        if not module or any(not IDENT_RE.fullmatch(part) for part in module.split("::")):
            raise CheckerError("--lib module filter is invalid")
        lib_rel = cargo.get("lib", {}).get("path", "src/lib.rs")
        lib_path = (member / safe_relative(lib_rel, "lib target path")).as_posix()
        lib_file = inspect_relative(repo_root, lib_path, "crate lib target", True)
        first = module.split("::", 1)[0]
        if f"mod {first};" not in lib_file.read_text(encoding="utf-8"):
            raise CheckerError(f"module {first!r} is not declared by {package}")
        full = (member / "src" / (module.replace("::", "/") + ".rs")).as_posix()
    else:
        raise CheckerError("command must contain exactly one --test or --lib target")
    inspect_relative(repo_root, full, "cargo test target", True)
    return full

def validate_field_semantics(value: Any) -> None:
    """The manifest must state, machine-readably, what the execution-looking fields mean (plan E16.6: derived JSON may not be a second truth)."""
    if not isinstance(value, dict):
        raise CheckerError("field_semantics: must be an object stating what command, exit_code, log_path, execution and production_consumer mean")
    known_keys(value, set(FIELD_SEMANTICS), "field_semantics")
    for key, expected in FIELD_SEMANTICS.items():
        if key not in value:
            raise CheckerError(f"field_semantics: missing the definition of {key!r}")
        if value[key] != expected:
            raise CheckerError(f"field_semantics.{key} differs from the definition pinned by the checker; a machine field may not be redefined to read as stronger evidence than it is")

def validate_execution(record: dict[str, Any], identifier: str, pr_number: int) -> None:
    """Record-level `execution`: what was literally run, kept apart from the declared single target in command/exit_code/log_path."""
    if "execution" not in record:
        if pr_number > EXECUTION_REQUIRED_ABOVE_PR:
            raise CheckerError(
                f"{identifier}: PR #{pr_number} is above #{EXECUTION_REQUIRED_ABOVE_PR}, so the record must carry an 'execution' object "
                "(runner, literally executed command, scope, real exit code, evidence); command/exit_code/log_path only describe the declared target"
            )
        return
    what = f"{identifier} execution"
    execution = known_keys(record["execution"], EXECUTION_KEYS, what)
    missing = sorted(EXECUTION_KEYS - set(execution))
    if missing:
        raise CheckerError(f"{what}: missing key(s) {missing}")
    runner = execution["runner"]
    if not isinstance(runner, str) or runner not in EXECUTION_RUNNERS:
        raise CheckerError(f"{what}: runner must be one of {sorted(EXECUTION_RUNNERS)}, got {runner!r}")
    scope = execution["scope"]
    if not isinstance(scope, str) or scope not in EXECUTION_SCOPES:
        raise CheckerError(f"{what}: scope must be one of {sorted(EXECUTION_SCOPES)}, got {scope!r}")
    command = nonempty(execution["command"], f"{what} command")
    if " ".join(command.split()).startswith(DECLARED_COMMAND_PREFIX):
        raise CheckerError(f"{what}: command must be the literally executed command, not the declared single-target form ({DECLARED_COMMAND_PREFIX} ...) copied again")
    exit_code = execution["exit_code"]
    if type(exit_code) is not int:
        raise CheckerError(f"{what}: exit_code must be the integer exit code of the executed command, got {exit_code!r}")
    nonempty(execution["evidence"], f"{what} evidence")

def validate_production_consumer(record: dict[str, Any], identifier: str) -> None:
    """Optional marker separating modules with a production caller from library-only ones; an absent marker claims nothing."""
    if "production_consumer" not in record:
        return
    value = record["production_consumer"]
    if not isinstance(value, str) or value not in PRODUCTION_CONSUMER_VALUES:
        raise CheckerError(f"{identifier}: production_consumer must be one of {sorted(PRODUCTION_CONSUMER_VALUES)} when present, got {value!r}")

def expected_v_to_e() -> dict[str, set[str]]:
    # Derived from the per-E lists above (plan §12 V-reference verbs, v4.5 Q-49: "新增"/"补充断言" count, "涉及" does not).
    result = {f"V{i:03}": set() for i in range(1, V_SCENARIO_COUNT + 1)}
    for task, scenarios in EXPECTED_E_SCENARIOS.items():
        for scenario in scenarios:
            result[scenario].add(task)
    return result

def evidence_expected_refs(e_tasks: set[str], evidence_by_task: dict[str, set[str]]) -> set[str]:
    return {item for task in e_tasks for item in evidence_by_task[task]}

def _rust_code_mask(source: str) -> str:
    """Keep code positions; comments/literals cannot impersonate the fact function."""
    masked = list(source)
    raw_literal = re.compile(r'(?:br|r)(#*)"')
    char_literal = re.compile(r"'(?:[^\\'\n]|\\(?:[nrt0\\'\"]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}))'")
    index = 0
    while index < len(source):
        end = index
        if source.startswith("//", index):
            end = source.find("\n", index)
            end = len(source) if end < 0 else end
        elif source.startswith("/*", index):
            end, depth = index + 2, 1
            while depth and end < len(source):
                if source.startswith("/*", end):
                    depth, end = depth + 1, end + 2
                elif source.startswith("*/", end):
                    depth, end = depth - 1, end + 2
                else:
                    end += 1
            if depth:
                raise CheckerError("reviewed merge facts contain an unterminated Rust comment")
        else:
            raw = raw_literal.match(source, index) if index == 0 or not (source[index - 1].isalnum() or source[index - 1] == "_") else None
            if raw:
                close = '"' + raw.group(1)
                close_at = source.find(close, raw.end())
                if close_at < 0:
                    raise CheckerError("reviewed merge facts contain an unterminated Rust raw string")
                end = close_at + len(close)
            elif source[index] == '"':
                end = index + 1
                while end < len(source) and source[end] != '"':
                    end += 2 if source[end] == "\\" else 1
                if end >= len(source):
                    raise CheckerError("reviewed merge facts contain an unterminated Rust string")
                end += 1
            elif source[index] == "'":
                char = char_literal.match(source, index)
                if char:
                    end = char.end()
        if end > index:
            for position in range(index, end):
                if masked[position] != "\n":
                    masked[position] = " "
            index = end
        else:
            index += 1
    return "".join(masked)

def reviewed_merge_facts(repo_root: Path) -> dict[int, str | None]:
    """Read the Rust checker's single finite fact table, without executing Rust."""
    path = inspect_relative(repo_root, "crates/evo-core/tests/support_scope.rs", "reviewed merge facts", True)
    try:
        source = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise CheckerError(f"reviewed merge facts are unreadable: {exc}") from exc
    # Any second declaration is ambiguous, even if it would be cfg-disabled.
    code = _rust_code_mask(source)
    declarations = list(re.finditer(r"\bfn\s+(?:r#)?expected_merge_for_pr\b", code))
    if len(declarations) != 1:
        raise CheckerError("reviewed merge facts require exactly one expected_merge_for_pr function")
    start = declarations[0].start()
    # A semicolon inside any token tree is not a top-level item boundary.
    stack, item_start = [], 0
    closers = {"}": "{", ")": "(", "]": "["}
    for position, token in enumerate(code):
        if position == start:
            if stack:
                raise CheckerError("reviewed merge facts require the top-level Rust function")
            if code[item_start:start].strip():
                raise CheckerError("reviewed merge facts require an ordinary private synchronous function without modifiers")
        if token in "{([":
            stack.append(token)
        elif token in "})]":
            if not stack or stack.pop() != closers[token]:
                raise CheckerError("reviewed merge facts have mismatched delimiters")
            if not stack and token == "}":
                item_start = position + 1
        elif token == ";" and not stack:
            item_start = position + 1
    if stack:
        raise CheckerError("reviewed merge facts have unclosed delimiters")
    function = re.match(
        r"(?ms)^fn\s+expected_merge_for_pr\s*\(\s*pr\s*:\s*u64\s*\)\s*"
        r"->\s*Result\s*<\s*Option\s*<\s*&'static\s+str\s*>\s*,\s*String\s*>\s*\{(.*?)^\}",
        source[start:],
    )
    body = re.fullmatch(
        r'\s*Ok\s*\(\s*match\s+pr\s*\{(.*?)'
        r'\s*_\s*=>\s*return\s+Err\s*\(\s*format!\s*\(\s*"no reviewed merge status for PR \{pr\}"\s*\)\s*\)\s*,\s*\}\s*\)\s*',
        function.group(1) if function else "",
        re.S,
    )
    if body is None:
        raise CheckerError("reviewed merge facts contain an unsupported function or fallback")
    facts: dict[int, str | None] = {}
    for line in body.group(1).splitlines():
        if not line.strip():
            continue
        arm = re.fullmatch(r'\s*(0|[1-9][0-9]*)\s*=>\s*(?:Some\("([0-9a-f]{40})"\)|(None))\s*,\s*', line)
        if arm is None:
            raise CheckerError("reviewed merge facts contain an unsupported literal arm")
        number = int(arm.group(1))
        if number > 2**64 - 1 or number in facts:
            raise CheckerError("reviewed merge facts contain an out-of-range or duplicate PR")
        facts[number] = arm.group(2)
    if not facts:
        raise CheckerError("reviewed merge facts are empty")
    return facts

def validate_evidence(record: Any, task: str, repo_root: Path, packages: dict[str, tuple[Path, dict[str, Any]]], implementation_files: list[str]) -> str:
    if not isinstance(record, dict):
        raise CheckerError(f"{task}: evidence record is not an object")
    identifier = nonempty(record.get("id"), f"{task} evidence id")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", identifier):
        raise CheckerError(f"{task}: evidence id is invalid")
    known_keys(record, EVIDENCE_KEYS, f"{task} evidence {identifier}")
    owner = max((tid for tid in EXPECTED_E_IDS if identifier.startswith(tid + ".")), key=len, default=None)
    if owner != task:
        raise CheckerError(f"{task}: evidence id {identifier!r} is not namespaced under its own task; evidence must be registered as '{task}.<name>'")
    version, digest = record.get("plan_version"), record.get("plan_sha256")
    lineage = PLAN_LINEAGE.get(version) if isinstance(version, str) else None
    if lineage is None:
        raise CheckerError(f"{identifier}: plan_version {version!r} is outside the registered plan lineage {list(PLAN_LINEAGE)}")
    if digest != lineage["sha256"]:
        raise CheckerError(f"{identifier}: plan_sha256 {digest!r} is not the registered digest of plan {version}; digests outside the lineage or paired with the wrong version are rejected")
    order = list(PLAN_LINEAGE)
    recheck = record["plan_recheck"] if "plan_recheck" in record else "not_affected"
    if not isinstance(recheck, str) or recheck not in PLAN_RECHECK_VALUES:
        raise CheckerError(f"{identifier}: plan_recheck must be one of {sorted(PLAN_RECHECK_VALUES)}, got {recheck!r}")
    if recheck == "required":
        raise CheckerError(f"{identifier}: plan_recheck is 'required'; the record was verified against plan {version} in a scope revised by {PLAN_VERSION} and has not been rechecked, so the manifest cannot pass until it is rechecked or withdrawn")
    if recheck.startswith("rechecked_"):
        if order.index(recheck[len("rechecked_"):]) <= order.index(version):
            raise CheckerError(f"{identifier}: plan_recheck {recheck!r} does not name a plan later than the record's own plan_version {version!r}; a record is only rechecked against a later plan")
    if version == PLAN_VERSION:
        if "plan_recheck" not in record or recheck != "not_affected":
            marker = repr(record["plan_recheck"]) if "plan_recheck" in record else "no marker"
            raise CheckerError(f"{identifier}: a record bound to the current plan {PLAN_VERSION} must carry plan_recheck 'not_affected' explicitly (plan §18.5), got {marker}")
    elif task in PLAN_RECHECK_TASKS:
        if recheck != PLAN_RECHECKED:
            raise CheckerError(f"{identifier}: task {task} is touched by the {PLAN_VERSION} revision; a record still bound to plan {version} must carry plan_recheck {PLAN_RECHECKED!r}, got {recheck!r}")
    elif recheck == PLAN_RECHECKED:
        raise CheckerError(f"{identifier}: plan_recheck {PLAN_RECHECKED!r} is valid only inside the {PLAN_VERSION} recheck union; task {task} is outside it, so the record keeps its historical marker")
    elif version in V43_RECHECK_FROM and task in V43_RECHECK_TASKS:
        if recheck != V43_RECHECKED:
            raise CheckerError(f"{identifier}: task {task} is outside the {PLAN_VERSION} recheck union but was rechecked by the v4.3 revision; a record bound to plan {version} keeps plan_recheck {V43_RECHECKED!r}, got {recheck!r}")
    elif recheck != "not_affected":
        raise CheckerError(f"{identifier}: no registered recheck applies to a plan {version} record of task {task}; it keeps plan_recheck 'not_affected', got {recheck!r}")
    for field in ("supersedes", "completes"):
        if field in record:
            ids = record[field]
            if not isinstance(ids, list) or any(not isinstance(item, str) or not re.fullmatch(r"[A-Za-z0-9_.-]+", item) for item in ids):
                raise CheckerError(f"{identifier}: {field} must be an array of evidence record ids")
            if len(ids) != len(set(ids)):
                raise CheckerError(f"{identifier}: {field} lists an id twice")
            if identifier in ids:
                raise CheckerError(f"{identifier}: {field} names the record itself")
    if set(record.get("supersedes", [])) & set(record.get("completes", [])):
        raise CheckerError(f"{identifier}: an id cannot be both superseded and completed by the same record")
    if ("supersedes" in record or "completes" in record) and order.index(version) < order.index(RELATION_MIN_VERSION):
        raise CheckerError(f"{identifier}: supersedes and completes exist since plan {RELATION_MIN_VERSION}; a record carrying them must be bound to plan {RELATION_MIN_VERSION} or later, got {version!r}")
    if "note" in record:
        nonempty(record["note"], f"{identifier} note")
    source_sha = record.get("source_sha")
    if not isinstance(source_sha, str) or not HEX40.fullmatch(source_sha):
        raise CheckerError(f"{identifier}: source_sha must be lowercase 40-hex")
    pr = record.get("pr")
    if not isinstance(pr, str) or PR_RE.fullmatch(pr) is None:
        raise CheckerError(f"{identifier}: PR URL is invalid")
    merged = record.get("merged_sha")
    if merged is not None and (not isinstance(merged, str) or not HEX40.fullmatch(merged)):
        raise CheckerError(f"{identifier}: merged_sha must be null or lowercase 40-hex")
    if merged is not None and merged == source_sha:
        raise CheckerError(f"{identifier}: merged_sha must differ from its own source_sha")
    pr_state = record.get("pr_state")
    if pr_state is not None and pr_state not in PR_STATES:
        raise CheckerError(f"{identifier}: pr_state must be one of {sorted(PR_STATES)}")
    if pr_state == "merged" and merged is None:
        raise CheckerError(f"{identifier}: pr_state is 'merged' but merged_sha is null")
    if pr_state not in (None, "merged") and merged is not None:
        raise CheckerError(f"{identifier}: merged_sha {merged} is set but the record's pr_state is {pr_state!r}, not 'merged'")
    digest = record.get("input_digest")
    if not isinstance(digest, str) or not HEX64.fullmatch(digest):
        raise CheckerError(f"{identifier}: input_digest must be lowercase SHA-256")
    for field in ("input_ref", "log_path", "record_source"):
        safe_relative(record.get(field), f"{identifier} {field}")
    test_entry = safe_relative(record.get("test_entry"), f"{identifier} test_entry")
    inspect_relative(repo_root, test_entry, f"{identifier} test_entry", True)
    command = nonempty(record.get("command"), f"{identifier} command")
    if resolve_cargo_target(repo_root, command, packages) != test_entry:
        raise CheckerError(f"{identifier}: command target differs from test_entry")
    package = command.split()[5]
    member = packages[package][0].as_posix()
    if not any(file == test_entry or file.startswith(member + "/") for file in implementation_files):
        raise CheckerError(f"{identifier}: test_entry {test_entry!r} (package {package}) is not among {task}'s declared implementation_files nor inside a crate they declare")
    exit_code = record.get("exit_code")
    if type(exit_code) is not int or exit_code != 0:
        raise CheckerError(f"{identifier}: verified execution must have integer exit_code 0")
    for field in ("description", "actual_result", "scope", "risk", "rollback"):
        nonempty(record.get(field), f"{identifier} {field}")
    validate_execution(record, identifier, int(PR_RE.fullmatch(pr).group(1)))
    validate_production_consumer(record, identifier)
    return identifier

def validate_refs(refs: Any, what: str, evidence_owner: dict[str, str], relevant_tasks: set[str]) -> set[str]:
    """Every ref must be a unique, existing evidence record id (V, B/U/K and SO refs all pass through here) owned by a task mapped to the entry."""
    values = string_set(refs, what)
    unknown = values - set(evidence_owner)
    if unknown:
        raise CheckerError(f"{what}: unknown evidence refs {sorted(unknown)}")
    irrelevant = {ref for ref in values if evidence_owner[ref] not in relevant_tasks}
    if irrelevant:
        raise CheckerError(f"{what}: evidence refs are outside the mapped E scope: {sorted(irrelevant)}")
    return values

def validate_plan_lineage(value: Any) -> None:
    if not isinstance(value, list) or not value:
        raise CheckerError("plan_lineage must be a non-empty array of {version, file, sha256} entries")
    expected = [(version, entry["file"], entry["sha256"]) for version, entry in PLAN_LINEAGE.items()]
    actual = []
    for index, item in enumerate(value):
        known_keys(item, LINEAGE_KEYS, f"plan_lineage[{index}]")
        version = nonempty(item.get("version"), f"plan_lineage[{index}] version")
        file = nonempty(item.get("file"), f"plan_lineage[{index}] file")
        digest = item.get("sha256")
        if not isinstance(digest, str) or not HEX64.fullmatch(digest):
            raise CheckerError(f"plan_lineage[{index}] sha256 must be lowercase SHA-256")
        actual.append((version, file, digest))
    if actual != expected:
        raise CheckerError(f"plan_lineage must list exactly the registered lineage {' -> '.join(PLAN_LINEAGE)} in order with the registered file names and SHA-256 digests")
    if actual[-1][0] != PLAN_VERSION or actual[-1][2] != PLAN_SHA256:
        raise CheckerError(f"plan_lineage must end with the current plan {PLAN_VERSION} ({PLAN_SHA256})")

def task_of(identifier: str) -> str | None:
    """Owner task of an evidence id by the longest registered E-task prefix (E16.5.x belongs to E16.5, not E16)."""
    return max((tid for tid in EXPECTED_E_IDS if identifier.startswith(tid + ".")), key=len, default=None)

def validate_withdrawn_records(value: Any, evidence_records: list[dict[str, Any]], evidence_owner: dict[str, str], reviewed: dict[int, str | None]) -> dict[str, dict[str, Any]]:
    """plan §18.10 (v4.5 Q-50): the reviewed withdrawn list; withdrawn ids never reappear; supersedes/completes point at the right records."""
    if not isinstance(value, list):
        raise CheckerError("withdrawn_records must be an array of {id, withdrawn_in_pr, clause, owner_task, status} entries")
    withdrawn: dict[str, dict[str, Any]] = {}
    for index, entry in enumerate(value):
        what = f"withdrawn_records[{index}]"
        known_keys(entry, WITHDRAWN_KEYS, what)
        missing = sorted(WITHDRAWN_KEYS - set(entry))
        if missing:
            raise CheckerError(f"{what}: missing key(s) {missing}")
        identifier = nonempty(entry["id"], f"{what} id")
        if not re.fullmatch(r"[A-Za-z0-9_.-]+", identifier) or task_of(identifier) is None:
            raise CheckerError(f"{what}: id {identifier!r} is not an evidence id namespaced under a registered E task")
        if identifier in withdrawn:
            raise CheckerError(f"withdrawn_records: duplicate id {identifier}")
        pr = entry["withdrawn_in_pr"]
        match = PR_RE.fullmatch(pr) if isinstance(pr, str) else None
        if match is None:
            raise CheckerError(f"{identifier}: withdrawn_in_pr must be a pull request URL")
        if int(match.group(1)) not in reviewed:
            raise CheckerError(f"{identifier}: withdrawn_in_pr names PR {match.group(1)}, which has no reviewed merge status")
        clause = nonempty(entry["clause"], f"{identifier} clause")
        if not clause.startswith("§"):
            raise CheckerError(f"{identifier}: clause must cite the plan section (§...) of the changed behavior")
        owner = entry["owner_task"]
        if not isinstance(owner, str) or OWNER_TASK_RE.fullmatch(owner) is None:
            raise CheckerError(f"{identifier}: owner_task must be a registered task number such as AG-187, got {owner!r}")
        status = entry["status"]
        if not isinstance(status, str) or status not in WITHDRAWN_STATUSES:
            raise CheckerError(f"{identifier}: withdrawn status must be one of {sorted(WITHDRAWN_STATUSES)}, got {status!r}")
        withdrawn[identifier] = entry
    expected_ids = set(EXPECTED_WITHDRAWN)
    if set(withdrawn) != expected_ids:
        raise CheckerError(
            f"withdrawn_records differ from the reviewed withdrawn list: missing={sorted(expected_ids - set(withdrawn))} "
            f"extra={sorted(set(withdrawn) - expected_ids)}; an entry is never dropped and a new withdrawal is reviewed in both checkers"
        )
    for identifier, entry in withdrawn.items():
        actual = (int(PR_RE.fullmatch(entry["withdrawn_in_pr"]).group(1)), entry["clause"], entry["owner_task"], entry["status"])
        if actual != EXPECTED_WITHDRAWN[identifier]:
            raise CheckerError(f"{identifier}: withdrawn entry (withdrawn_in_pr, clause, owner_task, status) {actual} differs from the reviewed tuple {EXPECTED_WITHDRAWN[identifier]}; withdrawn entries are pinned in both checkers")
    revived = sorted(set(withdrawn) & set(evidence_owner))
    if revived:
        raise CheckerError(f"withdrawn record(s) {revived} reappear in verified_subscopes; a withdrawn record is never revived (re-verify it with a new record that supersedes it)")
    superseded_by: dict[str, list[str]] = {}
    pairs: set[tuple[str, str, str]] = set()
    for record in evidence_records:
        identifier = record["id"]
        for old in record.get("supersedes", []):
            entry = withdrawn.get(old)
            if entry is None:
                raise CheckerError(f"{identifier}: supersedes {old!r}, which is not a withdrawn record; only withdrawn records can be superseded")
            if entry["status"] != "superseded":
                raise CheckerError(f"{identifier}: supersedes {old!r}, but its withdrawn status is {entry['status']!r}, not 'superseded'")
            if task_of(old) != evidence_owner[identifier]:
                raise CheckerError(f"{identifier}: supersedes {old!r} of task {task_of(old)}; the re-verification belongs to the withdrawn record's own E task")
            superseded_by.setdefault(old, []).append(identifier)
            pairs.add(("supersedes", identifier, old))
        for kept in record.get("completes", []):
            if kept in withdrawn:
                raise CheckerError(f"{identifier}: completes the withdrawn record {kept!r}; a withdrawn record can only be superseded")
            if kept not in evidence_owner:
                raise CheckerError(f"{identifier}: completes {kept!r}, which is not a current verified record")
            if evidence_owner[kept] != evidence_owner[identifier]:
                raise CheckerError(f"{identifier}: completes {kept!r} of task {evidence_owner[kept]}; a re-run belongs to the completed record's own E task")
            pairs.add(("completes", identifier, kept))
    for identifier, entry in withdrawn.items():
        if entry["status"] == "superseded" and identifier not in superseded_by:
            raise CheckerError(f"{identifier}: withdrawn status is 'superseded' but no verified record supersedes it")
    if pairs != EXPECTED_RELATIONS:
        raise CheckerError(
            f"supersedes/completes pairs differ from the reviewed relation pairs: missing={sorted(EXPECTED_RELATIONS - pairs)} "
            f"extra={sorted(pairs - EXPECTED_RELATIONS)}; a relation pair is reviewed in both checkers"
        )
    return withdrawn

def record_digest(record: dict[str, Any]) -> str:
    """SHA-256 of a record's canonical JSON without plan_recheck: keys sorted, no insignificant whitespace, UTF-8."""
    body = {key: value for key, value in record.items() if key != "plan_recheck"}
    return hashlib.sha256(json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")).hexdigest()

def validate_record_pins(evidence_records: list[dict[str, Any]]) -> None:
    """plan §18.10 / V080.d: the historical records retained from the previous index binding are never rewritten."""
    by_id = {record["id"]: record for record in evidence_records}
    for identifier, expected in EXPECTED_RECORD_DIGESTS.items():
        record = by_id.get(identifier)
        if record is None:
            raise CheckerError(f"{identifier}: pinned historical record is missing from verified_subscopes; a historical record leaves the index only through a reviewed withdrawal")
        if record_digest(record) != expected:
            raise CheckerError(f"{identifier}: historical record differs from its pinned digest (canonical JSON without plan_recheck); a historical record is never rewritten")

def _missing_identity(entry: dict[str, Any], what: str) -> tuple[int | None, str | None]:
    size, digest = entry["expected_bytes"], entry["expected_sha256"]
    if size != UNKNOWN and (type(size) is not int or size < 0):
        raise CheckerError(f"{what}: expected_bytes must be a non-negative integer or 'unknown'")
    if digest != UNKNOWN and (not isinstance(digest, str) or not HEX64.fullmatch(digest)):
        raise CheckerError(f"{what}: expected_sha256 must be lowercase SHA-256 or 'unknown'")
    incomplete = size == UNKNOWN or digest == UNKNOWN
    if incomplete:
        nonempty(entry.get("unknown_reason"), f"{what} unknown_reason")
    elif "unknown_reason" in entry:
        raise CheckerError(f"{what}: unknown_reason is only allowed when the expected bytes or SHA-256 are unknown")
    return (None if size == UNKNOWN else size), (None if digest == UNKNOWN else digest)

def _missing_window(value: Any, what: str) -> None:
    if value == UNKNOWN:
        return
    window = known_keys(value, MISSING_WINDOW_KEYS, f"{what} missing_window")
    if set(window) != MISSING_WINDOW_KEYS:
        raise CheckerError(f"{what}: missing_window must be 'unknown' or contain exactly after and before")
    for bound in ("after", "before"):
        item = window[bound]
        if item != UNKNOWN and (not isinstance(item, str) or not ISO_UTC.fullmatch(item)):
            raise CheckerError(f"{what}: missing_window.{bound} must be an ISO-8601 UTC timestamp or 'unknown'")

def validate_missing_register(manifest: dict[str, Any], repo_root: Path, rel: str, records: dict[str, dict[str, Any]]) -> dict[tuple[str, str], dict[str, Any]]:
    """plan §18.10 (v4.5 Q-50): the missing-evidence register is pinned by path and SHA-256 and names real record fields."""
    pointer = known_keys(manifest.get("evidence_missing_register"), REGISTER_POINTER_KEYS, "evidence_missing_register")
    if set(pointer) != REGISTER_POINTER_KEYS:
        raise CheckerError("evidence_missing_register must contain exactly path and sha256")
    if pointer["path"] != rel:
        raise CheckerError(f"evidence_missing_register.path {pointer['path']!r} differs from the register in use ({rel!r}, --known-missing)")
    if not isinstance(pointer["sha256"], str) or not HEX64.fullmatch(pointer["sha256"]):
        raise CheckerError("evidence_missing_register.sha256 must be lowercase SHA-256")
    path = inspect_relative(repo_root, rel, "missing-evidence register", True)
    if sha256_of_file(path) != pointer["sha256"]:
        raise CheckerError(f"missing-evidence register {rel} SHA-256 differs from evidence_missing_register.sha256; the register cannot change without the manifest pin")
    try:
        register = load_json_strict(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError) as exc:
        raise CheckerError(f"missing-evidence register is unreadable: {exc}") from exc
    known_keys(register, MISSING_REGISTER_KEYS, "missing-evidence register")
    if register.get("schema_version") != MISSING_SCHEMA_VERSION:
        raise CheckerError(f"missing-evidence register schema_version must be {MISSING_SCHEMA_VERSION}")
    nonempty(register.get("basis"), "missing-evidence register basis")
    entries = register.get("entries")
    if not isinstance(entries, list):
        raise CheckerError("missing-evidence register entries must be an array")
    result: dict[tuple[str, str], dict[str, Any]] = {}
    for index, entry in enumerate(entries):
        what = f"missing-evidence entry {index}"
        known_keys(entry, MISSING_ENTRY_KEYS, what)
        missing = sorted(MISSING_ENTRY_KEYS - {"unknown_reason"} - set(entry))
        if missing:
            raise CheckerError(f"{what}: missing key(s) {missing}")
        entry_path = safe_relative(entry["path"], f"{what} path")
        field = entry["field"]
        if field not in EVIDENCE_PATH_FIELDS:
            raise CheckerError(f"{what}: field must be one of {list(EVIDENCE_PATH_FIELDS)}")
        record = records.get(entry["record_id"]) if isinstance(entry["record_id"], str) else None
        if record is None:
            raise CheckerError(f"{what}: record_id {entry['record_id']!r} is not a current verified record")
        if record[field] != entry_path:
            raise CheckerError(f"{what}: {entry['record_id']} {field} is {record[field]!r}, not the registered path {entry_path!r}")
        key = (entry["record_id"], field)
        if key in result:
            raise CheckerError(f"{what}: {entry['record_id']} {field} is registered twice")
        size, digest = _missing_identity(entry, what)
        if field == "input_ref" and digest is not None and digest != record["input_digest"]:
            raise CheckerError(f"{what}: expected_sha256 of an input_ref must equal the record's input_digest")
        _missing_window(entry["missing_window"], what)
        nonempty(entry["group"], f"{what} group")
        result[key] = {"path": entry_path, "bytes": size, "sha256": digest}
    triples = {(record_id, field, value["path"]) for (record_id, field), value in result.items()}
    if triples != EXPECTED_MISSING_TRIPLES:
        raise CheckerError(
            f"missing-evidence register entries differ from the reviewed register: missing={sorted(EXPECTED_MISSING_TRIPLES - triples)} "
            f"extra={sorted(triples - EXPECTED_MISSING_TRIPLES)}; the (record_id, field, path) triples are pinned in both checkers"
        )
    return result

def validate_harness2(trace: dict[str, Any], evidence_owner: dict[str, str]) -> None:
    """plan §2.9 HS01–HS08 and §16.8 HSF01–HSF16/HSP01 (V123), with the four tracking axes of plan §18.5 (v4.5 Q-50)."""
    hsf_entries = trace.get("hsf_sources")
    if not isinstance(hsf_entries, list):
        raise CheckerError("hsf_sources must be an array")
    by_id = {entry.get("id"): entry for entry in hsf_entries if isinstance(entry, dict)}
    expected_ids = set(EXPECTED_HSF) | {"HSP01"}
    if len(by_id) != len(hsf_entries) or set(by_id) != expected_ids:
        raise CheckerError("hsf_sources must be exactly HSF01–HSF16 and HSP01 without duplicates")
    for sid in sorted(expected_ids):
        entry = known_keys(by_id[sid], HSF_KEYS, sid)
        if set(entry) != HSF_KEYS:
            raise CheckerError(f"{sid}: missing key(s) {sorted(HSF_KEYS - set(entry))}")
        if sid == "HSP01":
            pinned = {"repository": None, "commit": None, "tree": None, "path": HSP01_PATH, "blob_sha": None, "license": None}
        else:
            path, blob = EXPECTED_HSF[sid]
            pinned = {"repository": HARNESS2_REPO, "commit": HARNESS2_COMMIT, "tree": HARNESS2_TREE, "path": path, "blob_sha": blob, "license": HARNESS2_LICENSE}
        if any(entry[key] != expected for key, expected in pinned.items()) or entry["read_range"] != HSF_READ_RANGE:
            raise CheckerError(f"{sid}: fixed Harness² source pin differs from plan §16.8")
        if string_set(entry["e_tasks"], f"{sid} e_tasks") != HSF_E_TASKS:
            raise CheckerError(f"{sid}: E mapping differs (V123 source responsibility {sorted(HSF_E_TASKS)})")
    hs_entries = trace.get("hs_borrowings")
    if not isinstance(hs_entries, list):
        raise CheckerError("hs_borrowings must be an array")
    by_id = {entry.get("id"): entry for entry in hs_entries if isinstance(entry, dict)}
    if len(by_id) != len(hs_entries) or set(by_id) != set(EXPECTED_HS):
        raise CheckerError("hs_borrowings must be exactly HS01–HS08 without duplicates")
    valid_v = {f"V{i:03}" for i in range(1, V_SCENARIO_COUNT + 1)}
    for hid, (section, refs, tasks_text, scenarios, adoption) in EXPECTED_HS.items():
        entry = known_keys(by_id[hid], HS_KEYS, hid)
        if set(entry) != HS_KEYS:
            raise CheckerError(f"{hid}: missing key(s) {sorted(HS_KEYS - set(entry))}")
        tasks = set(tasks_text.split())
        if not isinstance(entry["adoption"], str) or entry["adoption"] not in HS_ADOPTIONS:
            raise CheckerError(f"{hid}: adoption must be one of {sorted(HS_ADOPTIONS)}")
        if (
            entry["section"] != section
            or string_set(entry["hsf_refs"], f"{hid} hsf_refs") != set(refs.split())
            or string_set(entry["e_tasks"], f"{hid} e_tasks") != tasks
            or string_set(entry["v_scenarios"], f"{hid} v_scenarios") != set(scenarios.split())
            or entry["adoption"] != adoption
        ):
            raise CheckerError(f"{hid}: borrowing trace chain differs from plan §2.9")
        if not set(refs.split()) <= set(EXPECTED_HSF) or not tasks <= EXPECTED_E_IDS or not set(scenarios.split()) <= valid_v:
            raise CheckerError(f"{hid}: points to an unknown HSF, E or V id")
        if entry["specification_status"] != "included" or entry["effect_status"] != "not_claimed":
            raise CheckerError(f"{hid}: specification/effect axes differ")
        implementation, verification = entry["implementation_status"], entry["verification_status"]
        if not isinstance(implementation, str) or not isinstance(verification, str) or implementation not in STATUSES or verification not in STATUSES:
            raise CheckerError(f"{hid}: unknown four-axis status")
        if adoption == "registered_only" and (implementation, verification) != ("explicitly_out_of_scope", "explicitly_out_of_scope"):
            raise CheckerError(f"{hid}: a registered-only borrowing is not implemented in this version (plan §2.9, v4.5 Q-44)")
        if implementation == "planned" and verification != "planned":
            raise CheckerError(f"{hid}: planned implementation has incompatible verification")
        if verification == "verified":
            raise CheckerError(f"{hid}: no Harness² borrowing has a verified scenario chain")
        validate_refs(entry["related_evidence_refs"], f"{hid} evidence refs", evidence_owner, tasks)
        nonempty(entry["remaining"], f"{hid} remaining")

def validate_manifest(manifest: Any, repo_root: Path, known_missing: str = DEFAULT_KNOWN_MISSING) -> list[dict[str, Any]]:
    return _validate_manifest(manifest, repo_root, known_missing)[0]

def _validate_manifest(manifest: Any, repo_root: Path, known_missing: str) -> tuple[list[dict[str, Any]], dict[tuple[str, str], dict[str, Any]]]:
    """Structure of the whole manifest; returns the evidence records and the validated missing-evidence register."""
    if not isinstance(manifest, dict):
        raise CheckerError("manifest root must be an object")
    if manifest.get("schema_version") != SCHEMA_VERSION:
        raise CheckerError(f"unsupported schema_version: {manifest.get('schema_version')!r}")
    if manifest.get("plan_version") != PLAN_VERSION or manifest.get("plan_sha256") != PLAN_SHA256:
        raise CheckerError(f"manifest top level is not bound to the current plan {PLAN_VERSION} ({PLAN_SHA256}); historical lineage digests are accepted only inside evidence records")
    known_keys(manifest, ROOT_KEYS, "manifest root")
    validate_project_license(manifest, repo_root)
    validate_plan_lineage(manifest.get("plan_lineage"))
    validate_field_semantics(manifest.get("field_semantics"))
    known_keys(manifest.get("dimensions"), DIMENSION_KEYS, "dimensions")
    if manifest.get("declaration") != "subset_only" or manifest.get("not_full_route_complete") is not True or manifest.get("legacy_equivalence_unverified") is not True or manifest.get("dimensions", {}).get("effect") != "not_claimed":
        raise CheckerError("manifest overstates completion/effect or drops legacy limitation")
    optional_disabled = string_set(manifest.get("optional_disabled"), "optional_disabled")
    if optional_disabled != {"E17", "E18"}:
        raise CheckerError("E17/E18 must remain disabled")
    out_of_scope = string_set(manifest.get("explicitly_out_of_scope_until_revised"), "explicitly_out_of_scope_until_revised")
    trace_coverage = manifest.get("trace_index_coverage")
    expected_coverage = {
        # V080 (E00/E16.6) tracks V001–V123, HS01–HS08, HSF01–HSF16 and HSP01 (plan §18.11, V080 v4.5 Q-50).
        "E00": {"purpose": "index_consistency_only_not_scenario_execution", "ranges": {"K01–K10", "SO01–SO18", "V087–V123", "HS01–HS08", "HSF01–HSF16", "HSP01"}},
        "E16.6": {"purpose": "complete_derived_index_coverage_not_scenario_execution", "ranges": {"B01–B10", "U01–U08", "K01–K10", "SO01–SO18", "E00–E18", "E16.1–E16.6", "V001–V123", "HS01–HS08", "HSF01–HSF16", "HSP01"}},
    }
    if not isinstance(trace_coverage, dict) or set(trace_coverage) != set(expected_coverage):
        raise CheckerError("trace_index_coverage must separately contain E00 and E16.6")
    for task, expected in expected_coverage.items():
        known_keys(trace_coverage[task], COVERAGE_KEYS, f"{task} trace_index_coverage")
        if trace_coverage[task].get("purpose") != expected["purpose"] or string_set(trace_coverage[task].get("ranges"), f"{task} trace ranges") != expected["ranges"]:
            raise CheckerError(f"{task} trace index responsibility differs")

    packages = workspace_packages(repo_root)
    scopes = manifest.get("e_scopes")
    if not isinstance(scopes, dict) or set(scopes) != EXPECTED_E_IDS:
        raise CheckerError(f"e_scopes must be the exact 25 task set; got {sorted(scopes) if isinstance(scopes, dict) else scopes!r}")
    parents = {task: task.rsplit(".", 1)[0] for task in scopes if "." in task and task.rsplit(".", 1)[0] in scopes}
    for child, parent in sorted(parents.items()):
        parent_status = scopes[parent].get("status") if isinstance(scopes[parent], dict) else None
        child_status = scopes[child].get("status") if isinstance(scopes[child], dict) else None
        if parent_status in STATUS_RANK and child_status in STATUS_RANK and STATUS_RANK[parent_status] > STATUS_RANK[child_status]:
            raise CheckerError(f"{parent}: status {parent_status!r} is stronger than sub-package {child} status {child_status!r}; a parent scope may not outrank its weakest child")
    evidence_records: list[dict[str, Any]] = []
    evidence_owner: dict[str, str] = {}
    log_owner: dict[str, tuple[str, str]] = {}
    run_owner: dict[tuple[str, str], tuple[str, str]] = {}
    pr_merge: dict[int, tuple[str | None, str]] = {}
    merged_owner: dict[str, tuple[int, str]] = {}
    source_owner: dict[str, tuple[int, str]] = {}
    for task, entry in scopes.items():
        known_keys(entry, SCOPE_KEYS, f"{task} scope entry")
        nonempty(entry.get("title"), f"{task} title")
        status = entry.get("status")
        if status not in STATUSES:
            raise CheckerError(f"{task}: unknown status {status!r}")
        nonempty(entry.get("status_reason"), f"{task} status_reason")
        files = entry.get("implementation_files")
        if not isinstance(files, list) or any(not isinstance(item, str) for item in files) or len(files) != len(set(files)):
            raise CheckerError(f"{task}: implementation_files must be a duplicate-free string array")
        if status != "planned" and not files:
            raise CheckerError(f"{task}: non-planned scope requires implementation files")
        for file in files:
            inspect_relative(repo_root, file, f"{task} implementation file", True)
        if string_set(entry.get("scenarios"), f"{task} scenarios") != EXPECTED_E_SCENARIOS[task]:
            raise CheckerError(f"{task}: scenario execution responsibility differs")
        remaining = entry.get("remaining")
        if not isinstance(remaining, list) or any(not isinstance(item, str) or not item.strip() for item in remaining):
            raise CheckerError(f"{task}: remaining must be an array of non-empty strings")
        records = entry.get("verified_subscopes")
        if not isinstance(records, list):
            raise CheckerError(f"{task}: verified_subscopes must be an array")
        if status == "verified" and (remaining or not records):
            raise CheckerError(f"{task}: verified status requires evidence and no remaining work")
        if status != "verified" and not remaining:
            raise CheckerError(f"{task}: non-verified task must state remaining work")
        if status == "planned" and records:
            ids = [item.get("id") for item in records if isinstance(item, dict)]
            raise CheckerError(f"{task}: status 'planned' is incompatible with verified evidence records {ids}; a planned scope has nothing verified")
        if status != "verified" and entry.get("completion_evidence_refs") is not None:
            raise CheckerError(f"{task}: completion_evidence_refs claims completion but status is {status!r}")
        if status == "verified" and task in optional_disabled:
            raise CheckerError(f"{task}: optional scope is listed in optional_disabled and no enablement is recorded; status 'verified' is not allowed")
        if status == "explicitly_out_of_scope" and task not in out_of_scope:
            raise CheckerError(f"{task}: status 'explicitly_out_of_scope' is not declared in explicitly_out_of_scope_until_revised")
        for record in records:
            identifier = validate_evidence(record, task, repo_root, packages, files)
            if identifier in EXPECTED_WITHDRAWN:
                raise CheckerError(f"{identifier}: withdrawn record reappears in verified_subscopes; a withdrawn record is never revived (re-verify it with a new record that supersedes it)")
            if identifier in evidence_owner:
                raise CheckerError(f"duplicate evidence id {identifier}")
            evidence_owner[identifier] = task
            evidence_records.append(record)
            log_path, source_sha, test_entry = record["log_path"], record["source_sha"], record["test_entry"]
            if log_path in log_owner:
                raise CheckerError(f"{identifier}: log_path {log_path!r} duplicates evidence {log_owner[log_path][0]} of task {log_owner[log_path][1]}; one test run may support only one evidence record")
            log_owner[log_path] = (identifier, task)
            run = run_owner.setdefault((source_sha, test_entry), (identifier, task))
            if run[1] != task:
                raise CheckerError(f"{identifier}: the same test run ({test_entry} at source_sha {source_sha}) is already claimed by {run[0]} of task {run[1]}; evidence sets may not be duplicated across tasks")
            number, merged = int(PR_RE.fullmatch(record["pr"]).group(1)), record["merged_sha"]
            known = pr_merge.setdefault(number, (merged, identifier))
            if known[0] != merged:
                raise CheckerError(f"{identifier}: PR #{number} carries merged_sha {merged!r} here but {known[0]!r} in {known[1]}; one PR cannot be both merged and unmerged or merged twice")
            if merged is not None:
                claim = merged_owner.setdefault(merged, (number, identifier))
                if claim[0] != number:
                    raise CheckerError(f"{identifier}: merged_sha {merged} is already recorded as the merge commit of PR #{claim[0]} ({claim[1]}); one merge commit cannot also merge PR #{number}")
                if merged in source_owner and source_owner[merged][0] != number:
                    raise CheckerError(f"{identifier}: merged_sha {merged} is the source_sha of PR #{source_owner[merged][0]} ({source_owner[merged][1]}), not a merge of PR #{number}")
            if source_sha in merged_owner and merged_owner[source_sha][0] != number:
                raise CheckerError(f"{identifier}: source_sha {source_sha} is recorded as the merge commit of PR #{merged_owner[source_sha][0]} ({merged_owner[source_sha][1]})")
            source_owner.setdefault(source_sha, (number, identifier))
        if status == "verified":
            completion = string_set(
                entry.get("completion_evidence_refs"),
                f"{task} completion evidence refs",
            )
            owned = {ref for ref, owner in evidence_owner.items() if owner == task}
            if not completion or not completion.issubset(owned):
                raise CheckerError(
                    f"{task}: verified status lacks explicit task completion evidence"
                )

    reviewed = reviewed_merge_facts(repo_root)
    for record in evidence_records:
        number = int(PR_RE.fullmatch(record["pr"]).group(1))
        if number not in reviewed:
            raise CheckerError(f"{record['id']}: no reviewed merge status for PR {number}")
        if record["merged_sha"] != reviewed[number]:
            raise CheckerError(f"{record['id']}: merged_sha differs from the reviewed PR merge")
    validate_withdrawn_records(manifest.get("withdrawn_records"), evidence_records, evidence_owner, reviewed)
    register = validate_missing_register(manifest, repo_root, known_missing, {record["id"]: record for record in evidence_records})

    trace = known_keys(manifest.get("traceability"), TRACEABILITY_KEYS, "traceability")
    so_entries = trace.get("so_sources")
    if not isinstance(so_entries, list):
        raise CheckerError("so_sources must be an array")
    so_by_id = {entry.get("id"): entry for entry in so_entries if isinstance(entry, dict)}
    if len(so_by_id) != len(so_entries) or set(so_by_id) != set(EXPECTED_SO):
        raise CheckerError("SO01-SO18 must be exact and duplicate-free")
    for sid, expected in EXPECTED_SO.items():
        entry = so_by_id[sid]
        known_keys(entry, SO_SOURCE_KEYS, sid)
        path, blob, read_range, tasks_text = expected
        if entry.get("repository") != SKILLOPT_REPO or entry.get("commit") != SKILLOPT_COMMIT or entry.get("path") != path or entry.get("blob_sha") != blob or entry.get("read_range") != read_range or entry.get("license") != "MIT":
            raise CheckerError(f"{sid}: fixed source pin differs")
        tasks = set(tasks_text.split())
        if string_set(entry.get("e_tasks"), f"{sid} e_tasks") != tasks:
            raise CheckerError(f"{sid}: E mapping differs")
        nonempty(entry.get("license_status"), f"{sid} license_status")
        source_status = entry.get("verification_status")
        if source_status not in {"reference_pins_only", "verified"}:
            raise CheckerError(f"{sid}: unknown verification_status")
        local = entry.get("local_use")
        if not isinstance(local, dict) or local.get("mode") not in {"reference_only_no_upstream_runtime_import", "adapted_with_license_notice"}:
            raise CheckerError(f"{sid}: local use mode is invalid")
        known_keys(local, LOCAL_USE_KEYS, f"{sid} local_use")
        artifacts = local.get("artifacts")
        if not isinstance(artifacts, list) or not artifacts or len(artifacts) != len(set(artifacts)):
            raise CheckerError(f"{sid}: local landing is absent or duplicated")
        for artifact in artifacts:
            inspect_relative(repo_root, artifact, f"{sid} local artifact", True)
        source_refs = entry.get("verification_evidence_refs", [])
        refs = validate_refs(source_refs, f"{sid} evidence refs", evidence_owner, tasks)
        if source_status == "verified":
            completion = string_set(
                entry.get("completion_evidence_refs"),
                f"{sid} completion evidence refs",
            )
            if not completion or not completion.issubset(refs):
                raise CheckerError(f"{sid}: verified source status lacks completion evidence")

    expected_v = expected_v_to_e()
    v_entries = trace.get("v_scenarios")
    if not isinstance(v_entries, list):
        raise CheckerError("v_scenarios must be an array")
    v_by_id = {entry.get("id"): entry for entry in v_entries if isinstance(entry, dict)}
    if len(v_by_id) != len(v_entries) or set(v_by_id) != set(expected_v):
        raise CheckerError(f"V001-V{V_SCENARIO_COUNT:03} must be exact and duplicate-free")
    for vid, tasks in expected_v.items():
        entry = v_by_id[vid]
        known_keys(entry, V_SCENARIO_KEYS, vid)
        if entry.get("specification_status") != "included" or entry.get("effect_status") != "not_claimed":
            raise CheckerError(f"{vid}: specification/effect axes differ")
        status, verification = entry.get("status"), entry.get("verification_status")
        if status not in STATUSES or verification not in STATUSES:
            raise CheckerError(f"{vid}: unknown status")
        if status == "planned" and verification != "planned":
            raise CheckerError(f"{vid}: planned scenario has incompatible verification status")
        if (status == "verified") != (verification == "verified"):
            raise CheckerError(f"{vid}: verified status axes disagree")
        if string_set(entry.get("e_tasks"), f"{vid} e_tasks") != tasks:
            raise CheckerError(f"{vid}: E responsibility differs")
        refs = validate_refs(entry.get("related_evidence_refs"), f"{vid} evidence refs", evidence_owner, tasks)
        remaining = nonempty(entry.get("remaining"), f"{vid} remaining") if status != "verified" else entry.get("remaining")
        if status == "verified":
            completion = string_set(
                entry.get("completion_evidence_refs"),
                f"{vid} completion evidence refs",
            )
            if (
                not refs
                or not completion
                or not completion.issubset(refs)
                or remaining not in (None, "", [])
            ):
                raise CheckerError(
                    f"{vid}: verified scenario lacks completion evidence or retains unfinished scope"
                )

    groups = {"b_commitments": "B", "u_commitments": "U", "k_commitments": "K"}
    for field, prefix in groups.items():
        entries = trace.get(field)
        if not isinstance(entries, list):
            raise CheckerError(f"{field} must be an array")
        expected_ids = {key for key in EXPECTED_COMMITMENTS if key.startswith(prefix)}
        by_id = {entry.get("id"): entry for entry in entries if isinstance(entry, dict)}
        if len(by_id) != len(entries) or set(by_id) != expected_ids:
            raise CheckerError(f"{field} exact set differs")
        for cid in expected_ids:
            entry = by_id[cid]
            known_keys(entry, COMMITMENT_KEYS, cid)
            sections, sources, tasks_text, scenarios = EXPECTED_COMMITMENTS[cid]
            tasks = set(tasks_text.split())
            if entry.get("specification_status") != "included" or entry.get("effect_status") != "not_claimed":
                raise CheckerError(f"{cid}: specification/effect axes differ")
            implementation, verification = entry.get("implementation_status"), entry.get("verification_status")
            if implementation not in STATUSES or verification not in STATUSES:
                raise CheckerError(f"{cid}: unknown four-axis status")
            if implementation == "planned" and verification != "planned":
                raise CheckerError(f"{cid}: planned implementation has incompatible verification")
            if verification == "verified" and implementation in {"planned", "blocked"}:
                raise CheckerError(f"{cid}: verified commitment has incompatible implementation status")
            if string_set(entry.get("sections"), f"{cid} sections") != set(sections.split()) or string_set(entry.get("sources"), f"{cid} sources") != set(sources.split()) or string_set(entry.get("e_tasks"), f"{cid} e_tasks") != tasks or string_set(entry.get("v_scenarios"), f"{cid} scenarios") != set(scenarios.split()):
                raise CheckerError(f"{cid}: normative trace chain differs")
            artifacts = entry.get("local_artifacts")
            if not isinstance(artifacts, list) or not artifacts or len(artifacts) != len(set(artifacts)):
                raise CheckerError(f"{cid}: local artifacts are absent or duplicated")
            for artifact in artifacts:
                inspect_relative(repo_root, artifact, f"{cid} local artifact", True)
            refs = validate_refs(entry.get("related_evidence_refs"), f"{cid} evidence refs", evidence_owner, tasks)
            if verification == "verified":
                completion = string_set(
                    entry.get("completion_evidence_refs"),
                    f"{cid} completion evidence refs",
                )
                if not refs or not completion or not completion.issubset(refs):
                    raise CheckerError(
                        f"{cid}: verified commitment lacks completion evidence"
                    )
            if verification != "verified":
                nonempty(entry.get("remaining"), f"{cid} remaining")
    validate_harness2(trace, evidence_owner)
    validate_record_pins(evidence_records)
    return evidence_records, register

@dataclass
class CheckReport:
    structure_valid: bool = True
    plan_binding_available: bool = False
    input_binding_available: bool = False
    input_binding_scope: str = "local_input_manifest_hashes_only"
    needs_verification: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    plan_version: str | None = None
    plan_sha256: str | None = None
    evidence_store_checked: bool = False
    evidence_missing: list[str] = field(default_factory=list)
    evidence_registered_not_checked: list[str] = field(default_factory=list)
    def fail(self, message: str) -> None:
        self.structure_valid = False
        self.errors.append(message)
    def to_dict(self) -> dict[str, Any]:
        return {
            "structure_valid": self.structure_valid,
            "plan_binding_available": self.plan_binding_available,
            "input_binding_available": self.input_binding_available,
            "input_binding_scope": self.input_binding_scope,
            "needs_verification": self.needs_verification,
            "errors": self.errors,
            "plan_version": self.plan_version,
            "plan_sha256": self.plan_sha256,
            "evidence_store_checked": self.evidence_store_checked,
            "evidence_missing": self.evidence_missing,
            "evidence_registered_not_checked": self.evidence_registered_not_checked,
            "disclaimer": "exit 0 proves only the frozen derivative's local structure. Commands were parsed, never executed; Git ancestry, logs, licenses, hosts, effects, and deployment were not automatically authenticated.",
        }

def validate_external_plan(path: Path) -> str:
    if path.is_symlink() or not path.is_file():
        raise CheckerError(f"source-of-truth is linked or not a regular file: {path}")
    actual = sha256_of_file(path)
    if actual != PLAN_SHA256:
        raise CheckerError(f"source-of-truth SHA-256 mismatch: computed={actual} expected={PLAN_SHA256} (plan {PLAN_VERSION})")
    return actual

def in_local_evidence_store(rel: str) -> bool:
    return rel.split("/", 1)[0] == LOCAL_EVIDENCE_STORE

def check_local_evidence(report: CheckReport, evidence: list[dict[str, Any]], register: dict[tuple[str, str], dict[str, Any]], register_rel: str, repo_root: Path, evidence_root: Path | None) -> bool:
    """plan §18.10 (v4.5 Q-50): three outcomes per evidence path. A missing path listed in the register is reported in
    evidence_missing, or in evidence_registered_not_checked when it lies in out/ and the evidence store is not supplied (its
    absence was not checked); a missing path outside the register fails (for out/ paths only when the evidence store is supplied);
    a registered path that reappears fails unless the registered SHA-256 (and bytes, when registered) match, and always
    fails when its identity was registered as unknown. Returns whether every input manifest is present and matching."""
    store_checked = evidence_root is not None
    all_inputs = bool(evidence)
    unjudged = 0
    for record in evidence:
        identifier = record["id"]
        for field_name in EVIDENCE_PATH_FIELDS:
            rel = record[field_name]
            in_store = in_local_evidence_store(rel)
            base = evidence_root if (store_checked and in_store) else repo_root
            path = inspect_relative(base, rel, f"{identifier} {field_name}", False)
            entry = register.get((identifier, field_name))
            if path is not None:
                if field_name == "input_ref" and sha256_of_file(path) != record["input_digest"]:
                    raise CheckerError(f"{identifier}: local input manifest SHA-256 mismatch")
                if entry is not None:
                    if entry["sha256"] is None:
                        raise CheckerError(f"{identifier}: {field_name} {rel!r} is registered missing with unknown identity and has reappeared; the register cannot authenticate it, so it must be verified through another binding (for example the input_ref SHA-256) and registered as a new record instead of restoring the old evidence")
                    if sha256_of_file(path) != entry["sha256"] or (entry["bytes"] is not None and path.stat().st_size != entry["bytes"]):
                        raise CheckerError(f"{identifier}: {field_name} {rel!r} is registered missing and has reappeared with bytes or SHA-256 that differ from the register")
                    report.needs_verification.append(f"{identifier}: registered missing {field_name} {rel!r} has reappeared with its registered identity; remove it from the register in the next ledger update")
                continue
            if field_name == "input_ref":
                all_inputs = False
            if entry is not None and in_store and not store_checked:
                report.evidence_registered_not_checked.append(f"{identifier} {field_name} {rel}")
            elif entry is not None:
                report.evidence_missing.append(f"{identifier} {field_name} {rel}")
            elif in_store and not store_checked:
                unjudged += 1
                label = "input manifest" if field_name == "input_ref" else field_name
                report.needs_verification.append(f"{identifier}: local {label} is unavailable")
            else:
                raise CheckerError(f"{identifier}: {field_name} {rel!r} is missing locally and is not listed in the missing-evidence register {register_rel}")
    if not store_checked:
        report.needs_verification.append(
            f"local evidence store not supplied (--evidence-root): {unjudged} absent {LOCAL_EVIDENCE_STORE}/ evidence path(s) were reported, not judged against "
            f"the missing-evidence register, and {len(report.evidence_registered_not_checked)} registered path(s) are listed as registered, not checked"
        )
    return all_inputs

def run_check(repo_root: Path, manifest_rel: str, source_of_truth: Path | None, known_missing: str | None = None, evidence_root: Path | None = None) -> CheckReport:
    report = CheckReport()
    if not repo_root.is_dir():
        report.fail(f"repo root does not exist or is not a directory: {repo_root}")
        return report
    if evidence_root is not None and not evidence_root.is_dir():
        report.fail(f"evidence root does not exist or is not a directory: {evidence_root}")
        return report
    report.evidence_store_checked = evidence_root is not None
    if source_of_truth is not None:
        try:
            validate_external_plan(source_of_truth)
            report.plan_binding_available = True
        except CheckerError as exc:
            report.fail(str(exc))
            return report
    else:
        report.needs_verification.append("no --source-of-truth supplied: plan bytes were not locally hash-bound")
    try:
        register_rel = safe_relative(known_missing if known_missing is not None else DEFAULT_KNOWN_MISSING, "--known-missing")
        manifest_path = inspect_relative(repo_root, manifest_rel, "manifest", True)
        manifest = load_json_strict(manifest_path.read_text(encoding="utf-8"))
        report.plan_version = manifest.get("plan_version") if isinstance(manifest, dict) else None
        report.plan_sha256 = manifest.get("plan_sha256") if isinstance(manifest, dict) else None
        evidence, register = _validate_manifest(manifest, repo_root, register_rel)
        report.input_binding_available = check_local_evidence(report, evidence, register, register_rel, repo_root, evidence_root)
        for record in evidence:
            if record["merged_sha"] is not None:
                number = PR_RE.fullmatch(record["pr"]).group(1)
                state = record.get("pr_state") or "unrecorded"
                report.needs_verification.append(f"{record['id']}: merged_sha {record['merged_sha']} for PR #{number} (pr_state {state}) is a manifest assertion; git ancestry (merge commit on main containing source_sha {record['source_sha']}) was not checked")
        report.needs_verification.extend([
            "manifest commands were parsed but never executed; exit codes and actual results require controller verification",
            "source_sha, PR ancestry, merge state, upstream SO objects, and license conclusions require independent verification",
        ])
    except (CheckerError, OSError, UnicodeError) as exc:
        report.input_binding_available = False
        report.fail(str(exc))
    return report

def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Read-only rsia.support_scope.v2 structural checker")
    parser.add_argument("--repo-root", required=True, type=Path)
    parser.add_argument("--manifest")
    parser.add_argument("--project-license-metadata-only", action="store_true")
    parser.add_argument("--workspace-packages-only", action="store_true")
    parser.add_argument("--source-of-truth", type=Path)
    parser.add_argument("--known-missing", help=f"missing-evidence register relative to --repo-root (default {DEFAULT_KNOWN_MISSING}); must equal evidence_missing_register.path")
    parser.add_argument("--evidence-root", type=Path, help="directory that holds the local evidence store out/ (normally the primary checkout); without it, absent out/ evidence paths are reported but not judged")
    return parser

def main(argv: list[str] | None = None) -> int:
    parser = build_arg_parser()
    args = parser.parse_args(argv)
    manifest_only = (args.known_missing, args.evidence_root)
    if args.workspace_packages_only:
        if args.project_license_metadata_only or args.manifest is not None or args.source_of_truth is not None or any(item is not None for item in manifest_only):
            parser.error("workspace-packages-only accepts only --repo-root")
        try:
            packages = workspace_packages(args.repo_root.absolute())
        except (CheckerError, OSError, UnicodeError, ValueError) as exc:
            print(json.dumps({"errors": [str(exc)]}))
            return 1
        print(json.dumps({name: member.as_posix() for name, (member, _) in packages.items()}, sort_keys=True))
        return 0
    if args.project_license_metadata_only:
        if args.manifest is not None or args.source_of_truth is not None or any(item is not None for item in manifest_only):
            parser.error("metadata-only accepts only --repo-root")
        try:
            metadata = validate_project_license_metadata(args.repo_root.absolute())
        except CheckerError as exc:
            print(json.dumps({"errors": [str(exc)]}))
            return 1
        print(json.dumps(metadata, sort_keys=True))
        return 0
    if args.manifest is None:
        parser.error("--manifest is required unless metadata-only is selected")
    evidence_root = args.evidence_root.absolute() if args.evidence_root is not None else None
    report = run_check(args.repo_root.absolute(), args.manifest, args.source_of_truth, args.known_missing, evidence_root)
    print(json.dumps(report.to_dict(), indent=2, sort_keys=True))
    return 0 if report.structure_valid else 1

if __name__ == "__main__":
    sys.exit(main())
