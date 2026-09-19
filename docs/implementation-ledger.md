# RSIAgent 实施台账

唯一规范入口（方案正文仅本地）：`RSIAgent-v4.1定稿-工程实施方案-2026-09-19.md`。
plan_version：`v4.1`；plan_sha256：`45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`。
本台账是实施与证据索引，不另立规范；历史 PR、测试和旧台账不决定当前规则。
用户于 2026-09-19 明确确认 v4.1 替代 v4，并授权将本台账上传远程；方案正文、内部合同、归档及 QA 原始材料只保留本地。用户后续明确要求验收通过即逐任务推送和建立 PR；当前按此执行。Actions 派发、合并和发布未授权。

## 当前基线与保护

| 字段 | 当前事实 |
|---|---|
| 核对时间 | 2026-09-19 UTC |
| source_sha / 实时远程 main | `9d4ef199c64275a0d4025cfa410f6475f635b0cd`；通过 git ls-remote 实查 |
| 初始分支 | `implementation/v4-e01-sequential-reject-holdout-20260918` |
| 初始未提交文件 | `evaluation.rs` 已修改；`holdout.rs`、`sequential.rs` 未跟踪；已逐文件和 diff 保存本地快照 |
| 既有 stash | `wip-host-cli-http-mcp: uncommitted at v4 E00 start`；已只读备份，未 apply/drop |
| 固定输入 | 从 source_sha 导出独立基线副本后运行门禁，工作树新改动不冒充基线测试输入 |
| 迁移 | 现有 0001/0002/0004；0003 为历史增量预留，不复用；主控已为E04分配0005_root_budget.sql；其他新增编号仍须统一分配 |
| v4 归档 | 已移动至本地 `archive/2026-09-19-pre-v4.1/`，前后 SHA-256 均 `d26ab3506736849f3ec1d286b49fcfa581a09c8be2243681fcc8e93b758f9d6d` |
| 旧台账 | 原字节另存本地归档；历史远程版本可由 source_sha 查阅 |
| 协作索引 | 当前仓库未发现 AGENTS.md 或独立协作索引；本轮按用户明确规则与当前真源执行，本地建立派发/合同索引 |
| 保护范围 | 不修改前端、只读参考仓；不改变架构/阶段/默认启用范围；禁止 cargo xtask ci |

## 单任务 PR 顺序

每个 E 任务独立审阅；先验收再整合。以下同时登记任务顺序与实际 GitHub PR；采用依赖栈，每个 PR 只审阅本任务差异。已推送不等于已合并，merged_sha 均为空。

| 顺序 | 任务 | 范围与依赖 | 负责人/修改白名单 | 状态 | PR / source_sha / merged_sha |
|---|---|---|---|---|---|
| 01 | E00 | 固定基线、真源切换、历史/当前证据范围；无依赖 | 子代理复验；主控已核对源码与原始日志并定向重测 | verified（固定基线与治理脚本局部） | [PR #29](https://github.com/acosmi/RSIAgent/pull/29) draft；源码 `ef8244d` 已推送；未合并 |
| 02 | E01 | 统计与独立留出静态合同；依赖 E00 核心基线 | sol/high实施，主控独立验收 | verified（静态合同）/ blocked（真实小试） | [PR #30](https://github.com/acosmi/RSIAgent/pull/30) draft；源码 `c5ac47d` 已推送；未合并 |
| 03 | E02 | 有界原子 Skill 编辑纯编译作用域；依赖 E01 合同验收 | sol/high实施，主控独立验收 | verified（编译子范围） | [PR #31](https://github.com/acosmi/RSIAgent/pull/31) draft；源码 `79af6c5` 已推送；未合并 |
| 04 | E03 | 真实应用诊断、小批反思、建议来源与开发消费者；依赖 E02 | sol；core/engine optimization、model、evidence及定向测试 | verified（程序消费者/恢复子范围） | [PR #32](https://github.com/acosmi/RSIAgent/pull/32) draft；源码 `727e9cc` 已推送；未合并 |
| 05 | E04 | 持久根预算、broker、取消/对账；依赖 E02 | sol/high；storage budget、0005_root_budget、engine broker/executor及定向测试 | verified（根预算/broker子范围） | [PR #33](https://github.com/acosmi/RSIAgent/pull/33) draft；源码 `f02e535` 已推送；未合并 |
| 06 | E05 | 独立评测/留出、连续前缀与早停证书；依赖 E01/E03/E04 | sol/high；engine evaluator/streaming_evaluator及定向测试 | verified（持久评测程序子范围）/ blocked（真实评测） | [PR #34](https://github.com/acosmi/RSIAgent/pull/34) draft；源码 `86e53b9` 已推送；未合并 |
| 07 | E06 | 组合发布门禁、CAS、实际应用快照与实时撤销；依赖 E02/E05 | sol/high实施；主控代码复读、修复回退与读取旁路后隔离复验 | verified（持久门禁/拒绝路径）/ blocked（真实发布） | [PR #35](https://github.com/acosmi/RSIAgent/pull/35) draft；源码 `071c54d` 已推送；未合并 |
| 08 | E07 | HostService、HTTP/MCP/CLI与参考宿主；依赖E03–E06 | sol/high实施；主控独立代码与实际进程验收 | verified（本地协议/E05管理子范围）/ blocked（真实G1） | [PR #36](https://github.com/acosmi/RSIAgent/pull/36) draft；源码 `0ed5ec9` 已推送；未合并 |
| 09 | E08 | 持久撤销、内容清理、备份与可信锚恢复；依赖E06 | sol实施、Astra/low恢复修复；主控独立验收 | verified（当前对象/本机离线恢复） | [PR #37](https://github.com/acosmi/RSIAgent/pull/37) draft；源码 `644d95d` 已推送；未合并 |
| 10 | E09 | 受限策略/持久探索/E03真实消费者；依赖E03/E04/E07 | sol/high实施，主控独立验收 | verified（程序协调子范围）/ blocked（真实G2及恢复场景） | [PR #38](https://github.com/acosmi/RSIAgent/pull/38) draft；源码 `c71db74` 已推送；未合并 |
| 11 | E10 | 不可变观察/世界池/纯回放/持久报告；依赖E08/E09 | sol/high实施，主控独立验收 | verified（程序回放子范围） | [PR #39](https://github.com/acosmi/RSIAgent/pull/39) draft；源码 `77177cd` 已推送；未合并 |
| 12 | E11 | 经济实验合同/全成本/持久阻塞报告；依赖E05/E07/E10 | sol/high实施，主控独立验收 | verified（静态与持久准备）/ blocked（真实经济实验） | [PR #40](https://github.com/acosmi/RSIAgent/pull/40) draft；源码 `289d920` 已推送；未合并 |
| 13 | E12 | 零预算课程/固定纯函数/可信事实拒绝门；依赖E04/E07/E08/E09 | sol/high实施，主控独立验收 | verified（离线子范围）/ blocked（真实学习闭环） | [PR #41](https://github.com/acosmi/RSIAgent/pull/41) draft；源码 `e612fcb` 已推送；未合并 |
| 14 | E13 | 持久监测/巩固触发/根预算与恢复；依赖E07/E08 | sol/high实施，主控独立验收 | verified（程序监测子范围）/ blocked（长期实测） | [PR #42](https://github.com/acosmi/RSIAgent/pull/42) draft；源码 `2c79608` 已推送；未合并 |

后续依真源依赖图按 E14、E15、E16.1–E16.6、E17、E18 分任务交付；E16 为六子包总门禁，不能用总勾选隐去未完成子包。

## 当前 v4.1 主任务状态

状态：planned / in_progress / blocked / implemented_not_verified / verified / explicitly_out_of_scope。verified 只对明确作用域成立，不等于效果 improved 或可发行。

| 编号 | 范围 | 状态 | 未完成项/边界 |
|---|---|---|---|
| E00 | 归并真实源码与可重建输入 | verified（固定基线/治理脚本）/ blocked（历史归并） | 原始112测试与门禁均复验通过；脚本已完成10项主控正负验证；源码或fixture变更后旧日志不能用于新输入；历史缺包不隐藏。 |
| E01 | 先冻结实验、任务分区和预算可行性 | implemented_not_verified（静态合同已verified） | 本地bfbed84；显式n/统计前提、alpha/留出/比较/完整费用合同已验；开发小试/正式样本与付费授权仍缺。 |
| E02 | 最小版本化契约与宿主能力边界 | in_progress | 纯编译作用域：有界原子编辑；运行/宿主/其他新增契约尚待后续消费者。 |
| E03 | 把跨任务证据真正接入生成消费者 | in_progress（消费者/恢复已verified） | 主控144项core/engine测试、fmt/clippy通过；实际ModelPort请求、同清单开发选择及持久恢复已验。真实提供商/样本/收益未验。 |
| E04 | 可信执行、隔离与根资源预算 | in_progress（根预算/broker已verified） | 主控预算15（含10001调用）、broker9、executor6项及fmt/clippy通过；group完整分页停止修复已追加PR #33；真实提供商、进程隔离尚未验。 |
| E05 | 独立验收器与有边界的统计判定 | in_progress（持久控制/早停已verified） | 主控12流式+6旧评测+13core共31项及fmt/clippy通过；晚到回执、输出不可变、Exposure时间/终态/未知费用保留已修；真实提供商、进程隔离、完整成本及逐依赖撤销仍未验，fixture禁止晋级。 |
| E06 | 组合发布、实际应用与最小撤销闭环 | in_progress（持久门禁已verified） | 主控7集成+4单元+12 E05回归及fmt/clippy通过；审批防回退、Host完整报告闭包、所有快照读入口已验；真实生产批准/组合应用/回滚受E05证据阻塞。 |
| E07 | 第一个最小可验证真实闭环 | in_progress（协议子范围已verified）/ blocked（真实闭环） | 主控协议21项与管理增量27项、真实HTTP/MCP/CLI、参考宿主及fmt/clippy/build通过；E05注册/票据管理已接线。exploration/replay/curriculum/meta管理适配仍缺；真实模型、独立数据与支付授权未取得。 |
| E08 | 可恢复的撤销、保留和备份链 | in_progress（当前对象/本机恢复已verified） | 主控47项及clippy/fmt通过；当前可信SQLite锚由操作者指定，不声称辨别假冒旧库。Linux/生产演练及后续新增对象清理另验。 |
| E09 | 生成/探索解耦与有状态在线探索 | in_progress（程序协调已verified） | 主控25项及clippy/fmt通过；完整输入幂等、两节点StoreJournal链、源水位、整批资源已验；实际产生可修复故障的E03链、真实G2及全多组/实践范围仍待完成。 |
| E10 | 不可变世界池与纯查表回放 | in_progress（程序回放/池/报告已verified） | 主控57项、clippy/fmt通过；观察正文绑定实际共同输入和来源，q0/辅助来源篡改拒绝；世界/池/报告持久与实时撤销已验。真实观测与管理适配仍未完成，不声称经济收益。 |
| E11 | 验证回放优化的真实经济收益 | in_progress（合同/持久准备已verified） | 主控10项及clippy/fmt通过；单票配对、九类成本、实际预算绑定/最终回执不可变、并发取消/晚到账/报告CAS已验。可信在线配对回执消费者尚未实现；真实经济实验未运行，不声称节省。 |
| E12 | 学习者条件化的经验自主获取 | in_progress（离线子范围已verified） | 主控21项和参考宿主3个进程用例、clippy/fmt通过；控制注册、精确平台期、冷却/零预算终态、事实拒绝门已验。E03可信执行/评分回执schema仍缺，真实隔离、应用正例及持久学习改变下轮选题仍未验，不启用G3。 |
| E13 | 长期部署适应与能力保留监测 | in_progress（程序监测已verified） | 主控22项及clippy/fmt通过；两周期触发、单claim、真实根绑定前置校验、异常/撤销持久终态和漂移已验。仅程序fixture，真实提供商、连续轮次保留/长期效果仍未取得。 |
| E14 | 受限改进器自身的继承控制器 | planned（本地合同建议已交接） | 未开始代码；单机制候选、真实下一作业及身份边界仍须主控定版；不启用G4。 |
| E15 | 后继质量与跨代收益实验 | planned | 真实后继实验未运行。 |
| E16 | 产品支持范围与最终交付门禁 | planned | 旧实现保留；尚未按 v4.1 全部合同独立验收。 |
| E16.1 | 来源导入与版本化读取器 | planned | 旧实现保留；尚未按 v4.1 全部合同独立验收。 |
| E16.2 | 资产导入／分享与隐私门禁 | planned | 旧实现保留；尚未按 v4.1 全部合同独立验收。 |
| E16.3 | 内置种子与本地修改保护 | planned | 旧实现保留；尚未按 v4.1 全部合同独立验收。 |
| E16.4 | 额外真实宿主与配置面漂移 | planned | 额外宿主需要本轮实查，历史 blocked 不自动升级。 |
| E16.5 | 持久恢复、容量、依赖与部署安全 | planned | 旧实现保留；尚未按 v4.1 全部合同独立验收。 |
| E16.6 | 发行、证据台账与唯一真源交接 | planned | 旧实现保留；尚未按 v4.1 全部合同独立验收。 |
| E17 | 可选：开发代理评分器演化 | planned | 默认关闭；仅拒绝路径待本轮验证。 |
| E18 | 可选：自动提出代码修改，不自动部署 | planned | 默认关闭；仅拒绝路径待本轮验证。 |

## 追踪范围

| 追踪组 | 规范纳入 | 实现 | 本轮验证 | 效果 |
|---|---|---|---|---|
| B01 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| B02 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| B03 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| B04 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| B05 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| B06 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| B07 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| B08 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| B09 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| B10 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| U01 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| U02 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| U03 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| U04 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| U05 | yes | 核查/实施中 | not_run（本轮未完成全项） | not_claimed |
| U06 | yes | 核查/实施中 | not_run（本轮未完成全项） | not_claimed |
| U07 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| U08 | yes | 核查/实施中 | not_run（本轮未完成全项） | not_claimed |
| K01 | yes | 核查/实施中 | not_run（本轮未完成全项） | not_claimed |
| K02 | yes | 核查/实施中 | not_run（本轮未完成全项） | not_claimed |
| K03 | yes | pure compiler implemented | compiler局部已验；消费者待验 | not_claimed |
| K04 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| K05 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| K06 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| K07 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| K08 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| K09 | yes | 核查/实施中 | not_run（本轮未完成全项） | not_claimed |
| K10 | yes | planned（不继承旧通过） | not_run（本轮未完成全项） | not_claimed |
| SO01–SO18 | 固定 SkillOpt commit `79124b37e9a6371e13b753f8bcd7adb1e493ade1` / 路径 / blob 以真源§16.5为准 | 不整体引入运行时；无直接复制声明 | 实际附件/摘要待核 | not_claimed |
| V001–V098 | 全98场景族保留 | 分 E/对象作用域登记 | 当前无全族 verified | not_claimed |

## 作用域化验收与证据

| 场景 | 状态 | 当前事实/后续条件 |
|---|---|---|
| V001/V072@E00 | verified（基线作用域） | 主控核对73个源blob、原始日志；112项测试全部通过；另行重跑缺行票据失败/不退额度测试通过；T历史等价仍unverified |
| V071@E00 | verified（本机入口/归档） | v4归档前后摘要一致，根目录唯一v4.1，方案/归档/QA被gitignore排除；远程台账已提交 PR #29 draft；方案未上传 |
| V073@E00 | blocked | 历史原包/增量包未取得；不从方案重造；不阻塞独立当前实现 |
| V080/V098@E00 | verified（来源/追踪索引局部） | 25个E节点、98个V场景族保留；SO/K/W41附件索引完整，3个vendor blob与8个RSIA blob吻合；未重新执行上游7项 |
| V010–V013/V084/V085/V096/V097@E01/core | verified（静态/数值局部） | 主控75项core测试、fmt/clippy/80位Python参考均通过；完整端到端和真实数据/费用/收益仍未验 |
| V089@E02/compiler | verified（a/b/c及d字节边界） | 主控隔离副本执行14项编辑测试与1项旧host_surface均通过；fmt/clippy修正后通过；Token/完整请求上下文部分仍未验 |
| V091/V094@E02 | implemented_not_verified | 仅纯编译报告/作用域/来源绑定；运行上下文、缓存、后续消费者与全族断言未验 |

每条后续验收记录必须保留 plan_version/plan_sha256、source_sha、PR、merged_sha、命令、输入摘要、退出码、本地日志位置、实际结果、风险/支持范围及回滚点。没有真实合并则 merged_sha 为未合并。

## 明确保留的阻塞

- 历史原始包 SHA-256 `63c2d194e5371b2eff3e2e38fa714a2ec51334b1a934f64a5027e0399b813b62` 与增量包 `a8e0a6226c354969db630529ff90a0e934b608825912cd3e71593dcbfa7e05cb` 未取得。V073 保持 blocked。
- T001–T126 原始断言未提供；实际源码回归按路径和测试名登记，保持 `legacy_equivalence_unverified`，不虚构 T 映射。
- 付费预算缺省0；本轮没有真实模型小试、真实收益、Linux隔离或容量性能证据。相应门禁不得标通过。
- 旧实现的 bootstrap、状态推进、按 policy 名查表、bool oracle/模型使用等问题按当前源码逐项复核；不借历史PR标题标绿。
- 验收通过的任务源码与台账逐任务推送并建立 PR；Actions 派发、合并和发布仍未授权。

## 本地证据索引

本轮本地目录：`out/implementation-v4.1-20260919/`。初始工作树快照、baseline-manifest.json、固定基线导出、子代理原始输出、主控复核与数值QA均不上传。
历史台账/规范内容仍保留为证据，当前执行无需回读旧方案。

## 2026-09-19 首批独立验收记录

| 作用域 | 输入 | 命令/实际结果 | 日志（本地） | 回滚点/限制 |
|---|---|---|---|---|
| E00固定源码 | `9d4ef199c64275a0d4025cfa410f6475f635b0cd`，73个tracked blob全匹配 | fmt/check/test/clippy/build/smoke_workspace/smoke_cli/inventory均exit0；112项Rust测试 | `out/implementation-v4.1-20260919/e00/`、`controller-e00.json` | 不含新增源码；Xcode缓存告警保留，未造成构建失败；历史包未归并 |
| E00主控定向 | 同固定基线 | `cargo test --locked --offline -p evo-engine evaluator::tests::incomplete_rows_fail_the_ticket_without_refund` exit0 | `controller-e00-directed-test.log` | 保留旧完整批次行为；不是流式早停已实现 |
| E02编辑编译 | 固定基线+E02三个文件+单独module export；输入逐文件hash留本地 | `cargo test --offline --locked -p evo-core --test skill_edit --test host_surface` 15 passed；fmt exit0；clippy -D warnings最终exit0 | `controller-e02-input.json`、`controller-e02-tests.log`、`controller-e02-fmt-after.log`、`controller-e02-clippy-after.log` | 仅编译子范围；未接真实宿主/token硬限/正式评估；未合并 |

审阅中已修复：E02必填锚点错误限制空技能插入、报告缺作用域、来源同ID异摘要、集合顺序、edition格式与Clippy。E01连续前缀逐项停止、alpha绑定、新wire数值、disabled留出与根费用合同均已修正并通过主控局部验收；相应持久消费者仍由E04/E05完成。所有工作均绑定页首v4.1摘要；方案正文未上传。

### 2026-09-19 后续验收与本地提交

| 任务 | 主控验收 | 本地源码提交 | 远程状态/仍未完成 |
|---|---|---|---|
| E00治理脚本 | 保留号/迁移checksum拒绝、未绑定/歧义台账拒绝、固定输入112日志观察、代码或fixture变化全部unverified；10项正负检查通过 | `c71b9ca05fd4652b7935a6c74997a4baabea2969` | 源码已推送至 PR #29；历史归并仍blocked |
| E01静态合同 | 隔离副本75项core测试（13项新增），fmt/clippy均0；独立80位参考k12/13/14和固定种子仿真通过；输入文件hash复核无变化 | `bfbed849e4b16d82e4bd088b4cc7cc01e3d3a64f` | 无真实小试/正式数据/付款授权，不声称E01全部完成 |
| E02纯编译 | 14新测+1旧宿主契约；fmt/clippy最终0；所有修正由主控复读 | `b7b3f1f278612ebaebfbca15d743255b89ed746e` | 上下文Token计量、真实宿主、后续消费者另验 |

E01原始日志：`controller-e01-tests.log`、`controller-e01-fmt.log`、`controller-e01-clippy.log`、`controller-e01-reference.log`、`controller-e01-input.json`，均位于本轮本地证据目录。数值外扩1e-12，当前参考扫描最大裸差约2.24e-16；此为程序参考检查，不是全平台数值证明或产品效果。E03/E04/E05已完成所列子范围的主控验收与源码推送，不用局部测试计数标记全量后端完成。

## 2026-09-19 逐任务远程交付

已纠正此前仅本地提交、未逐任务推送的问题。每个源码提交的 [skip ci] 交付副本与原主控验收版本逐文件比较，除台账外完全一致；不包含工作树中未验收的后续任务。以下提交是源码输入绑定点，后续台账提交不改变该源码。各 PR 均为 draft，merged_sha=null。

| 任务 | PR | 原验收提交 | 已推送源码提交 | base |
|---|---|---|---|---|
| E00 | [#29](https://github.com/acosmi/RSIAgent/pull/29) | `c71b9ca05fd4652b7935a6c74997a4baabea2969` | `ef8244dae4b5dd173987e78915500cc997dcd9b0` | `main` |
| E01 | [#30](https://github.com/acosmi/RSIAgent/pull/30) | `bfbed849e4b16d82e4bd088b4cc7cc01e3d3a64f` | `c5ac47d506a98a9feb0b65fac6815e80087c8f49` | `wrokbot/v4.1-e00-baseline-ledger` |
| E02 | [#31](https://github.com/acosmi/RSIAgent/pull/31) | `b7b3f1f278612ebaebfbca15d743255b89ed746e` | `79af6c515b3afdf36778ee57ba2f140a22a3c0e0` | `wrokbot/v4.1-pr-e01-evaluation` |
| E03 | [#32](https://github.com/acosmi/RSIAgent/pull/32) | `3d043c9f0a79e418656699952421c0aab3d5c804` | `727e9cca8d4118dd207b1ad5ab043c2b6229fef7` | `wrokbot/v4.1-pr-e02-atomic-edits` |
| E04 | [#33](https://github.com/acosmi/RSIAgent/pull/33) | `4be114e0b7f2af39d1d856de31a94e3503f7dd22` | `f02e535798423aaef930e915b2af081c110adf0e` | `wrokbot/v4.1-pr-e03-optimization` |

主控新增验收证据：E03输入 `controller-e03-final-input.json`，`cargo test --locked --offline -p evo-core -p evo-engine` 共144项通过，fmt/clippy exit0；日志 `controller-e03-final-tests.log`、`controller-e03-final-fmt.log`、`controller-e03-final-clippy.log`。E04输入 `controller-e04-session-input.json`，预算14/broker9/executor6项定向测试通过，fmt/clippy exit0；日志 `controller-e04-session-tests.log`、`controller-e04-session-clippy.log`。以上均仅存本地证据目录。回滚点为各任务上一源码提交；撤销水位、已派发费用和已消费查询不能回退。

### E05 程序子范围独立验收与 PR

[PR #34](https://github.com/acosmi/RSIAgent/pull/34)，base=`wrokbot/v4.1-pr-e04-root-budget`；原验收提交 `e5b3a155d12a941c4cfbced55925c8c5ee0222e3`，已推送源码 `86e53b9add040192c4db0334c04c15316cf32e66`；源码逐文件一致（除台账），merged_sha=null。

输入：`controller-e05-final-input.json` / `controller-e05-acceptance.json`；主控在隔离副本运行 `cargo test --locked --offline -p evo-engine --test streaming_evaluator` 12 passed、`cargo test --locked --offline -p evo-engine --lib evaluator::tests` 6 passed、`cargo test --locked --offline -p evo-core --test evaluation_v41` 13 passed；clippy all-targets -D warnings、fmt检查均exit0。日志为本地 `controller-e05-final-*.log`。修复后重新执行流式测试与clippy/fmt，最终输入摘要已冻结。

V010–13/V084/V085仅在冻结合同、程序fixture、持久票据/账本和拒绝路径作用域验收；不标整个场景族通过。ProgramFixture报告不可批准生产，真实执行/数据、独立进程、完整费用和逐依赖来源闭包仍未完成。回滚点为E04已验源码；已消费的query/alpha/派发和未知账单不回退。

### E06 持久发布门禁独立验收与 PR

[PR #35](https://github.com/acosmi/RSIAgent/pull/35)，base=`wrokbot/v4.1-pr-e05-streaming-evaluation`；原验收提交 `92dafe82c2f7c1d90468fd8efa5efe3cfc38e311`，已推送源码 `071c54d53488f8d9a75cc7148d82ed5f9efb3253`；源码逐文件一致（除台账），merged_sha=null。

输入 `controller-e06-final-input.json` / `controller-e06-acceptance.json`。主控在独立源码/target副本运行 `cargo test --locked --offline -p evo-engine --test release_store` 7 passed、`cargo test --locked --offline -p evo-engine --lib release_store::tests` 4 passed、E05流式回归12 passed，clippy all-targets -D warnings和fmt均exit0。日志本地 `controller-e06-final-*.log`、`controller-e06-e05-regression.log`。

作用域限候选/组合摘要、独立审批拒绝、并发CAS、幂等防状态倒退、合法零技能快照与缺报告/撤销/删除拒绝；未产生真实Approved/Canary/Active正例。Host专用crate内部只读验证复用完整E05闭包，不角色提升、不跳过正式报告。回滚点为E05已验源码；真实发布/组合/回滚仍待外部证据，不能用fixture升级支持范围。

### E07 本地协议子范围独立验收与 PR

[PR #36](https://github.com/acosmi/RSIAgent/pull/36)，base=`wrokbot/v4.1-pr-e06-release-store`；原验收提交 `58865edca396f54ae4cbd9585fb201173e19751e`，已推送源码 `b22eb5cf59508bce7d8385d9aa9055ac1cd18463`；源码逐文件一致（除台账），merged_sha=null。

输入 `controller-e07-protocol-acceptance.json`。主控业务9、HTTP lib2/真实listener2、MCP lib1/stdio2、旧closed_loop fixture5共21项通过；`cargo test --locked --offline`、clippy -D warnings、fmt、build均exit0；`python3 scripts/smoke_cli.py target/debug/rsia`实际输出 `SMOKE_CLI_OK model_transport=disabled benefit_claimed=false`。HTTP监听初次因sandbox EPERM失败，取得本地回环执行权限后实际复跑通过，未调用付费模型。日志本地 `controller-e07-*.log`。

主控另用rustc直接编译参考宿主并运行含tab/引号/换行/控制字符的真实路径，JSON解析和路径逐字一致；code模式sandbox_unavailable，见 `controller-e07-reference.json`。仅新增既有fs2所需锁定依赖，无原有registry版本漂移。管理操作仍待job/dispatch真实接线，真实G1/收益/独立进程/发布不在该通过声明内。回滚点为E06已验源码，不能撤回已有副作用或费用事实。

### E08 作用域化独立验收与 PR

[PR #37](https://github.com/acosmi/RSIAgent/pull/37)；原验收源码 `15e039ecee06de22eb6c3fc47ea67e094969aaf1`，已推送源码 `644d95dd85490bc716f837d7148d7245bce0b2c3`；源码逐文件一致（除台账），merged_sha=null。主控47项定向测试和clippy/fmt exit0。storage 9 unit + 15 budget + 13 lifecycle；engine 9 broker + 1 lifecycle。精确已知schema清理、嵌套秘密移除、不可逆费用/曝光保留、可信锚与原子no-replace均有真实数据库/目录回归。

输入/结论：`controller-e08-acceptance.json`；实际命令均 `cargo ... --locked --offline`；日志 `controller-e08-*.log` 仅本地。回滚点为上一E任务已验源码，已消费query/alpha/派发/费用及撤销水位不回退；支持/未完成边界见状态表及PR正文。

### E09 作用域化独立验收与 PR

[PR #38](https://github.com/acosmi/RSIAgent/pull/38)；原验收源码 `2f540fecf7f88f2d701fb46f1aa7251a33f1ea57`，已推送源码 `c71db74efd83a7dc2866b0f446049adcb4b8d3bd`；源码逐文件一致（除台账），merged_sha=null。主控25项定向测试和clippy/fmt exit0。core探索11、engine探索4、E03回归7、旧探索3。主控核对E03完整请求JSON提取前后token一致；真实StoreJournal fixture形成两节点，完成重连不重复模型调用。

输入/结论：`controller-e09-acceptance.json`；实际命令均 `cargo ... --locked --offline`；日志 `controller-e09-*.log` 仅本地。回滚点为上一E任务已验源码，已消费query/alpha/派发/费用及撤销水位不回退；支持/未完成边界见状态表及PR正文。

环境事件：本轮独立副本生成缓存导致磁盘耗尽，已保留失败日志并仅清理10个已完成副本的target构建缓存，释放约22GiB；源码、原始QA日志和验收输入均保留。受影响E09编译已实际重跑通过，不把ENOSPC记作产品通过或永久阻塞。

### E07 管理增量收口

同一 [PR #36](https://github.com/acosmi/RSIAgent/pull/36) 追加源码 `0ed5ec915f438764217e9bd059bb99c401dce26e`（本地 `2e0b275064a6076413644779f1a02692f467c0cf`）。主控独立27项通过：管理集成8、单元3、E05/管理14、真实HTTP2；实际CLI smoke、build、clippy、fmt exit0。输入/结果为本地 `controller-e07-management-acceptance.json` 与 `controller-e07-management-*.log`。持久身份注册、私有输入恢复、generation/lease fence、取消与错误终态、E05注册/签发/状态消费者已验。exploration/replay/curriculum/meta仍返回blocked_feature，属于剩余工程适配；真实G1和完整逐依赖清理仍未验。E08/E09仅整合此已验前置，未合并任何GitHub PR。

### E10 程序回放子范围独立验收与 PR

[PR #39](https://github.com/acosmi/RSIAgent/pull/39)，base=`wrokbot/v4.1-pr-e09-persistent-exploration`；本地验收源码 `227a09e`，推送源码 `77177cd5ffa87de5152a3295974ae41fc86498d4`；除台账外逐文件一致，merged_sha=null。

主控在固定独立副本执行 `cargo test --locked --offline`：core replay 7、storage unit/replay/lifecycle 33、engine legacy replay 4、engine replay integration 13，共57项；三crate的clippy all-targets -D warnings及fmt exit0。输入 `controller-e10-final-input.json`，结论 `controller-e10-acceptance.json`，日志 `controller-e10-*.log` 仅本地。

完整观察输入/来源、真实存储篡改反例、W_sim=1/2/4屏障、OOS/uncertain成本、池分区、报告语义重算和撤销清理已验；原始观察为程序fixture，不冒充真实任务执行或收益。回滚点为E09加E07管理已验源码，已消费费用/来源撤销不得回退。

### 实施分工切换决定

用户确认先收口当前在途任务，再切换由 Antigravity 实施、Codex 主控裁决与独立验收。当前只收口 E07管理增量和 E10–E13，不启动E14及后续代码。E14仅保留本地合同建议；剩余工程与外部条件分别登记。每项任务保持独立PR，旧任务补齐继续追加对应PR；方案/合同/原始QA仍仅本地。

### E11 静态/持久准备独立验收与 PR

[PR #40](https://github.com/acosmi/RSIAgent/pull/40)，base=`wrokbot/v4.1-pr-e10-immutable-replay`；本地 `d62bde65a42a46fa20ddcf06ad67418771bae099`，推送源码 `289d9204c6d76018080a9221967eebe5848e2b8d`；除台账外源码一致，merged_sha=null。主控 core 4、SQLite integration 3、legacy 3 共10项，clippy all-targets -D warnings、fmt exit0。输入 `controller-e11-final-input.json` / 结论 `controller-e11-acceptance.json`，日志 `controller-e11-*.log` 仅本地。

实际根/namespace/group/stage/付款身份、所有者、不可变最终费用、并发成本合并/取消和最终报告CAS已验。当前引擎只能产出blocked_support/usage_uncertain，完整在线回执消费者属于未完成工程；实际配对实验及收益仍blocked。回滚点E10；实际费用/查询不退。

### E12 离线子范围独立验收与 PR

[PR #41](https://github.com/acosmi/RSIAgent/pull/41)，base=`wrokbot/v4.1-pr-e11-replay-economics`；本地 `6fd4f5eb644e59d944eab86a39f4b2d3b8ae25fc`，推送源码 `e612fcb9563c8e320e011e6d06f026b9730f561c`；除台账外源码一致，merged_sha=null。主控core 5、engine课程3、E09回归4、E03回归7、可信helper拒绝1、legacy 1，共21项；clippy all-targets -D warnings、fmt exit0。直接rustc编译参考宿主，clamp正例/非法域/code禁用3个实际进程用例通过。

输入 `controller-e12-final-input.json`，结论 `controller-e12-acceptance.json`，日志 `controller-e12-*.log` 与 `controller-e12-reference.json` 仅本地。禁止用Fixture或Admin自报摘要形成完成周期；目前可信执行/评分回执schema仍未实现，实际学习改变下次选题的正链未验。来源撤销/预算事实不因回滚恢复；回滚点为前一已验源码。

### E13 程序监测子范围独立验收与 PR

[PR #42](https://github.com/acosmi/RSIAgent/pull/42)，base=`wrokbot/v4.1-pr-e12-curriculum`；本地 `c9b85b88d11190bbc83d6361904a91c3ede63a4c`，推送源码 `2c79608fb1d30f7592b742e972c209cb9208fabc`；除台账外源码一致，merged_sha=null。主控monitoring 5、broker 10、E03 regression 7，共22项；clippy all-targets -D warnings、fmt exit0。输入 `controller-e13-input.json`，结论 `controller-e13-acceptance.json`，日志 `controller-e13-*.log` 仅本地。

真实StoreJournal程序路径覆盖每两开发周期唯一claim、来源2撤销、隐藏域拒绝、漂移、根预算错配零派发、Running后故障/撤销保留不可重发终态。未获得真实长期测量和收益证明；fixture provenance保留，不能当生产证据。回滚点为前一已验源码；实际账单与撤销事实保留。

### 在途收口与当前进度

E07管理增量、E10–E13已按上述子范围验收、提交、推送并分别归入原任务PR。远程实查E00–E13共14个独立草稿PR（#29–#42），均open/draft，merged_sha=null。25个台账节点中14个已有已验子范围交付，属于56%的任务覆盖率，不能写成后端完成56%；本轮未宣称任一研究阶段的真实全链全部完成。

主控最终在隔离副本对整合源码 `c9b85b88d11190bbc83d6361904a91c3ede63a4c` 运行 `cargo test --locked --offline -p evo-core -p evo-storage -p evo-engine`，301项通过；对应all-targets Clippy -D warnings与fmt exit0。实际输入/命令/日志哈希保存在本地 `controller-closure-integrated-acceptance.json`。Cargo.lock仅构建清理引起reqwest/rmcp列表排序差异，依赖/版本/checksum逐项相同；仓库锁文件未改。

剩余工程：E07探索/回放/课程/meta管理handler；E03可信开发执行/评分回执schema；E05真实独立执行/完整成本与逐依赖闭包；E09实际恢复/多组/实践消费者；E11完整在线配对回执消费者；E12实际学习改变下轮选题；E13真实长期保留链；E14–E18及E16六子包的本轮完整验收。外部条件另列：真实模型/样本与支付授权、合格隔离环境、历史原包/T映射。不能把尚未实现的工程全部归为外部阻塞。

下一轮由Antigravity在隔离工作树实施，Codex负责唯一真源裁决、合同冻结、代码/测试/实际结果独立验收、PR和台账；未派发新代码任务。后续旧任务补齐仍使用对应任务PR，由主控整理依赖，不由实施方合并/推送。方案、内部交接和QA仅本地，本台账允许远程。
