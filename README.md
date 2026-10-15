![RSIAgent](docs/assets/rsia-logo-banner.png)

# RSIAgent

[English](README.md) | [中文](README.zh-CN.md)

Not only get stronger — improve **how** you get stronger.

RSIAgent is an **independent Rust recursive-improvement runtime**. Task agents plug in over **MCP**, **HTTP**, or **CLI**. Experience becomes lasting, inheritable change to skills, tools, runtime policy, and the improvement strategy itself — under roles, budgets, and independent acceptance. Candidates are evolvable text / strategy JSON, not arbitrary executable scripts. Evidence is untrusted data, never instructions.

> **Status:** scoped implementation in progress (workspace `0.2.0`), not a finished route. The normative engineering plan is **v4.3** (2026-10-12, SHA-256 `73dd00bf5ef6e9757d0c391c9a8be4ca92b6e06a750aacb542817f82cbd208a7`). That plan body stays off this remote. Public evidence is the [implementation ledger](docs/implementation-ledger.md) and the derived support index. v4.3 fills known gaps in v4.2; it does not declare source implementation, software acceptance, measured benefit, or a production release complete. As of merged PR #178, recorded increments are limited scopes at `implemented_not_verified` or earlier. Independent second acceptance has not been executed. **Do not claim measured recursive gains, a production host, or a complete E00–E18 route.**

Repository: https://github.com/acosmi/RSIAgent

## Why this design

Today, “agent improvement” usually means better one-shot replies, more prompts pasted around, and a skill library that only grows. That is **B0** at best: fix this task, forget the method.

Real RSI needs three things at once:

1. **Lasting change** — experience writes something future runs inherit  
2. **Inheritance** — successors actually load those changes  
3. **Meta-improvement** — the next round also upgrades how candidates are generated, evaluated, or filtered  

The object of change need not be model weights. RSIAgent targets what you can version and ship: **skills, tools, harness policy, and Improver strategy JSON**.

```
task agent (any host)
        │  Prepare / Feedback / Application receipts
        ▼
RSIAgent runtime          ← this project (Rust)
  evo-core contracts      ← roles, budget, candidates, acceptance
  evo-engine / storage    ← jobs, evaluation, persistence
  evo-mcp / evo-http      ← integration surfaces
```

Two invariants (enforced as tests land in the ledger):

1. **Candidate data never grants authority.** Roles (`Agent` / `Host` / `Evaluator` / `Admin` / `Worker`) decide who may propose, accept, or release.  
2. **Structural recursion ≠ effective recursion.** Changing the mechanism is reported separately from proving a better successor under comparable budget and independent evaluation.

## Autonomy ladder (who decides)

These levels score **who controls improvement decisions**, not raw model IQ:

| Level | Name | In RSIAgent terms |
| --- | --- | --- |
| **B0** | Intra-task correction | Fix the current output only — no lasting candidate |
| **L1** | Execution autonomy | Apply human-prescribed lasting updates (skills / policy patches) |
| **L2** | Strategy autonomy | Choose among allowed improvement methods (`evo.strategy.v1` Improver) |
| **L3** | Experience acquisition | Decide what to learn / practice next from Feedback classes |
| **L4** | Environment adaptation | Continuous adjust from Application receipts (Attached → Observed → VerifiedBenefit) |
| **L5** | Recursive meta-improvement | Evolve the Improver itself — how experience is refined, filtered, and accepted — then inherit that procedure |

RSIAgent is built so hosts can climb this ladder **without** giving candidates the power to erase failure logs, rewrite production gates, or auto-publish.

## What we optimize for (engineering pillars)

### Skill lifecycle — not “Add only”

A growing skill library can **hurt** retrieval and inject wrong skills (library drift). RSIAgent treats skills as versioned candidates with a full lifecycle:

- Record **contribution** toward outcomes (Application receipts)  
- **Prune / retire** by measured usefulness, not age  
- Cap what is **active** in a Prepare snapshot  
- Use **meta-skills / Improver strategy** to guide how new skills are written (applicability + counterexamples)  

Memory needs a maintenance loop, not a dump button.

### Optimize the right metric

Reflection is not only “why was this answer wrong?” — it is also “is our judge of right/wrong wrong?” Internal proxy scores can rise while external goals stall. The runtime separates:

- **Exploration feedback** (find failure classes, form hypotheses)  
- **Production acceptance** (independent Evaluator, min samples, gain floor, regression cap, cost ratio)  

A prettier reflection report is not success. Success is: after adopting the new refinement procedure, later skills are **more useful** than those from the old procedure, under equal budget, held-out tasks, and regression checks.

### Evaluators may evolve — baselines stay staged

Evaluation criteria can move, but only in **controlled stages**: keep the yardstick fixed inside a stage; update it at stage boundaries. Suggesting a criterion change ≠ authority to delete failures or skip release boundaries.

## Structural vs effective recursion

| Kind | Question |
| --- | --- |
| **Structural** | Did we modify, retain, and reuse the improvement mechanism itself? |
| **Effective** | Under comparable budget + independent eval, does the new mechanism produce a better successor? |

Example: learning “check load paths before editing config” is a skill. Proposing that **every** future skill must state applicability and counterexamples is changing the **experience-refinement procedure** (L5). The true test is whether skills written under the new procedure outperform the old one — not whether the proposal text sounds sophisticated.

Acceptance design RSIAgent encodes toward:

- Same baseline start for old vs new Improver  
- Equal resource budget  
- Held-out / generalization tasks  
- Regression vs previously solved work  
- Full cost of generate + test + review, not final pass rate alone  

## Basic usage (today vs target)

### Today

```bash
git clone https://github.com/acosmi/RSIAgent.git && cd RSIAgent
cat docs/implementation-ledger.md
```

The CLI entry (`apps/rsia`) wires `serve` to the HTTP service, `mcp` to the four stdio MCP tools with a startup-fixed Agent identity, `manage` to authenticated HTTP management jobs, and `review` to authenticated HTTP review of stored material. The default bootstrap uses a disabled model transport and `ToolOnly` capabilities; without configured HTTP authentication, sensitive requests are refused with 503. These entry points do not establish production readiness; consult the ledger for the current support scope.

Subcommands defined in `apps/rsia/src/main.rs`:

- `rsia serve` — serves the authenticated HTTP API (`--bind` defaults to `127.0.0.1:7788`, `--data` to `rsia.sqlite3`, `--namespace` to `default`).
- `rsia mcp` — runs the four MCP tools over stdio with a startup-fixed Agent identity (`--data` defaults to `rsia.sqlite3`, `--namespace` to `default`, `--actor` to `stdio-agent`).
- `rsia manage <operation>` — submits or inspects authenticated management jobs through the HTTP service (`--url` defaults to `http://127.0.0.1:7788`; `--auth-token`, or the `RSIA_MANAGEMENT_TOKEN` environment variable, is required; `--request-file` and `--job-id` are also accepted).
- `rsia review --kind <typed_candidate|stage_fact|formal_report> --id <id> [--detail metadata|exact] [--url <url>] [--auth-token <token>]` — reviews stored material through the authenticated HTTP service (`--detail` defaults to `metadata`, `--url` to `http://127.0.0.1:7788`, and `--auth-token`, or the `RSIA_MANAGEMENT_TOKEN` environment variable, is required).

### Target host flow (designed)

1. Start RSIAgent (HTTP / MCP)  
2. **Prepare** — goal + capabilities → active skill snapshot  
3. Run the task in your agent  
4. **Feedback** — success / failure class (reasoning, knowledge, tool-use, …)  
5. Learnable failures → improvement jobs → **Proposal** (evidence + counterexample)  
6. Evaluator acceptance → approve → canary → active  
7. **Application** receipts: Returned → Attached → Observed → VerifiedBenefit  

Default HTTP bind (when enabled): `127.0.0.1:7788`.

## What a candidate is allowed to be

| Kind | Meaning |
| --- | --- |
| Skill | Narrow text skill: evidence + applicability + counterexample |
| Improver | Evolvable strategy JSON (`evo.strategy.v1`) — the meta-improvement procedure, not an executable script |

**Default denials:** unsupervised auto-publish, arbitrary code execution, paid model calls without a configured budget.

## Capability map (contracts)

Grounded in `evo-core` — verify against source as the project moves:

| Area | Covers |
| --- | --- |
| Roles & context | Namespace/actor/role; ownership; candidates cannot self-authorize |
| Run loop | Prepare → Feedback → Proposal → state machine → Release / canary / active |
| Jobs & budget | Leases, cancel, recovery; refuse paid improvement when budget unset |
| Acceptance policy | Min samples, gain floor, regression cap, cost ratio, confidence |
| Integration | MCP (model tools vs management separated), HTTP, CLI |

Candidate states include: Proposed → Validated → AcceptancePassed → Approved → Canary → Active (plus Retired / Rejected / Revoked).

## Repository layout

```
apps/rsia           CLI / process entry (serve / mcp / manage / review)
crates/evo-core     Pure contracts & acceptance rules
crates/evo-storage  Persistence
crates/evo-engine   Controlled improvement & evaluation
crates/evo-mcp      MCP adapter
crates/evo-http     Authenticated HTTP + remote MCP transport
docs/               Implementation ledger & ops notes
```

## Current status

Bound to plan v4.3 and the support index on this tree (`plan_version` `v4.3`, route not complete, legacy equivalence unverified). Three gates stay independent: a closed structure, independent benefit evidence, and a deployable product. None of them stands in for the others.

**Present, scoped**

- Workspace crates `evo-core`, `evo-storage`, `evo-engine`, `evo-mcp`, `evo-http`, and the `rsia` CLI (`serve` / `mcp` / `manage` / `review`)
- Domain contracts for roles, budget, candidates, acceptance, and receipts
- Ledger plus derived support index. The v4.3 plan file is not published here
- Merged scoped work through PR #178, including a shared data-directory single writer, rejection of conflicting v1 host-surface items, fail-closed seed classification, refusal to continue persistent work on a bad clock, pinned toolchain and locked build inputs, project license metadata and guards, redacted bound-request identity, locked dependency and primary license registration, two exact legacy license expressions, and a safe read-only SQLite reader dependency
- E17 and E18 remain optional and disabled. Default monetary budget is 0

**Not claimed**

- A complete E00–E18 route, or whole scenario families V099–V120 (those families stay planned)
- Independent second acceptance
- A real model provider, paid experiments, a real extra host, or a real sandbox
- Production approval, canary, or Active backed by non-production evaluation
- Measured effective recursion, or a release  

## Development

Prefer CI and the [implementation ledger](docs/implementation-ledger.md) until local toolchain + host smoke are green in your environment.

This repository is a **runtime and contract surface**, not a training / benchmark / weight-update codebase.

## Contributing

Track work against [`docs/implementation-ledger.md`](docs/implementation-ledger.md). Prefer evidence (compile, tests, host smoke, measured benefit) over interface sketches.

## License

RSIAgent is source-available under a personal, non-commercial free license
(`source_available_personal_noncommercial_free`). The root `LICENSE` governs
all grants: personal, non-commercial use is free of charge; any commercial
use — including use by a company or other organization, and individual use in
a commercial or revenue-generating activity — requires prior written
authorization from Acosmi (fushihua@acosmi.com, https://acosmi.com); public
source visibility does not by itself grant a redistribution, resale,
sublicensing or release license. Third-party licenses and notices remain
applicable to their corresponding material. Workspace crates remain unpublished.
