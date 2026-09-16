# RSIAgent

[English](README.md) | [中文](README.zh-CN.md)

Not only get stronger — improve **how** you get stronger.

RSIAgent is an **independent Rust recursive-improvement runtime**. Task agents plug in over **MCP**, **HTTP**, or **CLI**. Experience becomes lasting, inheritable change to skills, tools, runtime policy, and the improvement strategy itself — under roles, budgets, and independent acceptance. Candidates are evolvable text / strategy JSON, not arbitrary executable scripts. Evidence is untrusted data, never instructions.

> **Status:** early implementation (workspace `0.2.0`). The richest surface today is the domain contracts in `evo-core`. Host services and a full CLI are still landing. The [implementation ledger](docs/implementation-ledger.md) is the source of truth for compile / smoke / evolution evidence. **Do not claim measured recursive gains until that ledger records them.**

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

The CLI entry (`apps/rsia`) currently prints an in-progress bootstrap message and does **not** start a full service. Prefer the ledger over tutorials that assume `cargo run` already exposes MCP/HTTP.

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
apps/rsia           CLI / process entry (bootstrap today)
crates/evo-core     Pure contracts & acceptance rules
crates/evo-storage  Persistence
crates/evo-engine   Controlled improvement & evaluation
crates/evo-mcp      MCP adapter
crates/evo-http     Authenticated HTTP + remote MCP transport
docs/               Implementation ledger & ops notes
```

## Current status

**In progress / present in tree**

- Workspace layout and CI hooks  
- Rich `evo-core` domain contracts (roles, budget, candidates, acceptance, receipts)  
- Explicit ledger separating written / compiled / tested / host-verified / measured benefit  
- Design coverage for L1–L5 autonomy, skill lifecycle, staged acceptance, structural vs effective recursion  

**Not yet**

- Full CLI / HTTP / MCP host you can treat as production-ready  
- One-command demo with published **effective**-recursion evidence  
- Ledger-backed measured recursive gains  

## Development

Prefer CI and the [implementation ledger](docs/implementation-ledger.md) until local toolchain + host smoke are green in your environment.

This repository is a **runtime and contract surface**, not a training / benchmark / weight-update codebase.

## Contributing

Track work against [`docs/implementation-ledger.md`](docs/implementation-ledger.md). Prefer evidence (compile, tests, host smoke, measured benefit) over interface sketches.

## License

See the repository license file if present; otherwise treat licensing as unset until one is added.
