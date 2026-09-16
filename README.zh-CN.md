# RSIAgent

[English](README.md) | [中文](README.zh-CN.md)

把智能体反复踩的坑，变成可以**版本化、审计、验收、发布**的改进——而不是让模型失控地改写自己。

RSIAgent 是**独立的 Rust 递归改进运行时**。任务智能体通过 **MCP / HTTP / CLI** 接入：宿主回报结果，RSIAgent 提出窄范围技能候选，做独立验收，再灰度 / 生效，并用回执证明「真的用上了」。候选是可演进的文本 / 策略 JSON，不是任意可执行脚本。证据一律视为不可信数据，不当指令。

> **现状：** 早期实施（workspace `0.2.0`）。目前最完整的是 `evo-core` 领域契约；宿主服务与完整 CLI 仍在落地。[实施台账](docs/implementation-ledger.md) 是编译 / smoke / 进化证据的权威来源。**在台账登记实测收益之前，不要宣称递归性能收益。**

仓库：https://github.com/acosmi/RSIAgent

## 为什么这样设计

现在的 Agent 调参往往散落在 settings、CLI 参数、临时 prompt 和「上次好像有用」的记忆里——不可版本、不可 diff、不可转交。

RSIAgent 把**改进**当作一等对象：

```
任务智能体（任意宿主）
        │  Prepare / Feedback / Application 回执
        ▼
RSIAgent 运行时           ← 本仓库（Rust）
  evo-core 契约           ← 角色、预算、候选、验收
  evo-engine / storage    ← 作业、评估、持久化
  evo-mcp / evo-http      ← 接入面
```

两条目标不变量（随台账测试落地）：

1. **候选数据永远不能自我授权。** 角色（`Agent` / `Host` / `Evaluator` / `Admin` / `Worker`）决定谁可提议、验收、发布。  
2. **结构验收与效果验收分开报告。** 「长得像有效提案」≠「证明有收益」。

## 基本用法（今天 vs 目标）

### 今天

```bash
git clone https://github.com/acosmi/RSIAgent.git && cd RSIAgent
# 先读契约与台账——完整宿主服务仍在落地。
cat docs/implementation-ledger.md
```

CLI 入口（`apps/rsia`）目前只打印进行中引导，**不会**拉起完整服务。在写稳定 `cargo run` / 安装教程前，以台账为准。

### 目标宿主流程（设计）

运行时就绪后：

1. 启动 RSIAgent（HTTP / MCP）  
2. **Prepare** — 目标 + 能力 → 本轮 skills 快照  
3. 在你的智能体里跑任务  
4. **Feedback** — 成功 / 失败分类（推理、知识、工具…）  
5. 可学习失败触发改进作业 → **Proposal**（含反例）  
6. Evaluator 验收 → 批准 → 灰度 → Active  
7. **Application** 回执：Returned → Attached → Observed → VerifiedBenefit  

HTTP 默认绑定（启用后）：`127.0.0.1:7788`。

## 候选允许是什么

| 类型 | 含义 |
| --- | --- |
| Skill | 窄范围文本技能：证据 + 适用面 + 反例 |
| Improver | 可演进策略 JSON（`evo.strategy.v1`），不是可执行脚本 |

**默认禁止：** 未监督自动发布、任意代码执行、无预算的付费模型调用。

## 能力地图（契约）

以 `evo-core` 为准，随代码演进请再核对源码：

| 领域 | 覆盖 |
| --- | --- |
| 角色与上下文 | 命名空间 / 执行者 / 角色；所有权检查；候选不可自我授权 |
| 运行闭环 | `Prepare` → `Feedback` → `Proposal` → 候选状态机 → `Release` / 灰度 / Active |
| 作业与预算 | 排队 / 运行租约、取消、恢复；未配置预算则拒绝付费改进 |
| 验收策略 | 最小样本、收益门槛、回归上限、成本比、置信度 |
| 接入 | MCP（模型工具与管理能力分离）、HTTP、CLI |

候选状态含：Proposed → Validated → AcceptancePassed → Approved → Canary → Active（以及 Retired / Rejected / Revoked）。

## 仓库结构

```
apps/rsia           CLI / 进程入口（今日为引导）
crates/evo-core     纯契约与验收规则
crates/evo-storage  持久化
crates/evo-engine   受控改进与评估
crates/evo-mcp      MCP 适配
crates/evo-http     鉴权 HTTP + 远程 MCP 传输
docs/               实施台账与运维说明
```

## 当前状态

**进行中 / 树内已有**

- Workspace 与 CI 挂钩  
- 丰富的 `evo-core` 契约（角色、预算、候选、验收、回执）  
- 明确区分「编写 / 编译 / 测试 / 宿主验收 / 实测收益」的实施台账  

**尚未具备**

- 可当生产用的完整 CLI / HTTP / MCP 宿主  
- 「一键自我进化」且带公开效果证据的演示  
- 无台账支撑的递归性能收益宣称  

## 开发

```bash
# 在本地工具链与宿主 smoke 变绿之前，优先看 CI 与台账。
# 见 .github/workflows 与 docs/implementation-ledger.md
```

本仓库是**运行时与契约面**，不是训练 / 评测 / 权重更新代码库。

## 贡献

请对照 [`docs/implementation-ledger.md`](docs/implementation-ledger.md)。优先证据（编译、测试、宿主 smoke、实测收益），而不是只有接口草图。

## 许可

若已有 License 文件以之为准；否则在补充许可前请勿默认任意再分发条款。