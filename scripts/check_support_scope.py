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
# Optional per-record recheck marker (plan §18.10 machine form, v4.5 Q-50). Missing means not_affected.
# "required" is a pending obligation and never passes; records of the tasks in the recheck union that are still
# bound to an older lineage digest must carry PLAN_RECHECKED. A rechecked_<version> marker must name a lineage
# version later than the record's own plan_version (a record is never rechecked against its own or an older plan).
PLAN_RECHECKED = "rechecked_v4.5"
PLAN_RECHECK_VALUES = {"not_affected", "required", "rechecked_v4.2", "rechecked_v4.3", "rechecked_v4.4", PLAN_RECHECKED}
# Exact E-task union of the landing points of plan §19.6 (as corrected by v4.5) and §19.7 (plan §18.11); the keys
# match literally, so "E16" is the parent key only and is not expanded to E16.1–E16.6 (plan §18.10, v4.5 Q-50).
PLAN_RECHECK_TASKS = {"E00", "E01", "E02", "E03", "E04", "E05", "E06", "E07", "E08", "E09", "E10", "E13", "E16", "E16.1", "E16.3", "E16.5", "E16.6"}
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
        "optional record-level array of withdrawn record ids that this re-verification replaces; every id must be listed in withdrawn_records "
        "with status superseded and belong to the same E task; the replaced record is neither rewritten nor revived"
    ),
    "completes": (
        "optional record-level array of still-valid record ids whose evidence this new record completes (for example a re-run of a record "
        "with registered missing evidence); the completed records stay unchanged and keep their own credit"
    ),
    "note": "optional record-level free-text note (for example a category or the task that will complete the record); it carries no verification credit",
    "withdrawn_records": (
        "top-level list of records withdrawn from verified_subscopes because a revision changed the behavior they asserted: id, withdrawn_in_pr, "
        "clause, owner_task and status (pending_recheck, superseded or replaced_by_binding); a listed id never reappears in verified_subscopes"
    ),
    "evidence_missing_register": (
        "top-level path and SHA-256 of the missing-evidence register: a registered missing path is reported, a missing path outside the register "
        "fails once the local evidence store is checked, and a registered path that reappears fails unless its registered bytes and SHA-256 match "
        "(a path registered with unknown identity always fails on reappearance)"
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

# Withdrawn records (plan §18.10, v4.5 Q-50): the pending-recheck list. The id set is reviewed and pinned here (and in
# crates/evo-core/tests/support_scope.rs) so that dropping an entry fails; ids never return to verified_subscopes.
WITHDRAWN_KEYS = {"id", "withdrawn_in_pr", "clause", "owner_task", "status"}
WITHDRAWN_STATUSES = {"pending_recheck", "superseded", "replaced_by_binding"}
OWNER_TASK_RE = re.compile(r"^AG-\d{3}[a-z]?$")
EXPECTED_WITHDRAWN_IDS = {
    # withdrawn by PR #165 (AG-124, v4.3 recheck) and PR #169 (AG-128, E04.ag054 correction)
    "E10.controller_acceptance", "E10.replay_management_controller_acceptance", "E10.ag064_replay_episode_recovery",
    "E10.ag103_replay_fairness_waits_legal_set_v42", "E08.ag037_economic_replay_cleanup", "E09.ag074_local_scoped_acceptance",
    "E16.6.ag012_plan_v42_index_binding", "E04.ag097_broker_close_before_refuse", "E04.ag105_broker_bound_recovery_close_v42",
    "E08.ag046_write_side_revocation", "E08.ag059_manifest_prewrite_gate", "E04.ag054_typed_predispatch",
    "E06.controller_acceptance", "E13.controller_acceptance", "E16.4.real_store_rejection_controller_acceptance",
    "E09.ag042_recovery_counting", "E13.ag028_consolidation_trigger", "E16.1.durable_import_controller_acceptance",
    "E02.ag062_required_improver_baselines",
    # withdrawn by the v4.5 rebind (AG-182): v4.4 Q-29/Q-32 and v4.5 Q-57 changed the asserted behavior
    "E09.ag033_run_next_trusted_gate", "E16.6.ag131_v43_project_license_metadata", "E05.ag052_versioned_batch_verdicts",
    "E05.ag080_local_scoped_acceptance", "E05.ag101_streaming_verdicts_v42",
}

# Missing-evidence register (plan §18.10 "本地缺失的证据路径", v4.5 Q-50).
DEFAULT_KNOWN_MISSING = "reports/evidence-missing.json"
REGISTER_POINTER_KEYS = {"path", "sha256"}
MISSING_SCHEMA_VERSION = "rsia.evidence_missing.v1"
MISSING_REGISTER_KEYS = {"schema_version", "basis", "entries"}
MISSING_ENTRY_KEYS = {"path", "record_id", "field", "expected_bytes", "expected_sha256", "unknown_reason", "missing_window", "group"}
MISSING_WINDOW_KEYS = {"after", "before"}
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
    recheck = record["plan_recheck"] if "plan_recheck" in record else "not_affected"
    if not isinstance(recheck, str) or recheck not in PLAN_RECHECK_VALUES:
        raise CheckerError(f"{identifier}: plan_recheck must be one of {sorted(PLAN_RECHECK_VALUES)}, got {recheck!r}")
    if recheck == "required":
        raise CheckerError(f"{identifier}: plan_recheck is 'required'; the record was verified against plan {version} in a scope revised by {PLAN_VERSION} and has not been rechecked, so the manifest cannot pass until it is rechecked or withdrawn")
    if recheck.startswith("rechecked_"):
        order = list(PLAN_LINEAGE)
        if order.index(recheck[len("rechecked_"):]) <= order.index(version):
            raise CheckerError(f"{identifier}: plan_recheck {recheck!r} does not name a plan later than the record's own plan_version {version!r}; a record is only rechecked against a later plan")
    if version != PLAN_VERSION and task in PLAN_RECHECK_TASKS and recheck != PLAN_RECHECKED:
        raise CheckerError(f"{identifier}: task {task} is touched by the {PLAN_VERSION} revision; a record still bound to plan {version} must carry plan_recheck {PLAN_RECHECKED!r}, got {recheck!r}")
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
    if set(withdrawn) != EXPECTED_WITHDRAWN_IDS:
        raise CheckerError(
            f"withdrawn_records differ from the reviewed withdrawn list: missing={sorted(EXPECTED_WITHDRAWN_IDS - set(withdrawn))} "
            f"extra={sorted(set(withdrawn) - EXPECTED_WITHDRAWN_IDS)}; an entry is never dropped and a new withdrawal is reviewed in both checkers"
        )
    revived = sorted(set(withdrawn) & set(evidence_owner))
    if revived:
        raise CheckerError(f"withdrawn record(s) {revived} reappear in verified_subscopes; a withdrawn record is never revived (re-verify it with a new record that supersedes it)")
    superseded_by: dict[str, list[str]] = {}
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
        for kept in record.get("completes", []):
            if kept in withdrawn:
                raise CheckerError(f"{identifier}: completes the withdrawn record {kept!r}; a withdrawn record can only be superseded")
            if kept not in evidence_owner:
                raise CheckerError(f"{identifier}: completes {kept!r}, which is not a current verified record")
    for identifier, entry in withdrawn.items():
        if entry["status"] == "superseded" and identifier not in superseded_by:
            raise CheckerError(f"{identifier}: withdrawn status is 'superseded' but no verified record supersedes it")
    return withdrawn

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
            if identifier in EXPECTED_WITHDRAWN_IDS:
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
    validate_missing_register(manifest, repo_root, known_missing, {record["id"]: record for record in evidence_records})

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
        raise CheckerError("V001-V120 must be exact and duplicate-free")
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
    return evidence_records

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
    evidence_missing; a missing path outside the register fails (for out/ paths only when the evidence store is supplied);
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
            if entry is not None:
                report.evidence_missing.append(f"{identifier} {field_name} {rel}")
            elif in_store and not store_checked:
                unjudged += 1
                label = "input manifest" if field_name == "input_ref" else field_name
                report.needs_verification.append(f"{identifier}: local {label} is unavailable")
            else:
                raise CheckerError(f"{identifier}: {field_name} {rel!r} is missing locally and is not listed in the missing-evidence register {register_rel}")
    if not store_checked:
        report.needs_verification.append(f"local evidence store not supplied (--evidence-root): {unjudged} absent {LOCAL_EVIDENCE_STORE}/ evidence path(s) were reported, not judged against the missing-evidence register")
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
        evidence = validate_manifest(manifest, repo_root, register_rel)
        register = validate_missing_register(manifest, repo_root, register_rel, {record["id"]: record for record in evidence})
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
