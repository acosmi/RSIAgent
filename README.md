# RSIAgent

[English](README.md) | [中文](README.zh-CN.md)

**Independent Rust recursive-improvement runtime** for task agents.

Any agent host can integrate over **MCP**, **HTTP**, or **CLI** to turn repeatable failures into *auditable* skill improvements — without letting a model rewrite itself unchecked.

> **Status (honest):** early implementation (workspace `0.2.0`). Domain contracts in `evo-core` are the most complete part. Host services / CLI bootstrap are still landing. Do **not** treat this as proven recursive performance gains until the [implementation ledger](docs/implementation-ledger.md) records runnable verification and measured benefit.

Repository: https://github.com/acosmi/RSIAgent

## Why it exists

Typical loop today:

1. Agent fails  
2. Human edits prompts by hand  
3. The next task fails in a new way  

RSIAgent productizes a safer loop:

**record failure → propose narrow skill candidates → independent acceptance → approve / canary → prove application with receipts**

Candidates are evolvable **text / strategy JSON**, not arbitrary executable scripts. Evidence is treated as **untrusted data**, never as instructions.

## Designed capabilities

Grounded in the `evo-core` contracts (verify against source as the project moves):

| Area | What it covers |
| --- | --- |
| Roles | `Agent` / `Host` / `Evaluator` / `Admin` / `Worker` — candidate payloads never grant themselves authority |
| Run loop | `Prepare` → `Feedback` → `Proposal` → candidate state machine → `Release` / canary / active |
| Jobs & budget | Leases, cancel, recovery; refuse paid improvement when budget is unset |
| Acceptance | Min samples, gain floor, regression cap, cost ratio, confidence — structural vs effect acceptance reported separately |
| Integration surfaces | MCP (model tools vs management capabilities separated), HTTP (default bind `127.0.0.1:7788`), CLI |

**Default denials:** unsupervised auto-publish, arbitrary code execution, paid model calls without a configured budget.

## How to use (target host flow)

Intended host-agent flow once the runtime is up:

1. Start the RSIAgent runtime (HTTP / MCP)  
2. **Prepare** before a task (goal + capabilities) → receive skill snapshot  
3. **Feedback** after the task (outcome + failure class: reasoning / knowledge / tool-use / …)  
4. Learnable failures may enqueue improvement jobs → **Proposal** (with counterexample)  
5. Evaluator acceptance → approve → canary → active  
6. **Application** receipts: Returned → Attached → Observed → VerifiedBenefit  

### What works today

- Read this README and [`docs/implementation-ledger.md`](docs/implementation-ledger.md) for scope and exit criteria  
- Inspect crate layout and contracts under `crates/evo-core`  
- Follow CI / ledger for compile, protocol smoke, and evolution-evidence gates  

### Not ready to claim yet

- Production “clone and it self-evolves” demos  
- Measured recursive performance gains without ledger evidence  

The CLI entry (`apps/rsia`) currently bootstraps with an in-progress message and does not start a full service — check the ledger before documenting install/run commands as stable.

## Repository layout

```
apps/rsia          CLI / process entry
crates/evo-core    Pure contracts & acceptance rules
crates/evo-storage Persistence
crates/evo-engine  Controlled improvement & evaluation
crates/evo-mcp     MCP adapter
crates/evo-http    Authenticated HTTP + remote MCP transport
docs/              Implementation ledger & ops notes
```

## Safety posture

- No other product dependency by design  
- Candidate content is not execution authority  
- Budget / lease / role checks are first-class  
- Structural acceptance and effect acceptance are reported separately  

## Contributing / tracking

Progress is tracked in [`docs/implementation-ledger.md`](docs/implementation-ledger.md). Prefer evidence (compile, tests, host smoke, measured benefit) over interface sketches.

## License

See repository license file if present; otherwise treat as unpublished licensing until one is added.