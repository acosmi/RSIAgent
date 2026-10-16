<p align="center">
  <img src="docs/assets/rsia-logo-banner.png" alt="RSIAgent" width="100%">
</p>

<h1 align="center">RSIAgent</h1>

<p align="center">
  <b>万物皆可 RSI —— 面向跨域应用的通用受控递归自我改进运行时</b><br>
  不仅把当前任务做得更好，更持续改进「如何变得更强」。
</p>

<p align="center">
  <a href="#核心架构双环认知与受控进化"><img src="https://img.shields.io/badge/%E6%9E%B6%E6%9E%84-%E5%8F%8C%E7%8E%AF%E8%AE%A4%E7%9F%A5%E4%B8%8E%E8%BF%9B%E5%8C%96-007ec6?style=flat-square" alt="核心架构"></a>
  <a href="#双层事实资产管线与版本演进链"><img src="https://img.shields.io/badge/%E6%95%B0%E6%8D%AE%E9%9D%A2-%E4%BA%8B%E5%AE%9E%E4%B8%8E%E8%B5%84%E4%BA%A7%E5%8F%8C%E5%B1%82%E7%AE%A1%E7%BA%BF-00897b?style=flat-square" alt="双层管线"></a>
  <a href="#自主程度阶梯b0l5"><img src="https://img.shields.io/badge/%E8%87%AA%E4%B8%BB%E9%98%B6%E6%A2%AF-B0--L5-149954?style=flat-square" alt="B0-L5 自主阶梯"></a>
  <a href="#研究基础与工程借鉴"><img src="https://img.shields.io/badge/%E7%A0%94%E7%A9%B6%E5%80%9F%E9%89%B4-Papers%20%26%20Systems-68448b?style=flat-square" alt="研究与工程借鉴"></a>
  <a href="#快速体验与接入流程"><img src="https://img.shields.io/badge/%E6%8E%A5%E5%85%A5-MCP%20%7C%20HTTP%20%7C%20CLI-f28c45?style=flat-square" alt="接入方式"></a>
  <a href="docs/implementation-ledger.md"><img src="https://img.shields.io/badge/%E8%AF%81%E6%8D%AE%E5%8F%B0%E8%B4%A6-Ledger-e65100?style=flat-square" alt="实施台账"></a>
  <a href="https://acosmi.com"><img src="https://img.shields.io/badge/%E5%AE%98%E7%BD%91-Acosmi-0052d9?style=flat-square" alt="Acosmi"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/%E8%AE%B8%E5%8F%AF-Non--Commercial%20Open-4c6ef5?style=flat-square" alt="License"></a>
</p>

<p align="center">
  <a href="README.md">English</a> | <b>中文</b>
</p>

**RSIAgent** 是基于 **Rust** 构建的独立受控递归自我改进运行时（Controlled Recursive Self-Improvement Runtime）。它将外部智能体的「在线业务执行」与「离线自我进化治理」彻底解耦：各类编码助手、通用智能体与可信宿主（如 Claude Code、Cursor、Codex、Antigravity、OpenCode 及自研业务 Agent）通过 **MCP、HTTP 与 CLI** 接入同一本地优先运行时与受控数据目录，在角色授权、预算边界与独立验收门禁下，实现**自主提出候选 → 预测后果 → 选择执行 → 从真实反馈中学习 → 跨代保留改进**的进化闭环。**你只需专注业务本身，无需为每个场景反复设计和打磨自我增强逻辑。**

> **工程基线：** v4.3 第一真源工程实施方案（2026-10-12，SHA-256 `73dd00bf5ef6e9757d0c391c9a8be4ca92b6e06a750aacb542817f82cbd208a7`）。  
> **当前状态：** 截至 2026-10-16，核心工程约完成 **80%**（维护者估算），即将进入全链路验证测试阶段；工程进度不等于验收通过或收益成立。公开证据见[实施台账](docs/implementation-ledger.md)与派生支持索引。

---

## 核心架构：双环认知与受控进化

传统 Agent 的「自我改进」往往停留在单次反射提示词拼接或只增不减的技能堆砌——不仅拉高在线时延，还会因错误经验混入导致检索漂移（Library Drift）。RSIAgent 借鉴认知科学的**双系统（System 1 快执行 + System 2 慢演进）**分工，将运行时划分为两条速率与权限截然不同的闭环：

```text
外部智能体 / 多宿主客户端 (Claude Code / Cursor / Codex / Antigravity / Custom Agents)
        │  MCP (stdio/remote) · HTTP · CLI
        ▼
┌──────────────────────────────────────────────────────────────────────┐
│ RSIAgent Runtime (Rust) — 一次部署 · 单一运行时 · 单写者受控数据面   │
│                                                                      │
│  [System 1 · 在线快路径]  Prepare 快照装配 ──► 任务执行 ──► Feedback │
│                           与 Application 四阶段回执（零进化阻塞）     │
│                                    │                                 │
│                                    ▼ 不可信证据入库（单写者事实层）   │
│  [System 2 · 离线慢演进]  可学习失败归因 ──► 历史回放 / 离线仿真      │
│                           ──► 候选生成与反例消解 ──► 独立 Evaluator  │
│                           ──► 人工审批 / 灰度 Canary / 原子 Active    │
└──────────────────────────────────────────────────────────────────────┘
```

| 维度 | System 1：在线任务执行环（快路径） | System 2：受控递归进化环（慢演进） |
| --- | --- | --- |
| **核心职责** | 依据当前任务目标与宿主能力，装配有界活跃技能快照并收集真实执行轨迹 | 从累积轨迹中识别可学习失败，离线生成、回放、筛选并验收新候选资产 |
| **调用开销** | 默认零额外模型调用（`ToolOnly` 快照装配），不阻塞宿主业务关键路径 | 异步作业调度，严格受根预算（Budget）与租约（Lease）上限约束 |
| **操作对象** | 冻结的只读活跃快照（Active Snapshot）与追加写入的阶段事实 / 回执 | 版本化 **Skill**（任务方法）与 **Improver**（元改进策略 `evo.strategy.v1`） |
| **安全边界** | 外部输入与轨迹一律视为**不可信数据**，绝不作为指令直接执行或自授权 | 独立 `Evaluator` 对照验收 + 审批 / 灰度 / 原子发布；候选永不自批自发 |

---

## 双层「事实—资产」管线与版本演进链

受长程生产力与工程记忆架构启发，RSIAgent 拒绝「只存笼统总结」或「只增不减堆砌技能」，而是将每一次任务交互沉淀为**双向溯源的双层结构**，并配套**版本演进与状态链**：

### 1. 阶段事实层 ↔ 可复用进化资产层

| 层级 | 记录内容 | 准入策略 | 核心结构与治理要求 |
| --- | --- | --- | --- |
| **事实层（Stage Facts & Receipts）** | 本次任务实际发生了什么：目标、工具轨迹、成功/失败分类、四阶段生效回执 | **零门槛完整留存**（Append-only 不可篡改事实） | 区分 `Returned → Attached → Observed → VerifiedBenefit` 四阶段回执，如实记录成功与失败证据 |
| **资产层（Evolvable Assets）** | 下次遇到同类任务或同类改进瓶颈时可复用的**技能（Skill）**与**元改进策略（Improver）** | **高门槛受控准入**（须经反例校验与独立对照验收） | **Skill**：窄范围文本技能（证据 + 适用边界 + 反例）<br>**Improver**：可演进策略 JSON（`evo.strategy.v1`，改进「如何提炼、过滤和生成经验」） |

- **双向血缘追溯与级联撤销**：每一份派生候选（`Skill` / `Improver`）均强绑定上游事实与证据 ID；一旦上游证据被发现污染或撤销，系统沿血缘链自动执行级联撤销与清理修复，确保活跃资产库随时可审计、可回滚。
- **全生命周期技能治理**：技能不再「只增不减」。运行时持续追踪技能在真实任务中的贡献度（Application 回执），按实测效用淘汰或归档低效资产，并严格限制单次 `Prepare` 快照的挂载上限。

### 2. 为什么需要版本演进链与反例保留（拒绝「总结即学习」）

在长程编码与复杂工具调用中，单纯的模型总结必然丢失**曾尝试但被回退的失败方案**以及**不再处于当前 HEAD 的中间演进细节**。RSIAgent 在进化链路中强制保留完整演进脉络：

- **有界原子编辑与版本演进链**：候选资产沿 `Proposed → Validated → AcceptancePassed → Approved → Canary → Active`（及 `Retired / Rejected / Revoked`）形成不可篡改的版本演进链，精确记录每次策略或技能修订的差分与原因，而非用新文本直接覆盖历史。
- **反例（Counterexample）一等公民**：任何新 Skill 除声明适用条件外，必须显式保留反例与失效边界，防止旧坑重踩。
- **结构性递归 ≠ 有效性递归**：修改了改进机制（Structural Recursion）不代表变得更强；只有当新 `Improver` 在同等资源预算、固定保护锚点与独立留出任务（Held-out Tasks）上产生实测更优的后继资产且不退化历史已解任务时，才承认有效性递归（Effective Recursion）。

---

## 自主程度阶梯（B0–L5）

阶梯评价的是**谁控制改进决策与演进深度**，而非底层模型的基础智商：

| 层级 | 名称 | 在 RSIAgent 中的工程落点 |
| --- | --- | --- |
| **B0** | 任务内纠错 | 仅修正当前会话输出，不产生任何跨会话持久候选 |
| **L1** | 执行自主 | 在受控边界内执行人类指定的持久更新（技能或策略补丁） |
| **L2** | 策略自主 | 在已开放的方法空间中自主选择改进手段（`evo.strategy.v1` Improver） |
| **L3** | 经验获取自主 | 根据 `Feedback` 失败分类与学习者当前状态，自主决定下一步学什么、练什么 |
| **L4** | 环境适应自主 | 依据 `Application` 四阶段回执（`Attached → Observed → VerifiedBenefit`）持续自适应调整 |
| **L5** | 递归元改进 | 演进 **Improver 本身**——升级「如何提炼、过滤、评估与验收经验」的程序，并由后继轮次实际继承 |

---

## 研究基础与工程借鉴

项目依据《**RSIAgent v4.3 定稿：受控递归自我改进系统工程实施方案**》这一唯一规范真源，参考以下前沿研究与开源工程体系推进独立 Rust 实现：

- **[Dream-RSI: Recursive Self-Improvement through Evolving Worlds](https://arxiv.org/pdf/2609.14858v1)**（论文）  
  [arXiv:2609.14858 · v1](https://arxiv.org/pdf/2609.14858v1)：借鉴生成与探索解耦、历史探索回放、离线「做梦」仿真与策略筛选机制。  
  参与机构：Google、Google DeepMind、马里兰大学（UMD）、弗吉尼亚大学（UVA）；作者包括 Tong Zheng、Zheng Zhang 等。

- **[The Last AI Built by Humans: Toward Genuine Recursive Self-Improvement](https://arxiv.org/pdf/2609.11873v2)**（论文）  
  [arXiv:2609.11873 · v2](https://arxiv.org/pdf/2609.11873v2)：参考 RSI 自主性分级框架（B0–L5），重点吸收学习者状态条件化的经验获取（L3），以及改进机制的跨代继承与元改良（L5）。  
  参与机构：上海交通大学、Theseus Labs、清华大学、字节跳动、面壁智能、小红书、上海人工智能实验室、Humanlaya、Agent-Native Research Lab、Frontis.AI；作者包括 Zhiyuan Liu（刘知远）、Bowen Zhou（周伯文）、Guoliang Li（李国良）、Fan Wu（吴帆）等。

- **[Memory Beyond Recall](https://arxiv.org/abs/2606.09483) & [Tencent-Hunyuan/Hy-Memory](https://github.com/Tencent-Hunyuan/Hy-Memory)**（论文与开源系统）  
  [arXiv:2606.09483](https://arxiv.org/abs/2606.09483)（EMNLP 2026）及腾讯混元开源的 [Hy-Memory / MemHub](https://github.com/Tencent-Hunyuan/Hy-Memory)：借鉴其 System 1 / System 2 双系统认知演进分工、生产力与编码场景的「会话事实 ↔ 可复用经验」双层解耦与双向溯源、代码版本演进差分链（Diff Chain）设计，以及跨宿主单写者受控运行时的治理范式。

- **[RSI-Harness](https://github.com/CosmosMind-ai/RSI-Harness/tree/33c4f8dfac4359987f2e814e187de67c332498de)**（固定提交 `33c4f8df`）与 **[Microsoft SkillOpt](https://github.com/microsoft/SkillOpt/tree/79124b37e9a6371e13b753f8bcd7adb1e493ade1)**（固定提交 `79124b37`）  
  吸收版本化资产、历史经验提炼、轨迹归因分析与技能内容有界优化思路；不整体移植上游运行时，也不将上游实验结果视为本项目的效果证明。

---

## 受控进化的工程原则

**结构确实闭环、收益有独立证据、产品可安全部署——三项门禁相互独立，不能互相替代。**

1. **一次部署、单写者数据面、多宿主隔离复用**  
   共享数据目录强制单写者（Single Writer）串行化事务写入，配合安全只读 SQLite 审阅通道，杜绝多客户端并发写坏状态；按命名空间（`namespace`）、执行者（`actor`）与五类角色（`Agent` / `Host` / `Evaluator` / `Admin` / `Worker`）实施严格权限隔离。
2. **经验必须真正生效，而非停留在总结文本**  
   严格区分「已返回（Returned）→ 已挂载（Attached）→ 已观测使用（Observed）→ 已验证收益（VerifiedBenefit）」；结合正负向证据、反例保留和有界原子编辑，拒绝把模型自评或漂亮总结当成学习成果。
3. **预测必须接受现实检验，允许提前拒绝、禁止提前批准**  
   历史回放与离线仿真仅用于有限支持范围内的候选初筛；正式收益必须通过独立真实任务对照验证，采用固定保护锚点、轮换独立留出集、最小样本量、收益下限、回归上限与全链路成本比核算。
4. **改进绝不绕过治理边界**  
   以根预算、租约恢复、人工审批和原子发布约束全流程；候选仅限可演进文本与策略 JSON，默认禁止未授权代码执行、无预算付费模型调用与未监督自动发布。候选永远无权修改最终验收器、提高权限或预算上限。

---

## 快速体验与接入流程

### 1. CLI 与服务入口（当前树实现）

```bash
git clone https://github.com/acosmi/RSIAgent.git && cd RSIAgent
cat docs/implementation-ledger.md
```

CLI 入口（`apps/rsia`）将模型工具面与管理审阅面严格分离（默认引导禁用外部模型传输并使用 `ToolOnly` 能力；未配置 HTTP 鉴权时敏感请求以 `503` 失败关闭）：

- `rsia serve` — 启动鉴权 HTTP 与远程 MCP 服务（`--bind` 默认 `127.0.0.1:7788`，`--data` 默认 `rsia.sqlite3`，`--namespace` 默认 `default`）。
- `rsia mcp` — 通过 stdio 暴露 4 个智能体运行时 MCP 工具，启动时固定 Agent 身份（`--actor` 默认 `stdio-agent`）。
- `rsia manage <operation>` — 提交或查询经鉴权的管理与演进作业（需 `--auth-token` 或 `RSIA_MANAGEMENT_TOKEN`）。
- `rsia review --kind <typed_candidate|stage_fact|formal_report> --id <id> [--detail metadata|exact]` — 通过只读安全通道审阅已存候选、阶段事实与正式报告。

### 2. 标准宿主闭环（7 步受控演进）

1. **启动运行时**：以 HTTP 或 MCP 方式挂载 RSIAgent。  
2. **Prepare（快照装配）**：传入任务目标与宿主能力，获取当前版本锁定的活跃技能快照。  
3. **执行业务任务**：宿主智能体在自身环境中完成编码、分析或工具调用。  
4. **Feedback（反馈归类）**：上报执行结果与失败类别（推理、领域知识、工具调用等）。  
5. **Proposal（异步提炼）**：可学习失败触发后台演进作业，生成携带证据、适用面与反例的候选资产。  
6. **Evaluation & Release（验收与发布）**：独立 Evaluator 验收通过 → 审批（Approved）→ 灰度（Canary）→ 激活（Active）。  
7. **Application（生效回执）**：后续任务回传 `Returned → Attached → Observed → VerifiedBenefit` 闭环证据。

### 3. 仓库结构

```text
apps/rsia           CLI 与守护进程入口 (serve / mcp / manage / review)
crates/evo-core     核心领域契约、角色权限、状态机与验收规则
crates/evo-storage  单写者持久化、只读审阅、血缘追溯与预算账本
crates/evo-engine   受控改进作业、回放评估与候选演进引擎
crates/evo-mcp      stdio / MCP 协议适配层（模型工具与管理能力隔离）
crates/evo-http     鉴权 HTTP 服务与远程 MCP 传输层
docs/               实施台账 (implementation-ledger.md)、依赖登记与运维文档
```

---

## 验证路线与边界声明

**G0–G5 分阶段门禁**与 **V001–V120 验收场景族**逐作用域推进：从配置优先级定位任务开始验证真实端到端闭环，再开展历史回放、课程学习与 L5 元改良对照实验，以及跨宿主兼容性、故障恢复和部署安全验证。

本项目并非上述论文或参考仓库的官方实现或完整复现。跨域通用性与循环进化是设计目标，实际支持范围和改进收益以独立验证结果为准。感谢相关研究者与开源社区贡献者。

**截至 2026-10-16，以下均不声称：**

- E00–E18 整线完成，或 V099–V120 整族验收（相关场景族仍为 `planned`）
- 独立二次验收（尚未执行）
- 真实模型提供商、付费实验、真实外部宿主或真实沙箱已接通验证
- 由非生产评价支撑的生产批准、灰度或 Active
- 实测有效递归收益，或正式生产发行

已登记增量状态最高为 `implemented_not_verified`；货币预算缺省为 `0`；开发评分器演化（E17）与自动代码修改（E18）仍为可选且默认关闭。

---

## 许可

RSIAgent 采用对学术与开发者友好的源码可用许可（`source_available_personal_noncommercial_free`）：

- **个人、学术研究、教学与非商业技术评估免费**：个人开发者、高校、科研院所、非营利机构及工程团队，均可出于个人学习、学术研究、教学演示、基准评测及内部非商业技术评估目的，免费使用、复制、修改和运行本项目。
- **允许开源 Fork、学术复现与非商业分享**：在完整保留原始版权声明与 [`LICENSE`](LICENSE) 文本的前提下，欢迎自由 Fork 本仓库、发布学术复现代码，或在非商业前提下向社区分享修改与补丁。
- **商业化生产与创收使用需事先授权**：将 RSIAgent 用于商业化生产运营、作为付费或托管云服务（SaaS/PaaS）对外提供，或将其集成、打包、分发至商业产品或创收服务中，需事先获得 Acosmi 书面授权（联系邮箱：[fushihua@acosmi.com](mailto:fushihua@acosmi.com)，官网：[acosmi.com](https://acosmi.com)）。

根目录 [`LICENSE`](LICENSE) 载明完整中英双语条款；第三方组件与材料的许可及声明仍适用于其对应材料。Workspace crate 保持不发布。
