<p align="center">
  <img src="docs/assets/rsia-logo-banner.png" alt="RSIAgent" width="100%">
</p>

<h1 align="center">RSIAgent</h1>

<p align="center">
  <b>Controlled Recursive Self-Improvement Runtime for Cross-Domain LLM Agents</b><br>
  Not only get stronger — improve <b>how</b> you get stronger.
</p>

<p align="center">
  <a href="#core-architecture-dual-loop-cognition--controlled-evolution"><img src="https://img.shields.io/badge/architecture-dual--loop%20RSI-007ec6?style=flat-square" alt="Architecture"></a>
  <a href="#two-layer-fact--asset-pipeline--version-evolution-chain"><img src="https://img.shields.io/badge/data_plane-fact%20%E2%86%94%20asset%20pipeline-00897b?style=flat-square" alt="Two-Layer Pipeline"></a>
  <a href="#autonomy-ladder-b0l5"><img src="https://img.shields.io/badge/autonomy-B0--L5%20ladder-149954?style=flat-square" alt="Autonomy Ladder"></a>
  <a href="#research-basis--engineering-references"><img src="https://img.shields.io/badge/research-papers%20%26%20systems-68448b?style=flat-square" alt="Research & References"></a>
  <a href="#quick-start--integration-flow"><img src="https://img.shields.io/badge/integration-MCP%20%7C%20HTTP%20%7C%20CLI-f28c45?style=flat-square" alt="Integration"></a>
  <a href="docs/implementation-ledger.md"><img src="https://img.shields.io/badge/evidence-implementation%20ledger-e65100?style=flat-square" alt="Implementation Ledger"></a>
  <a href="https://acosmi.com"><img src="https://img.shields.io/badge/website-Acosmi-0052d9?style=flat-square" alt="Website"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Non--Commercial%20Open-4c6ef5?style=flat-square" alt="License"></a>
</p>

<p align="center">
  <b>English</b> | <a href="README.zh-CN.md">中文</a>
</p>

**RSIAgent** is an independent **Rust** controlled recursive self-improvement runtime. It decouples **online task execution** from **offline self-evolution governance**: coding assistants, general-purpose agents, and trusted hosts (such as Claude Code, Cursor, Codex, Antigravity, OpenCode, and custom business agents) connect over **MCP, HTTP, and CLI** into a unified, local-first single-writer runtime (`Prepare → Feedback → Proposal → Evaluation → Release → Application`). Under strict role boundaries, root budgets, and independent acceptance gates, experience turns into lasting, inheritable improvements to both **task skills (`Skill`)** and **the meta-improvement strategy itself (`Improver`)**. **Focus on your business logic — without rebuilding self-improvement infrastructure for every scenario.**

> **Engineering Baseline:** Normative plan **v4.3** (2026-10-12, SHA-256 `73dd00bf5ef6e9757d0c391c9a8be4ca92b6e06a750aacb542817f82cbd208a7`). That plan body stays off this remote.  
> **Current Status:** As of 2026-10-16, core engineering is roughly **80%** complete (maintainer estimate) and approaching end-to-end verification; engineering progress does not imply acceptance pass or measured benefit. Public evidence is tracked in the [implementation ledger](docs/implementation-ledger.md) and the derived support index. **Do not claim measured recursive gains, a production host, or a complete E00–E18 route.**

---

## Core Architecture: Dual-Loop Cognition & Controlled Evolution

Today, “agent improvement” usually means ad-hoc reflection prompts or a skill library that only grows (`Add only`) — inflating latency while injecting noisy or conflicting instructions (library drift). Inspired by dual-process cognitive architectures (**System 1 fast execution + System 2 slow evolution**), RSIAgent splits runtime execution and recursive evolution into two loops with distinct latency and authority boundaries:

```text
External Task Agents / Multi-Host Clients (Claude Code / Cursor / Codex / Antigravity / Custom Agents)
        │  MCP (stdio/remote) · HTTP · CLI
        ▼
┌──────────────────────────────────────────────────────────────────────┐
│ RSIAgent Runtime (Rust) — One Runtime · Single-Writer Data Plane     │
│                                                                      │
│  [System 1 · Online Fast Loop]  Prepare Snapshot ──► Task Execution  │
│                                 ──► Feedback & 4-Stage Application   │
│                                     Receipts (zero evolution block)  │
│                                           │                          │
│                                           ▼ Untrusted evidence store │
│  [System 2 · Offline Slow Loop] Learnable Failure Attribution        │
│                                 ──► Replay / Offline Simulation      │
│                                 ──► Proposal & Counterexamples       │
│                                 ──► Independent Evaluator Acceptance │
│                                 ──► Approval / Canary / Active       │
└──────────────────────────────────────────────────────────────────────┘
```

| Dimension | System 1: Online Task Execution Loop (Fast Path) | System 2: Controlled Recursive Evolution Loop (Slow Path) |
| --- | --- | --- |
| **Responsibility** | Assemble a bounded active skill snapshot for the task goal + host capabilities, and record verbatim execution traces | Identify learnable failures from accumulated traces; generate, replay, filter, and evaluate new candidates offline |
| **Model Overhead** | Zero extra LLM calls by default (`ToolOnly` snapshot assembly); never blocks the host's critical path | Asynchronous job scheduling strictly bounded by root budgets (`Budget`) and recoverable leases |
| **Target Objects** | Frozen read-only active snapshot + append-only stage facts and receipts | Versioned **Skill** (task method) and **Improver** (`evo.strategy.v1` meta-improvement strategy JSON) |
| **Safety Boundary** | External inputs and traces are treated strictly as **untrusted data**, never executable instructions or self-authorization | Independent `Evaluator` acceptance + human approval / canary / atomic release; candidates never self-approve |

---

## Two-Layer Fact ↔ Asset Pipeline & Version-Evolution Chain

Drawing on long-horizon productivity and software-engineering memory design, RSIAgent avoids storing vague summaries or unpruned skill dumps. Every task trace feeds a **two-layer pipeline with bidirectional provenance** and a **version-evolution chain**:

### 1. Verbatim Stage Facts ↔ Thresholded Evolvable Assets

| Layer | What It Records | Admission Policy | Structure & Governance Contract |
| --- | --- | --- | --- |
| **Fact Layer (`StageFact` & Receipts)** | What the run actually did: goal, tool trace, success/failure class, and four-stage application receipts | **Zero-threshold verbatim retention** (append-only immutable records) | Tracks `Returned → Attached → Observed → VerifiedBenefit`; preserves both positive and negative outcomes |
| **Asset Layer (`Skill` & `Improver`)** | Reusable approaches for future tasks (**Skill**) and meta-strategies for future refinement (**Improver**) | **High-threshold gated admission** (must pass counterexample checks & independent evaluation) | **Skill**: narrow text skill (evidence + applicability boundary + counterexample)<br>**Improver**: evolvable strategy JSON (`evo.strategy.v1`) governing how experience is refined, filtered, and accepted |

- **Bidirectional Provenance & Cascading Revocation**: Every derived candidate (`Skill` or `Improver`) is cryptographically and relationally bound to its upstream stage facts and evidence IDs. If upstream evidence is tainted or revoked, the runtime cascades revocation and cleanup across derived assets so active state remains auditable and recoverable.
- **Full-Lifecycle Skill Maintenance (Not “Add Only”)**: Skills are versioned candidates with a full lifecycle. RSIAgent tracks downstream contribution via `Application` receipts, caps active skills per `Prepare` snapshot, and prunes or retires low-utility assets based on measured usefulness rather than age.

### 2. Why a Version-Evolution Chain & Counterexamples Matter (Summaries ≠ Learning)

In long-horizon coding and complex tool use, plain text summaries inevitably drop **reverted attempts**, **negative trajectories**, and **intermediate revisions no longer present at `HEAD`**. RSIAgent enforces exact evolutionary lineage:

- **Bounded Atomic Edits & State Evolution Chain**: Candidates progress through `Proposed → Validated → AcceptancePassed → Approved → Canary → Active` (plus `Retired / Rejected / Revoked`), preserving the exact diff and rationale of each revision rather than overwriting history in place.
- **First-Class Counterexamples**: Every skill proposal must state explicit applicability boundaries and counterexamples so future runs know both *when to apply* and *when not to apply* a method.
- **Structural vs. Effective Recursion**: Modifying the improvement mechanism (**Structural Recursion**) is tracked separately from proving that the new `Improver` produces higher-utility successor skills under equal budget, held-out generalization tasks, and non-regression checks on previously solved work (**Effective Recursion**).

---

## Autonomy Ladder (`B0–L5`)

These levels score **who controls improvement decisions and evolutionary depth**, not raw base-model IQ:

| Level | Name | In RSIAgent Terms |
| --- | --- | --- |
| **B0** | Intra-task correction | Fix the current output only — no lasting cross-session candidate |
| **L1** | Execution autonomy | Apply human-prescribed lasting updates (skills or policy patches) within governed bounds |
| **L2** | Strategy autonomy | Choose among allowed improvement methods (`evo.strategy.v1` Improver) |
| **L3** | Experience acquisition | Decide what to learn or practice next from `Feedback` failure classes and learner state |
| **L4** | Environment adaptation | Continuously adjust from four-stage `Application` receipts (`Attached → Observed → VerifiedBenefit`) |
| **L5** | Recursive meta-improvement | Evolve the **Improver itself** — how experience is refined, filtered, and accepted — and inherit that procedure in subsequent generations |

---

## Research Basis & Engineering References

Grounded in the normative **RSIAgent v4.3 Engineering Implementation Plan**, this project advances an independent Rust implementation informed by the following research papers and open-source engineering systems:

- **[Dream-RSI: Recursive Self-Improvement through Evolving Worlds](https://arxiv.org/pdf/2609.14858v1)** *(Paper)*  
  [arXiv:2609.14858 · v1](https://arxiv.org/pdf/2609.14858v1): Informs generation–exploration decoupling, historical exploration replay, offline “dreaming” simulation, and strategy filtering.  
  Institutions: Google, Google DeepMind, University of Maryland (UMD), University of Virginia (UVA); authors include Tong Zheng, Zheng Zhang et al.

- **[The Last AI Built by Humans: Toward Genuine Recursive Self-Improvement](https://arxiv.org/pdf/2609.11873v2)** *(Paper)*  
  [arXiv:2609.11873 · v2](https://arxiv.org/pdf/2609.11873v2): Informs the B0–L5 RSI autonomy taxonomy, learner-state-conditioned experience acquisition (L3), and cross-generation inheritance & meta-refinement of the improvement procedure (L5).  
  Institutions: Shanghai Jiao Tong University, Theseus Labs, Tsinghua University, ByteDance, ModelBest, Xiaohongshu, Shanghai AI Lab, Humanlaya, Agent-Native Research Lab, Frontis.AI; authors include Zhiyuan Liu, Bowen Zhou, Guoliang Li, Fan Wu et al.

- **[Memory Beyond Recall](https://arxiv.org/abs/2606.09483) & [Tencent-Hunyuan/Hy-Memory](https://github.com/Tencent-Hunyuan/Hy-Memory)** *(Paper & Open-Source System)*  
  [arXiv:2606.09483](https://arxiv.org/abs/2606.09483) (EMNLP 2026) and Tencent Hunyuan's open-source [Hy-Memory / MemHub](https://github.com/Tencent-Hunyuan/Hy-Memory): Informs the System 1 / System 2 cognitive division, the two-layer `Session Fact ↔ Reusable Experience` pipeline with bidirectional provenance, the version-evolution diff-chain philosophy for coding trajectories, and the local-first single-writer cross-host runtime model.

- **[RSI-Harness](https://github.com/CosmosMind-ai/RSI-Harness/tree/33c4f8dfac4359987f2e814e187de67c332498de)** (pinned commit `33c4f8df`) & **[Microsoft SkillOpt](https://github.com/microsoft/SkillOpt/tree/79124b37e9a6371e13b753f8bcd7adb1e493ade1)** (pinned commit `79124b37`)  
  Inform versioned asset management, historical experience distillation, trajectory attribution, and bounded skill refinement; upstream runtimes are not transplanted wholesale, nor are upstream benchmark numbers claimed as proof of effect for this repository.

---

## Engineering Principles of Controlled Evolution

**A closed structural loop, independent benefit evidence, and a safely deployable product are three independent gates — none substitutes for the others.**

1. **One Runtime, Single-Writer Data Plane, Multi-Host Isolation**  
   A shared data-directory single writer serializes persistent mutations, paired with a safe read-only SQLite reader for inspection. Access is isolated by `namespace`, `actor`, and five explicit roles (`Agent` / `Host` / `Evaluator` / `Admin` / `Worker`).
2. **Experience Must Prove Real Downstream Utility**  
   The runtime separates `Returned → Attached → Observed → VerifiedBenefit` receipts, combines positive and negative evidence with preserved counterexamples, and rejects model self-reports as proof of learning.
3. **Predictions Must Face Held-Out Reality (Early Reject Allowed, Early Approve Forbidden)**  
   Historical replay and offline simulation serve bounded candidate screening only. Formal benefit requires independent evaluation across fixed protection anchors and rotated held-out tasks under min-sample, gain-floor, regression-cap, and full cost-ratio constraints.
4. **Evolution Never Bypasses Governance**  
   Root budgets, lease recovery, human approval, and atomic release govern every state transition. Candidates are restricted to evolvable text and `evo.strategy.v1` JSON — never arbitrary executable code, unsupervised auto-publishing, or unbudgeted paid model calls.

---

## Quick Start & Integration Flow

### 1. CLI & Service Entry Points (Current Tree)

```bash
git clone https://github.com/acosmi/RSIAgent.git && cd RSIAgent
cat docs/implementation-ledger.md
```

The CLI entry (`apps/rsia`) strictly separates agent tool surfaces from authenticated management and review operations (default bootstrap disables external model transport and uses `ToolOnly` capabilities; without configured HTTP authentication, sensitive requests fail closed with `503`):

- `rsia serve` — serves the authenticated HTTP API and remote MCP transport (`--bind` defaults to `127.0.0.1:7788`, `--data` to `rsia.sqlite3`, `--namespace` to `default`).
- `rsia mcp` — runs the four agent MCP tools over stdio with a startup-fixed Agent identity (`--data` defaults to `rsia.sqlite3`, `--namespace` to `default`, `--actor` to `stdio-agent`).
- `rsia manage <operation>` — submits or inspects authenticated management and evolution jobs (`--url` defaults to `http://127.0.0.1:7788`; requires `--auth-token` or `RSIA_MANAGEMENT_TOKEN`; supports `--request-file` and `--job-id`).
- `rsia review --kind <typed_candidate|stage_fact|formal_report> --id <id> [--detail metadata|exact]` — inspects stored candidates, stage facts, and formal reports over the authenticated read-only review path.

### 2. Target 7-Step Host Loop

1. **Start Runtime** — run RSIAgent over HTTP or stdio MCP.  
2. **Prepare** — submit task goal + host capabilities → receive a version-locked active skill snapshot.  
3. **Execute Task** — run the task inside your host agent without evolution-path blocking.  
4. **Feedback** — report outcome and failure class (reasoning, domain knowledge, tool-use, etc.).  
5. **Proposal** — learnable failures trigger asynchronous improvement jobs producing candidates with evidence, applicability bounds, and counterexamples.  
6. **Evaluate & Release** — independent `Evaluator` acceptance → `Approved` → `Canary` → `Active`.  
7. **Application Receipts** — subsequent runs record `Returned → Attached → Observed → VerifiedBenefit`.

### 3. Repository Layout

```text
apps/rsia           CLI & daemon entry (serve / mcp / manage / review)
crates/evo-core     Pure domain contracts, role permissions, state machines & acceptance rules
crates/evo-storage  Single-writer persistence, read-only inspection, lineage & budget ledger
crates/evo-engine   Controlled improvement jobs, replay evaluation & candidate evolution
crates/evo-mcp      MCP adapter (agent tools isolated from management surfaces)
crates/evo-http     Authenticated HTTP service & remote MCP transport
docs/               Implementation ledger, dependency register & operations notes
```

---

## Verification Route & Scope Boundaries

**G0–G5 staged gates** and **V001–V120 verification scenario families** advance scope by scope: starting from end-to-end configuration-priority localization tasks, then expanding to historical replay, curriculum learning, L5 meta-improvement controlled experiments, cross-host compatibility, fault recovery, and deployment safety.

This repository is a **runtime and contract surface**, not a model-weight training codebase or an official reproduction of the cited papers or reference repositories. We thank the respective researchers and open-source contributors.

**Present in this tree (scoped, through PR #184):**
- Workspace crates `evo-core`, `evo-storage`, `evo-engine`, `evo-mcp`, `evo-http`, and the `rsia` CLI (`serve` / `mcp` / `manage` / `review`)
- Domain contracts for roles, budgets, candidates, acceptance, and four-stage receipts
- Implementation ledger plus derived support index (`plan_version` `v4.3`, route not complete, legacy equivalence unverified)
- Shared data-directory single writer, safe read-only SQLite reader, single-source management registry, real `control.review` route, fail-closed seed classification, bad-clock persistence guard, pinned toolchain and locked build inputs, project license metadata and guards, redacted bound-request identity, and locked dependency license provenance

**Not claimed (as of 2026-10-16):**
- A complete E00–E18 route, or whole scenario families V099–V120 (those families remain `planned`)
- Independent second acceptance
- A live external model provider, paid experiments, a live external host, or a production sandbox
- Production approval, canary, or `Active` backed by non-production evaluation
- Measured effective recursion gains, or a formal production release (recorded increments are at `implemented_not_verified` or earlier; default monetary budget is `0`; E17 and E18 remain optional and disabled)

---

## License

RSIAgent is source-available under a research- and community-friendly non-commercial license (`source_available_personal_noncommercial_free`). The root `LICENSE` governs all grants: personal learning, academic and scientific research, educational use, benchmarking, and internal non-commercial technical evaluation (by individuals, universities, research institutions, non-profits, or engineering teams) are free of charge; open-source forking, academic reproduction artifacts, and non-commercial community sharing of modifications are permitted provided the original copyright notice and `LICENSE` are retained; deploying RSIAgent in commercial production operations, offering it as a paid or managed cloud/SaaS service, or embedding/distributing it in commercial products or revenue-generating services requires prior written authorization from Acosmi (fushihua@acosmi.com, https://acosmi.com). Third-party licenses and notices remain applicable to their corresponding material. Workspace crates remain unpublished.
