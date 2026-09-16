# RSIAgent

[English](README.md) | [中文](README.zh-CN.md)

**独立的 Rust 递归改进运行时**，面向任务智能体。

任意宿主智能体可通过 **MCP / HTTP / CLI** 接入，把可复现的失败变成**可审计**的技能改进——而不是让模型失控地改写自己。

> **现状（如实）：** 早期实施阶段（workspace `0.2.0`）。`evo-core` 领域契约相对完整；宿主服务 / CLI 仍在落地。在 [实施台账](docs/implementation-ledger.md) 登记可运行验证与实测收益之前，**不要宣称已获得递归性能收益**。

仓库：https://github.com/acosmi/RSIAgent

## 解决什么问题

常见现状：

1. 智能体失败  
2. 人肉改 prompt  
3. 换个任务又挂  

RSIAgent 要把这条链路产品化：

**记录失败 → 提出窄范围技能候选 → 独立验收 → 批准 / 灰度 → 用回执证明「真的用上了、且有收益」**

候选是可演进的 **文本 / 策略 JSON**，不是任意可执行脚本。证据一律视为**不可信数据**，不当指令。

## 具备什么能力（设计）

以 `evo-core` 契约为准（随代码演进请再核对源码）：

| 领域 | 内容 |
| --- | --- |
| 角色 | `Agent` / `Host` / `Evaluator` / `Admin` / `Worker` — 候选数据不能自我授权 |
| 运行闭环 | `Prepare` → `Feedback` → `Proposal` → 候选状态机 → `Release` / 灰度 / Active |
| 作业与预算 | 租约、取消、恢复；未配置预算则拒绝付费改进 |
| 验收 | 最小样本、收益门槛、回归上限、成本比、置信度 — 结构验收与效果验收分开报告 |
| 接入面 | MCP（模型工具与管理能力分离）、HTTP（默认 `127.0.0.1:7788`）、CLI |

**默认禁止：** 未监督自动发布、任意代码执行、无预算的付费模型调用。

## 如何使用（目标宿主流程）

运行时就绪后的目标流程：

1. 启动 RSIAgent 运行时（HTTP / MCP）  
2. 任务前 **Prepare**（目标 + 能力）→ 拿回 skills 快照  
3. 任务后 **Feedback**（结果 + 失败分类：推理 / 知识 / 工具…）  
4. 可学习失败触发改进作业 → **Proposal**（含反例）  
5. Evaluator 验收 → 批准 → 灰度 → Active  
6. **Application** 回执：Returned → Attached → Observed → VerifiedBenefit  

### 今天可以做什么

- 阅读本 README 与 [`docs/implementation-ledger.md`](docs/implementation-ledger.md)  
- 查看 `crates/evo-core` 契约与仓库结构  
- 以 CI / 台账为准跟踪编译、协议 smoke、进化证据门槛  

### 尚不宜宣称

- 「clone 即自我进化」的生产级演示  
- 无台账证据的递归性能收益  

CLI 入口（`apps/rsia`）目前仍是进行中引导输出，不会拉起完整服务——在文档中写稳定安装/启动命令前请先核对台账。

## 仓库结构

```
apps/rsia          CLI / 进程入口
crates/evo-core    纯契约与验收规则
crates/evo-storage 持久化
crates/evo-engine  受控改进与评估
crates/evo-mcp     MCP 适配
crates/evo-http    鉴权 HTTP + 远程 MCP 传输
docs/              实施台账与运维说明
```

## 安全姿态

- 设计上不依赖其他产品  
- 候选内容 ≠ 执行权限  
- 预算 / 租约 / 角色检查为一等公民  
- 结构验收与效果验收分开报告  

## 贡献与跟踪

进度见 [`docs/implementation-ledger.md`](docs/implementation-ledger.md)。优先提交证据（编译、测试、宿主 smoke、实测收益），而不是只有接口草图。

## 许可

若仓库已有 License 文件以之为准；否则在补充许可前请勿默认任意再分发条款。