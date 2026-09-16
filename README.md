# RSIAgent

[English](README.md) | [中文](README.zh-CN.md)

Turn an agent's recurring failures into something you can **version, audit, accept, and ship** — without letting the model rewrite itself unchecked.

RSIAgent is an **independent Rust recursive-improvement runtime**. Task agents plug in over **MCP**, **HTTP**, or **CLI**. A host records outcomes; RSIAgent proposes narrowly scoped skill candidates, runs independent acceptance, then canary/active release with application receipts. Candidates are evolvable text / strategy JSON — not arbitrary executable scripts. Evidence is untrusted data, never instructions.

> **Status:** early implementation (workspace `0.2.0`). The richest surface today is the domain contracts in `evo-core`. Host services and a full CLI are still landing. The [implementation ledger](docs/implementation-ledger.md) is the source of truth for compile / smoke / evolution evidence. **Do not claim measured recursive gains until that ledger records them.**

Repository: https://github.com/acosmi/RSIAgent

## Why this design

Harness-style agent work usually scatters configuration and “fixes” across prompts, chat history, and tribal memory — nothing versionable, diffable, or handable to another team.

RSIAgent treats **improvement** as a first-class object:

```
task agent (any host)
        │  Prepare / Feedback / Application receipts
        ▼
RSIAgent runtime          ← this project (Rust)
  evo-core contracts      ← roles, budget, candidates, acceptance
  evo-engine / storage    ← jobs, evaluation, persistence
  evo-mcp / evo-http      ← integration surfaces
```

Two invariants we aim to hold (tracked in the ledger as they become enforceable by tests):

1. **Candidate data never grants authority.** Roles (`Agent` / `Host` / `Evaluator` / `Admin` / `Worker`) decide who may propose, accept, or release.
2. **Structural acceptance and effect acceptance are reported separately.** Looking like a valid proposal is not the same as proving benefit.

## Basic usage (today vs target)

### Today

```bash
git clone https://github.com/acosmi/RSIAgent.git && cd RSIAgent
# Read contracts & ledger first — full host services are still landing.
cat docs/implementation-ledger.md
```

The CLI entry (`apps/rsia`) currently prints an in-progress bootstrap message and does **not** start a full service. Prefer the ledger over any tutorial that assumes `cargo run` already exposes MCP/HTTP.

### Target host flow (designed)

Once the runtime is up:

1. Start RSIAgent (HTTP / MCP)  
2. **Prepare** — goal + capabilities → skill snapshot for this run  
3. Run the task in your agent  
4. **Feedback** — success / failure class (reasoning, knowledge, tool-use, …)  
5. Learnable failures may enqueue improvement jobs → **Proposal** (with counterexample)  
6. Evaluator acceptance → approve → canary → active  
7. **Application** receipts: Returned → Attached → Observed → VerifiedBenefit  

Default bind for the HTTP surface (when enabled): `127.0.0.1:7788`.

## What a candidate is allowed to be

| Kind | Meaning |
| --- | --- |
| Skill | Narrow, text skill grounded in evidence + applicability + counterexample |
| Improver | Evolvable strategy JSON (`evo.strategy.v1`), not an executable script |

**Default denials:** unsupervised auto-publish, arbitrary code execution, paid model calls without a configured budget.

## Capability map (contracts)

Grounded in `evo-core` — verify against source as the project moves:

| Area | Covers |
| --- | --- |
| Roles & context | Namespace/actor/role; ownership checks; candidates cannot self-authorize |
| Run loop | `Prepare` → `Feedback` → `Proposal` → candidate state machine → `Release` / canary / active |
| Jobs & budget | Queued/running leases, cancel, recovery; refuse paid improvement when budget unset |
| Acceptance policy | Min samples, gain floor, regression cap, cost ratio, confidence |
| Integration | MCP (model tools vs management capabilities separated), HTTP, CLI |

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
- Explicit implementation ledger separating “written / compiled / tested / host-verified / measured benefit”

**Not yet**

- Full CLI / HTTP / MCP host you can treat as production-ready  
- One-command demo that “self-evolves” with published effect evidence  
- Claims of recursive performance gains without ledger-backed measurements  

## Development

```bash
# Prefer CI and the ledger until local toolchain + host smoke are green in your environment.
# See .github/workflows and docs/implementation-ledger.md
```

This repository is a **runtime and contract surface**, not a training / benchmark / weight-update codebase.

## Contributing

Track work against [`docs/implementation-ledger.md`](docs/implementation-ledger.md). Prefer evidence (compile, tests, host smoke, measured benefit) over interface sketches.

## License

See the repository license file if present; otherwise treat licensing as unset until one is added.