# RSIAgent 实施台账

唯一规范入口（方案正文仅本地）：`RSIAgent-v4.2定稿-工程实施方案-2026-09-30.md`。
plan_version：`v4.2`；plan_sha256：`70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455`。谱系：v4.1 `RSIAgent-v4.1定稿-工程实施方案-2026-09-19.md`（SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，2026-09-30 归档至本地 `archive/2026-09-30-pre-v4.2/`）→ v4.2。历史验证记录保留其验证时的 plan_sha256（真源 §18.8）。
本台账是实施与证据索引，不另立规范；历史 PR、测试和旧台账不决定当前规则。
用户于 2026-09-19 明确确认 v4.1 替代 v4，并授权将本台账上传远程；方案正文、内部合同、归档及 QA 原始材料只保留本地。2026-10-01 第四轮由 Codex 主控持续推进；实施子代理统一使用 gpt-6.1-sol，逐卡记录思考档位。每个任务独立 PR；主控全量复跑及至少两个反例通过后，以固定 head 按依赖顺序合并并核对合并树。历史轮次的合并操作者按各节原始记录保留。Actions派发、付费运行和部署/发布未授权。

## 2026-10-01 第三轮证据折入（AG-060，第四轮主控整理）

本节登记第三轮既有验收与合并事实；不是第四轮重新取得产品效果证据。原始卡、裁决、日志、反例和输入清单仅本地保留。AG-060 的新验收范围是登记一致性和当前整树回归，后续主控验收记录另行折入。

| PR | 任务 | 主控验收 head | merged_sha |
|---|---|---|---|
| [#93](https://github.com/acosmi/RSIAgent/pull/93) | AG-049 | `81449ccd1a3a9f73b464438d8a6edd26a0ff9734` | `81449ccd1a3a9f73b464438d8a6edd26a0ff9734` |
| [#94](https://github.com/acosmi/RSIAgent/pull/94) | AG-050 | `a4a60c8eb0f808cfb6492e867fe82e96d66af95d` | `4a8487413cb8fe51ee16b91a538a71816997adfb` |
| [#95](https://github.com/acosmi/RSIAgent/pull/95) | AG-051 | `4bdc8844738356ba70f9cbbfca1143d2bde94efe` | `e2c9107142413c0ba304b5090692b56037958f40` |
| [#96](https://github.com/acosmi/RSIAgent/pull/96) | AG-046 | `229fea802d8184eb03c7f53f979a78200284b47b` | `49d5b85ec4fa757144213573c585087798324e91` |
| [#97](https://github.com/acosmi/RSIAgent/pull/97) | AG-048 | `f178dc239b619746b07b74241a756dce4897f059` | `804302602efe575d92973be20c532d264019f9aa` |

#93 随 #94 合并进入 main，GitHub 的 mergeCommit 是 #93 head 本身，不能伪造独立 merge commit。#94–97 各 merged_sha 的树等于各自验收 head；第四轮开始时已通过 GitHub 元数据、git 祖先与 tree 再核对。

### 索引登记与作用域

新增四条 v4.2 记录：E12.ag050_curriculum_priority、E16.5.ag051_economic_restore、E08.ag046_write_side_revocation、E09.ag048_terminal_classes，各用其原验收 head 和唯一原始全量测试日志。命令字段为检查器支持的 `cargo test --locked --offline -p … --test …` 文法；实际执行是带镜像的 `cargom test --workspace --locked --no-fail-fast`，未使用 offline，已在 actual_result/input 清单中明示。AG-050 同名测试分属 core（5）与 engine（10），此记录主入口为 engine，日志另含 core 5 项。AG-048 主入口14项，模块内另9项计入全工作区1124项。

总体状态与索引统一：E00 implemented_not_verified（核心基线有已验子范围，历史归并 blocked）；E01 blocked（静态合同已验，实际小试/正式样本及独立任务定义仍缺）；E05 blocked（控制/早停子范围已验，真实执行、完整成本及其他工程项仍缺）。不将总体 blocked 解释为无可推进工程工作，也不将子范围 verified 解释为全项完成。

### GA-1 当前基线测试与迁移清单

固定 main `804302602efe575d92973be20c532d264019f9aa` 的树等于原验收 head `f178dc239b619746b07b74241a756dce4897f059`。日志 `out/audit-20260930/qa/ctrl-ag048-f178dc2.log.test`（SHA-256 `42c1d94f65139322820a9c5af6f8ff4fc748aa3eb7819b58fbb27c6e82622c15`）枚举1124条通过测试；86为 result 行数，含doc-tests，不另称86个测试二进制。逐测试清单 `out/audit-20260930/qa/inputs/baseline-r4-8043026.json`（SHA-256 `0c995a03088c0d9d11242c082fe99c7b9b81b318a64c8d4720e98b0e34832c2a`）仅本地；不把源码静态 test 数当运行计数。

| 当前迁移（crates/evo-storage/migrations） | SHA-256 |
|---|---|
| 0001_runtime.sql | `a2ccef4eba4411a4a5f83aa6b32525345edd7e0cd5e3aaff54b8bc8ef672e684` |
| 0002_revoke_graph.sql | `1695a07425e95a13338a349887c98ba3e4a90ed9cfff35a2f46f49c488c1e057` |
| 0004_replay_worlds.sql | `528d50b397f49021c65484b26c2d4e9f0cfdadf4074648af55c19af843f47fb9` |
| 0005_root_budget.sql | `c7bd918e6a2d123ce1548769f138b71d08140ef314736be7f1bee8e8bb385329` |
| 0006_revoke_cleanup.sql | `2cb41c47cc872cbd5438e0d351f022966a150de879818bd8ac7df5967d3e0610` |

0003_v3_assets.sql 为历史占用；缺失历史包仍是外部阻塞，不复用编号，不声称 T001–T126 等价归并完成。

### GA-1 §1.4/§1.5 当前消费者复核

以下定位基于 main `8043026`，是当前源码事实；“已验”仅引用本台账既有验收，静态复核本身不新增运行或收益证据。

| 真源行 | 当前实现定位与剩余边界 |
|---|---|
| §1.4 L94 | `evo-engine/src/exploration.rs:89` 旧 Coordinator::decide 仍是状态推进；持久路径 `decide_next:713` → `pure_decision:2086` → `evo-core/src/strategy.rs:558 decide_elastic` 消费节点质量/合法动作/剩余预算，`run_next:973` 接实际步骤。类型化修复来源与历史消费仍待完成，不称真实 G2。 |
| §1.4 L95 | `evo-engine/src/replay.rs:43 run_replay`、`:802 lookup_transition` 按 record_seq/generation_signature/parent_context/action 匹配，`:850 finish_report` 保存曲线与 AUC；已非按 policy 名查答案。缺历史为 OutOfSupport，development_only，不能变 FormalEvaluation。 |
| §1.4 L96 | `evo-core/src/curriculum.rs:8` 旧 LearnerState/next_task 仍保留；`:304 LearnerStateV2` 与 `:455 detect_plateau_signal` 有覆盖/环境/冷却/周期等，空失败可继续判断。离线默认零预算/Disabled，状态结构齐备不等于真实学习闭环；失败簇更新语义仍待 GC-6。 |
| §1.4 L97 | `evo-core/src/curriculum.rs:73 proposal_from_text` 默认 oracle_ok=false（AG-050/#94）；`evo-engine/src/curriculum_profiles.rs:299 is_development_eligible` 恒 false，只有 Quarantined/SandboxUnavailable。文本形状不能取得 verified；真实 runner 与隔离仍缺。 |
| §1.4 L98 | `evo-engine/src/curriculum.rs:23 step` 先 next_task? 后 reserve?，保留 typed 选题错误（AG-050/#94）。旧 RootBudget 接口仍为内存、无 dispatch/释放审计，完整预算生命周期不在本修复声明内。 |
| §1.4 L99 | `evo-engine/src/evaluator.rs:80 grade` 保留旧整组 rows；streaming 的 record_grader_receipt/stop_ticket/invalidate/settle_after_stop 与 `evo-core/src/sequential.rs:469 record_complete_unit` 处理前缀、早停及对账。`streaming_evaluator.rs:1944` 完整批次仍被强制 Inconclusive（AG-052 待办）；仅 ProgramFixture/TicketExecutionOnly。 |
| §1.5 L109 | `apps/rsia/src/main.rs:86 main` 已有 Serve/Mcp/Manage，`:139 axum::serve` 和 `:161 serve_stdio` 接入口，已非仅打印 bootstrap。bootstrap 仍是 disabled model/reference host；额外真实宿主与模型证据未取得。 |
| §1.5 L110 | `evo-core/src/contract.rs:193 SkillSnapshot`、`:238 SkillPatch` 保留三态及叶字段；`skill_edit.rs:285 compile_skill_edit_batch` 校验绑定、保护区、编辑界限并生成既有 SkillPatch。文本编译不证明优化收益。 |
| §1.5 L111 | `evo-engine/src/closed_loop.rs:53 run_structural_loop` 仍由传入 Verdict 构造观测；`:148` 忽略凭据 bool，used_real_model/auto_promote 固定 false，attached/used/benefit 为空。结构 fixture 无真实调用、审批或效果声明。 |
| §1.5 L112 | `evo-engine/src/optimization.rs:2823 run_optimization_step` 包含内容编译、`:2269 model.dispatch`、`:2017 runner.run` 和开发选择；持久协调器消费同一步骤，Fixture 不能取得 Trusted。缺端口拒绝；真实模型/runner/收益仍需外部证据。 |
| §1.5 L113 | `evo-engine/src/replay.rs:802 lookup_transition` 不以待测 policy 身份查答案，run_replay/finish_report 实现历史揭示与目标计算；固定历史/OOS 边界及 `:1085 replay_v2_is_not_formal` 保持，新技能实际效果不能由回放证明。 |
| §1.5 L114 | `evo-engine/src/meta.rs:112 verify_mechanism_inheritance` 消费匹配 policy/caps/world 的非空使用记录；`exploration.rs:746 verified_mechanism_usage` 从持久派发事实重验。meta.start 在 `dispatch.rs:1420` 仍 Blocked；仅 exploration policy/深度1程序范围，非元收益证明。 |


### 第三轮记录逐项折入

下列为第三轮事件原记录；其中“待合并/下一窗口/在途”等为当时状态，最终以本节合并表及当前状态表为准。原稿中的测试“二进制数”沿用当时措辞，实际计数口径是日志 result 行。


#### 闸门与收尾
- 真源 SHA-256 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455 一致；读 controller-state、ledger-draft-r2、_common。本地 main 快进到 696252f。
- AG-049 / PR #93（台账 + support-scope 登记，主控自做）：head 81449ccd1a3a9f73b464438d8a6edd26a0ff9734，主控全量复跑 81/1064/0、检查器 structure_valid、unittest 48 OK；探针 qa/probe-ag049/probe.py（P1 31 条记录 git 祖先与输入清单、P2 日志内容、T1–T6 篡改全拒，首版 T5 设计有误已改并在 PR 披露）。已转 ready，runbook AI。待用户合并。

#### 缺口侦察（只读，main 696252f）
- GAP-A（E00–E05、E16.x）：cards/gap-A-20261001.md；GAP-B（E06–E10 + 已知队列事实底稿 D1–D10）：cards/gap-B-20261001.md；GAP-C（E11–E15、E17/E18）：cards/gap-C-20261001.md。
- 主控核实：§1.4 L97 proposal_from_text 仍写 oracle_ok:true（core curriculum.rs:65–76）；E05 streaming_evaluator.rs 约 1939–1947 对 CompleteBatch 无条件把 verdict 覆写为 Inconclusive（与 E05 L1257"结论区分…UCB越界才确认平均退化"不符）；真源全文"tombstone"只在 E08 回滚 L1301，"tombstone 键带 kind"无条款（不派，待用户）。

#### 主控裁决（本轮）
- 队列顺序以真源依赖为准：AG-048（类型化终态类）先于 AG-047（类型化可修复来源以 AG-048 的终态类为基础）。
- AG-048：终态类与固定码在 evo-engine；NoChange/KeepIncumbent 作为显式终态分别记录（节点与 dispatch fact 加 serde(default) 字段，AG-033 先例），决策语义不变；KeepIncumbent 是否可作为可加深的 observed_valid 不在本卡裁定（KeepIncumbent 含"分数更高但破坏既有通过任务"，直接改 Valid 违背 §6.7.3 L751），登记为待用户事项。
- 无真源条款的项不派发：tombstone 键带 kind、SQLite 本地物理擦除。

#### 派发（基线 81449cc = #93 栈顶；四卡文件互不重叠，并行）
- AG-048（E09 B4c）卡 SHA-256 69eb6abb91a55f6b92480636849abfd0a2135bf7f70dd6dde028c8594163edce；worktree ag-048。
- AG-046（E08 写入侧闸门）卡 SHA-256 646b8f055194b6582650857be631cb1076c46ec48d5a68a9f7453f2e255174a8；worktree ag-046。
- AG-050（§1.4 课程两项）卡 SHA-256 c88285c7b8dae1d7ebf7b6c9dfe48f31747d567d05dc3e2d2b062e487f302873；worktree ag-050。
- AG-051（经济记录恢复保护）卡 SHA-256 000f84a869a0701ae9bd537235af187e076022b782b7180b9609ae02795722ef；worktree ag-051。
- _common.md SHA-256 5b33c7145f3ee0e89c29fa89393f96fdc6284ba9fb27b7745ef585a75345e246（scratchpad 路径改为本会话）。

#### CTRL-AG050-R1 / PR #94：§1.4 课程两项优先修复（主控独立验收）
- 交付 head `a4a60c8eb0f808cfb6492e867fe82e96d66af95d`（父 = 栈顶 #93 81449cc，无需叠放）；4 文件 +350/−2：core curriculum.rs（proposal_from_text oracle_ok=false＋注释）、engine curriculum.rs（旧 step 先 next_task 后 reserve，保留 typed 错误）、两个新测试文件（core 5、engine 10）。实施方基线复现：core 4/5、engine 7/10 失败；两种变异分别被 3 与 7 个测试抓住。
- 主控复跑（全新 target-ctrl-ag050，`qa/ctrl-ag050-a4a60c8.log*`）：fmt 0；clippy 0；83 结果行 1079/0（1064+15）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例 2/2（`qa/probe-ag050/append.rs`；head 日志 `qa/ctrl-ag050-adversarial.log`，基线日志 `qa/ctrl-ag050-adversarial-base-81449cc.log`，基线 2/2 失败）：Q1 正文自称 oracle_ok、JSON 往返、池首同族——文本提案始终 unverified、不被选中；Q2 cost=0、恰好用满额度、已 dispatch 的预算——选题错误始终先于预算错误，失败步不预留。
- 结论：verified（E12/E04 子范围：§1.4 L97/L98 两项）。固定 head a4a60c8；须在 #93 之后合并（runbook AJ）。回滚点 81449cc。
- 已知边界（接受）：TaskProposal.oracle_ok 仍可由反序列化设为 true（卡要求不改结构）；旧 step 是内存 RootBudget，无持久预留可审计释放。

#### 合并与收尾（2026-10-01T09:06Z，用户在终端合并 #94）
- 用户以固定 head a4a60c8 合并 #94：merged_sha `4a8487413cb8fe51ee16b91a538a71816997adfb`（2026-10-01T09:06:22Z）。#94 的 head 以 #93 的 head 81449cc 为父，故 #93 随之进入 main；GitHub 将 #93 标为 MERGED（mergedAt 2026-10-01T09:06:25Z，mergeCommit 记为其 head `81449ccd1a3a9f73b464438d8a6edd26a0ff9734` 本身，无单独合并提交）。主控核对：main = 4a84874，其树 abb4cd83… 等于验收 head a4a60c8 的树；81449cc 与 a4a60c8 均为 main 的祖先。
- 合并后收尾（用户第 8 步第 5 项）：本地 main 快进到 4a84874；移除 30 个已合并任务 worktree（ag-018…ag-045、ag-049、ag-050）与 base-81449cc，均先核对 head 为 origin/main 祖先且工作区干净，未用 --force；本地 36 个已合并 wrokbot/* 分支以 `git branch -d` 删除；本地 wrokbot/ag-041-recover-binding-ready（74cf54b）经 `git cherry` 核对两个提交均有等价补丁在 main（"-"），以 -D 删除；远端 36 个 wrokbot/* 分支逐个核对 head 为 origin/main 祖先后删除（首次因 zsh 不分词整体失败、未删除任何分支；改用 xargs 后经一次 SSL_ERROR_SYSCALL 重试全部删除），远端 wrokbot/* 剩 0。git status 干净，main == origin/main。保留：在途 ag-046、ag-048、ag-051 与待派 ag-052；implementation/* 旧分支未动（不在指示范围）。

#### CTRL-AG051-R1 / PR #95：经济回放记录纳入恢复受保护集合（主控独立验收）
- 交付 `6365e4f`（基线 81449cc；4 文件 +1664/−10：lifecycle.rs PROTECTED_ACTION_SCHEMA_VERSIONS 10→11 项加 `rsia.replay_economic_artifact_envelope.v1` 及其文档注释；restore_backup.py 元组同步；新测试 restore_economic_v42.rs 9 项；按授权最小修改 restore_coverage_v42.rs 的列表断言）。实施方基线复现 8/9 失败；两侧单独删项的变异各被 5 个新测试与漂移守卫抓住。
- 线性叠放：从 81449cc rebase 到 main 4a84874（#93、#94 已合并）之上得到 `4bdc884`，补丁逐字节相同；force-with-lease（锁定 6365e4f）推送；PR 正文首行改写。
- 主控复跑（全新 target-ctrl-ag051，`qa/ctrl-ag051-4bdc884.log*`）：fmt 0；clippy 0；84 结果行 1088/0（main 1079 + 9）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例 2/2（`qa/probe-ag051/append.rs`；head 日志 `qa/ctrl-ag051-adversarial.log`，基线 main 4a84874 日志 `qa/ctrl-ag051-adversarial-base-4a84874.log`）：R1 备份之后锚上只以 register_admin_cost 记一笔成本回执（实施方未覆盖的写入路径，根预算表不动）→ 恢复隔离、启动门隔离并点名该回执与原地变化的作业；基线 `RESTORE_OK events=0`（失败）。R2 近似 schema（.v10、.v1.bak、全大写、尾随空格）只在锚上 → 恢复通过、启动门放行（负对照，head 与基线均通过）。
- 结论：verified（E16.5/E08 子范围：经济实验、作业、成本回执（预算调用与 Admin 测量两种来源）与报告受恢复保护，按精确 schema 选取）。固定 head `4bdc8844738356ba70f9cbbfca1143d2bde94efe`；基于 main（runbook AK）。回滚点 4a84874。
- 已知边界（实施方披露，主控接受）：恢复脚本的隔离信息是固定文本，点名由启动门诊断与脚本选取体现；v1 信封下未来新增的 record_kind 也会被保护（清理对未知 record_kind 失败关闭）；只在 macOS 实测。
- 2026-10-01T09:31:04Z 用户以固定 head 4bdc884 合并 #95：merged_sha `e2c9107142413c0ba304b5090692b56037958f40`；main 的树 05d09745… 等于验收 head 4bdc884 的树。随后移除 ag-051 worktree、本地与远端分支（均先核对为 main 祖先）。

#### CTRL-AG046-R1 / PR #96：E08/§11 写入侧撤销闸门（主控独立验收）
- **交付**：`e534bad`（基线 81449cc）。6 个文件，+2543/−126：
  - 新模块 revocation_gate.rs，承接从 dispatch.rs 搬出的两阶段判定。dispatch.rs 保留三参包装，措辞逐字不变。
  - evidence.rs：store_source_selection 在首写前做闸门。
  - packages.rs：verify_sources 改为两阶段，覆盖全部 15 个调用点，含读取路径。
  - lib.rs 一行。
  - 新测试 write_side_revocation_gate_v42.rs，13 项：21 个用例 × 5 个入口的统一表，另有 grant 三态、真实 import_source、衍生源三态、既有包/安装/导出、namespace、上限 10 000/10 001、提交闸七条文案逐字钉住等。
  - 实施方证据：基线复现 11/13 失败；6 个单行变异全部被抓住。
- **线性叠放**：从 81449cc rebase 到 main e2c9107（#95 已合并），得到 `229fea8`。补丁逐字节相同（cmp）。以 force-with-lease 锁定 e534bad 推送，PR 正文首行已改写。
- **主控复跑**：全新 target-ctrl-ag046，日志 `qa/ctrl-ag046-229fea8.log*`。
  - fmt 0；clippy 0。
  - 85 个结果行，1101 通过 / 0 失败（main 1088 + 13）。
  - build 0；两组 smoke 均通过。
  - 检查器 structure_valid；unittest 48 OK。
- **主控反例 2/2**：源文件为 `qa/probe-ag046/append.rs`，附加到新测试文件副本 `qa/probe-ag046/source-write_side_revocation_gate_v42-229fea8.rs` 后运行。head 日志 `qa/ctrl-ag046-adversarial.log`，基线 main e2c9107 日志 `qa/ctrl-ag046-adversarial-base-e2c9107.log`。基线 2/2 失败，探针后 git status 干净。
  - **P1**：在 run 存活时已获批的 grant，于其中第二个 run 经真实 begin_revoke 撤销后原样重复，分别在 Pending 与 Complete 各重复一次。
    - head：两次都返回点名 Conflict，什么都不写，已脱敏的 grant 保持脱敏。
    - 基线：两次都 Ok，每次写 1 行审计；Complete 时的重复**把 grant 的明文正文写回、覆盖了脱敏墓碑**（§11 复活）。
  - **P2**：种子安装与 staged 包各带两个自身存活的来源，第二个来源的上游 run 已经真实撤销（Pending），两种顺序各试一次。
    - head：返回 Forbidden，什么都不写；只用存活来源时仍被接受。
    - 基线：Forbidden 只来自 storage 的晚期检查，此前每次已提交 artifact、边与审计行（残留）。
- **主控裁决（实施方回报的发现 1–3）**：
  1. 卡的侦察事实不完整：storage 的 publish_registered_blob 本有晚期按 id 检查，且有残留。已记录，结论不变。
  2. 同 id 他 kind 的 tombstone 使新 stage/install 在首写后被 storage 晚拒，留下 Prepared 残留：接受为已知边界。理由：AG-036 之后生产中无法造出这种配对；已由 known_boundary 测试钉住；storage 层的 kind 精确性随"tombstone 键带 kind"待用户。
  3. manifest dependency_refs 在定稿（1580）才校验，首写前不校验：接受为已知边界。"什么都不写"只对 grant 与请求的来源引用成立。已入队后续项："stage_package 在首写前对 dependency_refs 做闸门"，依据 §11.3 L1110"阻止新生成"。
- **结论**：verified，子范围为 E08 / §11.3 / §11.5：
  - 来源本身或其上游闭包已撤销时，grant、新 staged 包与新种子安装在首写之前被拒；判定按 namespace 与 kind 精确，不可读 tombstone 失败关闭，闭包超限即拒、不截断。
  - 既有包与安装的读取、导出、交接、重置按同一规则拒绝。
  - 固定 head `229fea802d8184eb03c7f53f979a78200284b47b`，基于 main（runbook AL），回滚点 e2c9107。
- **不可声明**：
  - Complete 之后的其他迟到写入；物理残留。
  - grant 来源的存在性与可信性（仍由消费侧负责）。
  - 生产调用方（三处均无）。
  - storage 发布期检查的 kind 精确性；release_store::stage_bundle；tombstone 键带 kind；manifest dependency_refs 的首写前拒绝。
- **台账提示**：docs/implementation-ledger.md:263 写的是"dispatch.rs 的 ensure_dependencies_live"，逻辑现位于 revocation_gate.rs，下一次台账 PR 时更正。

#### AG-048 主控裁决 R1（2026-10-01；收尾模式下属在途关联返修）
- 实施方本地提交 576674b（基线 81449cc）按卡停下：exploration_trust_v42、exploration_start_after_dispatch_v42、meta_trial_v42、skill_groups_v42 共 4 个既有测试文件也钉住了被移除的自由文本，属卡侦察遗漏。
- 裁决全文：cards/AG-048-R1.md（SHA-256 见下）。要点：
  - 授权上述 4 个文件只改断言期望值、字段访问与导入，"原因"类断言改为与固定码精确相等。
  - 两个 consolidation 测试文件确认属卡内"monitoring 相关"。
  - 设计偏差 1–10 接受。可声明的"不落库"收窄到白名单内的记录；模型回答（日志 DispatchObserved/ResponseObserved）与 broker 账本措辞不可声明。
  - broker.rs 的既有类型归属入队，本卡不动：停止组被归为 Unauthorized；并发上限与 call_id 复用经端口 Err 被归为 Uncertain。依据 §7.2 L817"资源不足…分别记录"。
- 叠放计划：AG-048 交回后叠到 #96 的 229fea8 之上。
- 裁决文件 cards/AG-048-R1.md SHA-256 2ebbaabeab8fba11fad6d45a5a2f1217b2a2575601e2e3f196c68492f01bc755；以 SendMessage 送达实施方（a86ca78），其继续在 81449cc 上补改授权测试、全量自测、推送并开 draft PR。

#### CTRL-AG048-R1 / PR #97：E09 B4c 步骤终态改为封闭类型化终态类与固定码（主控独立验收）
- **交付**：两个提交，基线 81449cc。
  - `8c0a691`：实现，新测试，卡内与 monitoring 范围的既有测试修改。树与首交本地提交 576674b 相同（bb96d005…）。
  - `7c2b89e`：按 R1 授权的 4 个既有测试文件。
  - 合计 13 个文件，+5263/−336。
  - 实现在 evo-engine：optimization.rs 的 StepTerminalClass（32 个变体，43 个码），以及 exploration.rs、monitoring.rs、groups.rs 的映射与记录。evo-core、storage、broker、迁移均未改。
  - 实施方证据：Stage-A 基线 6/11 失败，偏差数：出口表 78、拒绝 kind 36、节点/dispatch fact 39、闭包 1、巩固 5+5；基线全工作区 1069/6。
- **R1 提交逐行复核**：只改期望值、字段访问、导入与描述被断言文本的注释；原因类断言全部改为与固定码精确相等。观测值与实施方预测一致；meta_trial L1964 实测为 `model_transport_outcome_unknown`。
- **线性叠放**：从 81449cc rebase 到 #96 的 229fea8 之上，得到 `418c3a4` 与 `f178dc2`。两提交各自补丁逐字节相同（cmp）。以 force-with-lease（锁定 7c2b89e）推送，PR 正文首行已改写。
- **主控复跑**：全新 target-ctrl-ag048，日志 `qa/ctrl-ag048-f178dc2.log*`。
  - fmt 0；clippy 0。
  - 86 个结果行 1124 通过 / 0 失败，即 main 1088 + #96 的 13 + 本 PR 的 23（新文件 14、模块测试 9）。
  - build 0；两组 smoke 通过。
  - 检查器 structure_valid；unittest 48 OK。
- **主控反例 2/2**：源文件 `qa/probe-ag048/append.rs`，附加到本 PR 不改的 tests/optimization.rs 副本 `qa/probe-ag048/source-optimization-229fea8.rs` 上运行。head 日志 `qa/ctrl-ag048-adversarial.log`；基线（父 229fea8）日志 `qa/ctrl-ag048-adversarial-base-229fea8.log`，基线 2/2 失败。探针后 git status 干净。
  - **T1**：KeepIncumbent 两类的边界，自定义 runner 报两个配对任务。
    - head 四种情形全部正确：
      - 保留通过被调换、总分相等 → `retention_broken`；
      - 破坏保留、候选总分更高 → `retention_broken`；
      - 保留全部通过、总分更低 → `not_improved`；
      - 保留全部通过、总分相等 → `not_improved`。
    - 类中两个总分与选择一致；step_completed 与 terminal_rejected 的码与类相同，旧句不落库。对照组（总分更高且保留全部通过）仍是候选。
    - 基线：四种情形都只存同一自由文本句，无类。
  - **T2**：模型回答不是建议列表，而是含标记串的 JSON 字符串（解析错误会回显它）。
    - head：撤销前只有该调用的 dispatch_observed 持有标记；真实 begin_revoke 来源 run 并清理到 Complete 后，任何 kind 的对象都不再持有（逻辑扫描）。
    - 基线：撤销前 step_completed 也持有标记（错误回显）。
    - 撤销后基线同样清干净，说明日志属来源闭包——这证实了 R1 中"模型回答在闭包内、随撤销清理"的表述。
- **结论**：verified，子范围为 E09 / E03 §6.7.3 / E13 / §11：
  - 优化步骤终态为封闭类型化终态类与固定码；NoChange 与 KeepIncumbent 两子类在节点与 dispatch fact 上分别记录；白名单内的记录不再持久化错误、模型或来源派生的原文；决策逐项不变；存量记录照常读取。
  - 固定 head `f178dc239b619746b07b74241a756dce4897f059`，叠在 #96 之上（runbook AM，须在 AL 之后合并），回滚点 229fea8。
- **不可声明**（R1 收窄）：
  - 模型回答与 StepPrepared 输入不落库（它们在日志中，随来源闭包清理）。
  - broker 账本中的 broker 措辞与 validation_error。
  - AG-047 的可修复来源与 Recover；PR-C 的历史与签名；KeepIncumbent 可加深；任何质量收益。
- **已知边界**：
  - 升级时恰处中间态的存量步骤以 step_error_conflict 终结（失败关闭、不重复派发）。
  - 类字段无交叉校验，也不被决策读取。
  - serde 内部标签的单元变体不拒绝多余字段。
  - 内层 grant 检查经 run_step 不可达。
- **入队（主控）**：broker 的类型归属，依据 §7.2 L817"资源不足…分别记录"：
  - 停止组在预留阶段被拒，归为 Unauthorized，而非 CancelledBeforeDispatch；
  - 台账并发上限 `Conflict("root_budget_concurrency_limit")` 与 call_id 复用冲突，经端口 Err 到达步骤，被记为 transport outcome unknown（Uncertain）。

#### 合并与收尾（2026-10-01T10:35Z，用户在终端合并 #96、#97）
- **#96 AG-046**：用户以固定 head 229fea8 合并。merged_sha `49d5b85ec4fa757144213573c585087798324e91`（mergedAt 2026-10-01T10:35:33Z），父为 e2c9107 与 229fea8。合并后树 5564ce6c… 等于验收 head 229fea8 的树。
- **#97 AG-048**：用户以固定 head f178dc2 合并。merged_sha `804302602efe575d92973be20c532d264019f9aa`（mergedAt 2026-10-01T10:35:48Z），父为 49d5b85 与 f178dc2。合并后树 068e6e2e… 等于验收 head f178dc2 的树。
- **合并后收尾**：
  - 本地 main 快进到 8043026。
  - 移除 ag-046、ag-048 worktree：先核对 head 为 origin/main 祖先、工作区干净，未用 --force。
  - 本地分支用 `git branch -d` 删除；远端两个 wrokbot/* 分支逐个删除。删除后远端 wrokbot/* 剩 0。
  - git status 干净，main == origin/main = 8043026。
- **第三轮合并总表**：

  | PR | 任务 | head | merged_sha |
  |---|---|---|---|
  | #93 | AG-049 | 81449cc | 81449cc（随 #94 合并，GitHub 记 mergeCommit 为其 head 本身） |
  | #94 | AG-050 | a4a60c8 | 4a84874 |
  | #95 | AG-051 | 4bdc884 | e2c9107 |
  | #96 | AG-046 | 229fea8 | 49d5b85 |
  | #97 | AG-048 | f178dc2 | 8043026 |

  merge-all.sh 覆盖 A–AM，全部已合并。开放 PR 为 0。


### 当前队列与待用户边界（第三轮结项后）

#### 下一批（文件与在途不重叠，可随时派）
- AG-052 [M] E05 完整批次保存统计判定本身（GA-16 方案 A）：卡 cards/AG-052.md 已写、未派。卡内基线 a4a60c8 已过期，派发前改为当时栈顶，并重核 streaming_evaluator.rs 行号。
- AG-058 [S] 过期文字（原 queue 中误编为 AG-052，现改号）：
  - README.md:107/147（CLI 现状）；
  - reports/longitudinal/README.md:3（"family retention gates exist" 不实）；
  - evo-core lib.rs:579 MetaEvidence "Equal total budgets" 注释（§9 L1009/V035）；
  - evo-engine meta.rs:649–653；
  - tests/cleanup_fixpoint_v42.rs 头注释；
  - 对应 GB-13、GC-15、GC-18 前半、GA-1 README 部分。
- AG-059 [S] stage_package 首写前对 manifest dependency_refs 做撤销闸门（AG-046 发现 3；§11.3 L1110"阻止新生成"、§11.5 L1132）。
  - 现状：dependency_refs 在定稿 packages.rs 约 1580 才校验，被拒时残留 Prepared 信封。
  - 文件：packages.rs（stage_package）。新测试从 write_side_revocation_gate_v42.rs 复制夹具；known_boundary_a_manifest_dependency… 改为断言什么都不写。
- AG-053 [S] storage lib.rs load_dependency_record_snapshots（约 742–789）CTE 内 LIMIT + CROSS JOIN，语义不变（GB-9；§3.3.1 有界、E16.5）。
- AG-054 [S–M] broker 派发被拒携带真实原因；development runner 派发被拒/执行失败释放 1 micro（GB-10；§7.2.1 L831 未 dispatch 可撤销预留；E04）。文件 broker.rs、development.rs、storage budget.rs。
  - 并入 AG-048 验收时登记的 broker 类型归属（§7.2 L817"资源不足…分别记录"）：
    - 停止组在预留阶段被拒时，broker 归为 Unauthorized（broker.rs 约 492–499：非 Budget 错误一律 Unauthorized），应为 CancelledBeforeDispatch 一类。
    - 台账并发上限 `Conflict("root_budget_concurrency_limit")`（storage budget.rs 约 1801，begin_budget_dispatch）与 call_id 复用冲突（broker.rs 约 431），以端口 Err 返回，步骤将其记为 model_transport_outcome_unknown（Uncertain），而它们是 dispatch 前的确定拒绝。需要裁决：改为带类型的 Rejected{NotDispatched}，并确认对恢复与计费的影响。
    - meta_trial_v42 L1825/L1964、skill_groups_v42 L1986 届时随之改期望值。
- AG-055 [S] E02 v1 golden（五类请求、四工具描述符 ≤2000 字节、v1 Strategy/Evaluation 序列化）（GA-6；E02 L1215）。
- AG-056 [S] E00 CI 质量门：inventory 失败不再被 `|| echo` 吞掉；py 单测与 statistics_reference.py 入 CI；smoke_workspace 钉 0005/0006 校验和（GA-2；E00 L1185"检查真实退出码"）。只改工作流文件，不派发 Actions。
- AG-057 [S] E01 docs/evaluation.md 补 §3.4.1/§3.1.1/定义/锚点矩阵与 manifest 模板；first_low_risk 去掉默认 n_planned=60（GA-4；E01 L1197/L1201）。
- E05 GA-16 [M] verdict 覆写：先派只读侦察核清 v1/v2 decide 语义与"成本未核"理由，再定卡。

#### 之后（按依赖）
- E02：GA-7 → GA-8 → GA-9 → GA-10（contract.rs 串行）；GC-19。
- E03：GA-11（AG-046 后，evidence.rs）。
- E04：GA-14（AG-054 后）。
- E05：GA-16、GA-15、GA-18（AG-046 后）、GA-17（GA-14 后）。
- E06/E07：GB-1 审阅视图、GB-2 测试、GB-4（GB-3 后，可 fixture 先行）。
- E08：GB-7 typed 边完整性检查器（E08 L1295 明文交付）。
- E09：AG-047（AG-048 后；修复模板等定义待用户或主控最小裁决）∥ GB-16 回放 per-episode 对齐；PR-C（AG-048 后）；GB-20 最终候选→stage_bundle 桥；GB-18 Practice 阶段计量；GB-23 世界封存与 ReplayWorldV2（GB-15/17 后）；GB-21/GB-22（L）。
- E11：GC-2 → GC-1；GC-3。
- E12：GC-6 → GC-7 → GC-8 → GC-9。
- E13：GC-11 → GC-13；GC-12。
- E14：GC-16 → GC-17、GC-20；GC-19。
- E15：GC-22 → GC-21。
- E16：GA-19、GA-21、GA-22、GA-23、GA-24、GA-25、GA-26；GA-20（待用户：新依赖）。
- 台账：GA-1（§1.4/§1.5 复核表、基线清单、迁移清单含 0006、索引与台账状态口径对齐 E00/E01/E05）随下一个台账 PR。

#### 待用户（不派）
§3.3.1 新对象容量上限；§6.1 管理操作清单（含 exploration 驱动）；tombstone 键带 kind（连同 AG-046 发现 2：storage publish_registered_blob 在首写后按 id 检查，不分 kind，同 id 他 kind 时留 Prepared 残留）；SQLite 物理擦除；真实模型供应商/协议；新依赖与 Cargo.lock（ZIP、工具链版本、去 reqwest）；首个受控任务定义；E09 修复模板/受支持修复动作/environment_reset 最小定义；KeepIncumbent 可否作为 observed_valid；内置种子清单、项目许可证、Tool-only HostSurface 映射、RSIA trace 导出方、E11 方向提示、E15 后继流统计单元、ResolvedBundle.improver v1/v2 定稿。


## 2026-09-30—10-01 v4.2 第二轮实施（PR #59–#92，runbook A–AH）：主控验收、合并与索引登记

- 真源：`RSIAgent-v4.2定稿-工程实施方案-2026-09-30.md`，SHA-256 `70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455`（每次启动与压缩续跑均先核对）；谱系 v4.1 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`（本地 `archive/2026-09-30-pre-v4.2/`）。
- 流程：每个增量一张本地任务卡（E 归属、所依据真源条款与行号、V 编号、主控裁决、文件白名单、必做测试、可声明与不可声明）；实施子代理在独立 worktree 实施并开 draft PR（base main，线性栈）；主控在全新 target 目录独立全量复跑（`out/audit-20260930/qa/ctrl_verify.sh`：fmt、clippy `-D warnings`、全工作区测试、build rsia、两组 smoke、support-scope 检查器及其 unittest），并至少做 2 个反例（尽量在基线上复现缺陷、在 head 上通过）；通过后转 ready，由用户在本机终端以 `--match-head-commit` 固定 head 合并。主控不合并。
- 结果：34 个 PR 全部合并（用户运行 `merge-all.sh`，2026-10-01T02:02Z–07:59Z）。A–AG 合并后 main `ecdfe31` 的树等于主控验收栈顶 `e7c0d0f` 的树；AH 合并后 main = `696252faf8ef2b2391102f4e6b0e9c0ee20c53d5`，其树等于验收 head `3d68d30` 的树。开放 PR 为 0。
- 声明边界：下列 verified 只对各自写明的子范围成立，不声称效果、收益或可发行；不让已验子范围掩盖各 E 的默认范围。E 任务整体状态见下文“当前 v4.2 主任务状态”。

### 合并结果（runbook 顺序；merged_sha 为 GitHub 合并提交）

| 序 | PR | 任务 | E 归属 | 子范围（摘要） | 主控验收 head | merged_sha | 索引登记 |
|---|---|---|---|---|---|---|---|
| A | [#60](https://github.com/acosmi/RSIAgent/pull/60) | AG-012 | E16.6/E00 | 派生索引、检查器、索引测试绑定 v4.2 与谱系；E09 补 V097 | `2e5c6682c33b629fd9ce9cda30578f0d4eb99866` | `dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b` | E16.6 记录 |
| B | [#59](https://github.com/acosmi/RSIAgent/pull/59) | AG-013 | E00/E07 | smoke_cli 刷新（完整 replay.run 载荷；blocked 代表改为 meta.start） | `9dec3274716ac513d29f6f6f4d2e30793d4feeca` | `69a3b3ed04377c30ee7193c0679a3747f1606a1e` | 仅台账 |
| C | [#61](https://github.com/acosmi/RSIAgent/pull/61) | AG-015 | E07 | curriculum.step 管理消费者（幂等回执、读侧重验） | `3a0e26af88744f93bc0a1874aafc5ac6f67595e4` | `b220971c4e5ad44453c9d3e5787fbdfda6eb01ac` | E07 记录 |
| D | [#62](https://github.com/acosmi/RSIAgent/pull/62) | AG-014 | E16.5 | MVP 容量门接入六个真实入口，实例级跨命名空间计数（F24） | `0d6ec3dd94954be7c01a140463b12062be686e6c` | `9c3ec979fd2ee3c53dfbf4ddf96967e94808b11e` | E16.5 记录 |
| E | [#63](https://github.com/acosmi/RSIAgent/pull/63) | AG-016 | E03 | 可信开发执行/评分回执、已登记纯函数运行器与观察门 | `b1d008f985b1568dcfce92237c8016866c4b0646` | `3e2fc067190077b8cf8401875d6c627b8d9e13fc` | E03 记录 |
| F | [#64](https://github.com/acosmi/RSIAgent/pull/64) | AG-017 | E07 | exploration.start 管理消费者（幂等登记、读侧重决策） | `71c0fa8a79e34b1cfec1c2bb61728fbfc6703138` | `f8fa1a19d5e968f83a52385ecb43d07d56442610` | E07 记录 |
| G | [#65](https://github.com/acosmi/RSIAgent/pull/65) | AG-020 | 台账 | 第一轮主控记录、合并顺序与固定 head | `eb64a4d74d82d521e41c095f6753f27d603bc35a` | `c763973ddf57c6b7babcd9bd6fa87025f367aef3` | 仅台账 |
| H | [#67](https://github.com/acosmi/RSIAgent/pull/67) | AG-018 | E16.5 | 启动恢复隔离门与部署门接入 rsia serve/mcp | `3f96c53da1003b5358d68208dc3b963977f31f7f` | `5c4f81dca79373c6871449aed287f61fb1f3cbc2` | E16.5 记录 |
| I | [#66](https://github.com/acosmi/RSIAgent/pull/66) | AG-019 | E14 | E14 增量 1：有界 ElasticPolicy、决策摘要、ImproverContentV2、MechanismUsageRecordV1 | `3dd88787cd1910ad3eafc1da50a6a883b1adba3a` | `5a0ef4419c51687534baa3296dbbf7e310a5d810` | E14 记录 |
| J | [#68](https://github.com/acosmi/RSIAgent/pull/68) | AG-023 | E00/E07 | 裸 --data 文件名加锁启动修复 | `44b4c90d89447d94024f2f5d8802013cc7412fcf` | `3ffe1e26a908d4c9240320d52fcba1e92dc383d4` | 仅台账 |
| K | [#69](https://github.com/acosmi/RSIAgent/pull/69) | AG-021 | E09 | §7.7 技能组作业编排（≤2 组、组间隔离、组合只作标记） | `c841d1a126dc51ffca424ff72e79105b4d8c5166` | `31566ce87203b44c44b72986233af1c02fbe5cda` | E09 记录 |
| L | [#71](https://github.com/acosmi/RSIAgent/pull/71) | AG-024 | E09/E08 | 探索节点、派发事实与历史接入撤销清理闭包 | `605d92abf21eedf9ef48a3704a49de14c8c2513d` | `9519c26f049afff63154fb2337893217b18b7e3c` | E09 记录 |
| M | [#70](https://github.com/acosmi/RSIAgent/pull/70) | AG-022 | E13 | 巩固：监测撤销闭包、纯 pass/fail 配对、提案持久化与单候选暂存 | `551aa33a8efdd0f92f5cfbf30820493443bbc020` | `c81b258b6f890ab78bc9c7111798f75560c2ea23` | E13 记录 |
| N | [#72](https://github.com/acosmi/RSIAgent/pull/72) | AG-025 | E12/E08 | 课程信封 9 种记录的清理分类 | `2df06e77f6dd1442fe1f04f07a6a2e8d79f5a8ee` | `15b140ddef25c8e794d586fecae56f129226a5c6` | E12 记录 |
| O | [#75](https://github.com/acosmi/RSIAgent/pull/75) | AG-028 | E13 | 巩固触发接入：E03 门控可信周期、自触发护栏、收口即触发 | `145f9cff261de6dbac9bbede1bf28080101bec3d` | `6a23cb3697f656e1f5132cbe62ae766b4c0383f0` | E13 记录 |
| P | [#73](https://github.com/acosmi/RSIAgent/pull/73) | AG-026 | E09 | §8.5 同题对比实践 PracticeAttemptSetV1 | `6a906355fdfb33a9e663123092a3938340850cbd` | `7ad4994c7a9208f523612d305660dc8ca9529620` | E09 记录 |
| Q | [#74](https://github.com/acosmi/RSIAgent/pull/74) | AG-027 | E07/E08/E12 | 管理作业与私有输入清理闭包；提交时拒绝已撤销依赖 | `d2929c7e16c0f535068922bd16945a8dbfdbcb3a` | `c72aae14c9f35c1bcc090b5a374d2696d57dacfc` | E07 记录 |
| R | [#77](https://github.com/acosmi/RSIAgent/pull/77) | AG-030 | E13 | V096.a 巩固调用单独计量（Consolidation 预算阶段） | `400f707805084199a9046008a52a2afd6c038f96` | `887b1d714581ae8a71d45243d38be1759697600d` | E13 记录 |
| S | [#76](https://github.com/acosmi/RSIAgent/pull/76) | AG-029 | E16.5/E08 | 本轮新增动作、幂等与失效事实的恢复覆盖 | `43984ab3ecc0833e435bb25ddc4b6b3f2adadbf5` | `7120842a1f586ae394b76e146c8298f11bec52bc` | E16.5 记录 |
| T | [#78](https://github.com/acosmi/RSIAgent/pull/78) | AG-031 | E13 | 巩固终态类别有界 | `892ed1cebaf50ea3a7ae2a38ddd05d70313637b1` | `84d8a7054c2428db0f821fcd478c615b09ade76b` | E13 记录 |
| U | [#79](https://github.com/acosmi/RSIAgent/pull/79) | AG-032 | E10/E07/E08 | 回放读路径点名脱敏；提交检查比对 tombstone 的 source_kind | `8954b6c86cbe1bc1a5c0b957828080b59f1d1704` | `b262bbb2a0ea9ead68bc33fe1e58235e85bcffea` | E10 记录 |
| V | [#80](https://github.com/acosmi/RSIAgent/pull/80) | AG-033 | E09 | run_next 可信观察门与证据标签 | `04e63092f2995c3668e14640c985012fd6a6a2b2` | `9e3e350ed5fbef2975117f12a7f8d3ca3b9cdcbf` | E09 记录 |
| W | [#81](https://github.com/acosmi/RSIAgent/pull/81) | AG-034 | E14 | E14.2a MetaTrial 分叉（程序范围） | `c5c04bad5c349f00c815bb02c9a3eccc439b0da7` | `d9b3f6f5234ad14da18956a78c804e4c6818fa10` | E14 记录 |
| X | [#83](https://github.com/acosmi/RSIAgent/pull/83) | AG-035 | E08 | 撤销清理只在闭包不动点处完成 | `fee5897ff45070fc198c0f46daac9e364ce7e38d` | `51db9b6c9a251def4e6566c96641ed8b63d43a85` | E08 记录 |
| Y | [#82](https://github.com/acosmi/RSIAgent/pull/82) | AG-036 | E08 | 同 id 第二撤销源点名拒绝；run 与 import_source 不再碰撞 | `f15a42fe94426a49e8967869f8087a99837fd4a5` | `8d959dee844fb16f0763b0684ba3adf48ad75f1f` | E08 记录 |
| Z | [#85](https://github.com/acosmi/RSIAgent/pull/85) | AG-038 | E08/E04 | 已撤销来源的预算调用不预留、不派发，结算照常入账 | `e06bfdd513efbbf87952bd8c822e2de12e803df7` | `f4b1ef66340412f5c0edf69a48549cd1386324cf` | E08 记录 |
| AA | [#84](https://github.com/acosmi/RSIAgent/pull/84) | AG-039 | E09 | 合法动作与前缀卫生（Deepen 要求 Trusted、X1、X2） | `d4837f5f53c4ecae67f8e528b3e4e9f06758ef0a` | `42f89bd602db0c2783de38d87d91e80cd65f220b` | E09 记录 |
| AB | [#86](https://github.com/acosmi/RSIAgent/pull/86) | AG-037 | E08/E10 | 经济回放记录纳入清理分类；start 写作业前重验来源 | `0c1bacd1752e20a7f1de2ab6058fec6271781d8f` | `16bdb08e1ac118e3e732a912358add468c49173b` | E08 记录 |
| AC | [#88](https://github.com/acosmi/RSIAgent/pull/88) | AG-043 | E08/E04 | 撤销后到达的模型响应照常计费、不可用、不留明文 | `b1c1bdefaf8e538f36287ca9b287c3aa2f41f18d` | `ca117c45d87986500b38bc5afee3d6e161fea4f5` | E08 记录 |
| AD | [#87](https://github.com/acosmi/RSIAgent/pull/87) | AG-040 | E09/E07 | 登记指纹只含不可变字段；计数器按登记时值还原并绑定成本 | `c44aa8d7db9d79f0b62379239e06184e49b5f73b` | `32a2f5090fe5136b21c7d3454f53964b36990247` | E09 记录 |
| AE | [#89](https://github.com/acosmi/RSIAgent/pull/89) | AG-044 | E07/E08 | 提交时上游闭包两阶段闸门 | `f7946f02407bb5694f08bd9f357425e2f1ad2262` | `cc85b5c71cec06763909fb0efa33c4e296b6178d` | E07 记录 |
| AF | [#91](https://github.com/acosmi/RSIAgent/pull/91) | AG-041 | E09 | 付费后路径收敛为终态、确定性失败付费前拒绝、fact 与节点绑定 | `4254e6d9408dfe4bdba4526a19de4987e6244198` | `a38b1452ede8fc5755bcab65686edd2f2817b51a` | E09 记录 |
| AG | [#90](https://github.com/acosmi/RSIAgent/pull/90) | AG-045 | E08 | 撤销后写入的 stage fact 以脱敏形态落盘 | `e7c0d0f94c2915cc95663f8673d6c62f87ada2e6` | `ecdfe31faabb21a368f08d341b2a6e15223b0849` | E08 记录 |
| AH | [#92](https://github.com/acosmi/RSIAgent/pull/92) | AG-042 | E09 | 恢复计数改为派生视图、与回放同口径 | `3d68d30a43c3fc0582053d48cf14ac35c3d4e841` | `696252faf8ef2b2391102f4e6b0e9c0ee20c53d5` | E09 记录 |

- #65 的验收 head 为 `f752e1d`；因 #60 先合并造成 criss-cross 合并基，用户在其上加了一个以 `f752e1d` 与当时 main `f8fa1a1` 为父、树保持 `f752e1d` 不变的提交 `eb64a4d`，再以它为固定 head 合并（主控只读核对树与父提交）。
- 第一轮 #59–#65 的主控记录见下文“2026-09-30 第一真源前置审计”节；第二轮全部主控记录按时间顺序折入下文，每条结论后补记 merged_sha。

### CTRL-AG014 补记 / PR #62：MVP 容量门接入六个真实入口（第一轮记录只有任务表一行，此处补全）

- 真源：v4.2 E16.5、§3.3.1（MVP 容量初值）、§11.4。固定 head `0d6ec3dd94954be7c01a140463b12062be686e6c`（= 主控重叠 `d8bfdd5` + F24），叠在 #61 `3a0e26a` 之上。改动：capacity.rs、dispatch.rs、exploration.rs、packages.rs、service.rs、evo-storage budget.rs/lib.rs 及相应测试。
- 主控返修 F24：容量计数必须是实例级、跨命名空间，不能按命名空间拆分。主控反例 `ctrl_ag014_lease_cap_cannot_be_split_across_namespaces` 通过（`qa/ctrl-ag014-adversarial.log`）；全工作区复跑见 `qa/ctrl-ag014-workspace.log`，栈顶 `71c0fa8` 复跑 48/536/0。
- 2026-10-01 索引登记复跑：在 `0d6ec3d` 上 `cargom test --locked -p evo-engine --test capacity_v41` 23 passed / 0 failed（`qa/ctrl-reg-ag014-0d6ec3d.log`）。
- 结论：verified（E16.5 容量门接入真实入口子范围）；启动恢复/部署门当时未接（由 AG-018 #67 完成）；无真实沙箱。已合并，merged_sha `9c3ec979fd2ee3c53dfbf4ddf96967e94808b11e`。

### 第二轮主控记录（逐项折入，按时间顺序）

#### 闸门与现状实查
- 真源闸门：`RSIAgent-v4.2定稿-工程实施方案-2026-09-30.md` SHA-256 = `70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455`（一致）；谱系 v4.1 `archive/2026-09-30-pre-v4.2/…` = `45f3ba06…`（一致）。
- 实查结果（gh pr list / git worktree list）：#60 `2e5c668`、#59 `9dec327`、#61 `3a0e26a`、#62 `0d6ec3d`、#63 `b1d008f`、#64 `71c0fa8`、#65 `099a99a` 均为 open、base=main、非 draft、MERGEABLE/CLEAN；main = origin/main = `0c43e0f`；均未合并。与交接一致。
- 与交接的差异：
  1. 模拟按序合并（`git merge-tree --write-tree` 链式模拟 #60→#59→#61→#62→#63→#64→#65）发现：#60 合并后 #65 在 `docs/implementation-ledger.md` 冲突（两处 hunk：顶部审计节任务表、E16.5 状态行），原 G 命令会失败。
  2. `user-commands.md` 中 D（#62）与 F（#64）的 `--match-head-commit` 为 7 位短 SHA，已改为完整 SHA。

#### CTRL-AG020-R2 / PR #65：台账 PR 重叠 #60
- 处置：在 #65 分支合入 #60 head `2e5c6682c33b629fd9ce9cda30578f0d4eb99866`；两处冲突均取 #65 一侧（#65 为 #60 台账文本的超集），解决后台账与 `099a99a` 逐字节相同；树 = `099a99a` + #60 的索引/检查器/索引测试 4 个文件。新 head `f752e1db7df0e1d1231ec3e62bbc0c2acd72aca5`（快进推送；先转 draft、推送、再转 ready）。
- 复跑（最终整树 = #60 + #59…#64 + 台账；`qa/ctrl-stack-final-f752e1d.log*`）：fmt 0；clippy 全工作区 -D warnings 0；全工作区 48 个测试二进制 540 通过 / 0 失败（71c0fa8 为 536，+4 为 #60 support_scope 新增）；`cargo build -p rsia` 0；`smoke_workspace.py` 0；`smoke_cli.py` SMOKE_CLI_OK；`check_support_scope.py --source-of-truth <v4.2>` structure_valid=true、plan_binding_available=true、plan_version v4.2、errors=[]；checker unittest 48 OK。
- 结论：#65 可在 #60…#64 之后无冲突合并；固定 head `f752e1d`。（后因 criss-cross 合并基改以 `eb64a4d` 合并，merged_sha `c763973ddf57c6b7babcd9bd6fa87025f367aef3`，见本节合并结果表。）

#### AG-018 派发（E16.5 启动恢复门/部署门接入 rsia serve/mcp）
- 侦察完成：代码事实详见侦察报告要点（Store::open 必写库且无只读变体；restore.json 为 rsia.restore_receipt.v2 且位于被验目录；capacity.rs 恢复/部署门与 IsolationPolicy 在生产代码零调用；无 sandbox/code-execution CLI/env 入口；迁移 0001/0002/0004/0005/0006，0003 保留）。
- 主控裁决（任务卡 cards/AG-018.md，SHA-256 见下）：启动门在 Store::open 前只读执行；锚只能由 CLI --trusted-revocations-db 给出（不信 restore.json 内 path_local_only）；首启与锚逐项等值（水位/墓碑/受保护账目/root_budget*/迁移集），通过后在 bootstrap 成功后写 restore.admission.json（绑定 restore.json 摘要，不覆盖、不删收据）；失败一律 recovery_quarantine 零写入；代码执行在 CLI 层无入口，部署门拆分后在两条路径都真实调用；不新增迁移。
- 卡摘要：72f450f0efb96c8ffda088534d5db9059e0371af39b357d5553bdc5da061e828（_common.md 23f0e86f49427bef950834367bade4b3bb201875e5b477d50844cb8b7f831208）。

#### AG-019 派发（E14 增量 1：单机制 ExplorationPolicy 受限内容、有界参数与真实决策使用证据）
- 侦察要点：ElasticPolicyV1::validate 要求等于默认值（任何候选无法存在）；CoordinatorDecision 无 policy/caps 摘要；meta.rs 以 used_in_next_job 布尔为唯一门，且同时放行 generation 与 exploration 两类；ImproverContent*/MechanismUsageRecord 全仓库不存在；policy 不在世界 context_signature 中（符合 §7.4.1/V094.c）；meta.start 为 blocked 代表。
- 主控裁决（cards/AG-019.md）：validate 改为有界区间（gain [1000,100000]；stagnation_abs [0,50000] 且 < gain；window [1,2]；focus [1,4]；fairness [1,11]），每个边界由代码事实推出（前缀只保留 2 条增益、零值退化、caps 可达范围），不增策略字段；决策携带 policy_digest/caps_digest（fingerprint 约定与 replay 同值），dispatch fact 升 v2，v1 显式拒绝；ImproverContentV2 只开放 exploration_policy 一类，其余类别分类报错（未开放 Invalid、acquisition 无消费者、受保护控制面 Forbidden），不进 bundle 与世界签名；FIELD_CONTRACTS 登记 5 个字段，consumer=exploration.decide_elastic；MechanismUsageRecordV1 为只可序列化的派生视图，只由 Observed（节点真实存在）或 Uncertain 的派发事实经校验生成，Claimed 与纯 decide 不计；删除 used_in_next_job，改由 verify_mechanism_inheritance 依据真实使用记录判定；meta.start 仍 blocked，不做 MetaTrial/批准登记。
- 卡摘要：57f47ab6a7c54092a4efdfb89f5ced868571e1aec8129a7ded239f1f1671fd94。

#### CTRL-AG018-R1 / PR #67：E16.5 启动恢复门与部署门接入 rsia serve/mcp（主控独立验收）
- 真源：v4.2 E16.5、§11.3、§11.4、§11.5 末段、E08、E04；V018/V069/V075/V085/V086.d/V007（只覆盖程序子范围）。固定 head `3f96c53da1003b5358d68208dc3b963977f31f7f`（`51a2a55` 为实现，`3f96c53` 只改测试辅助函数的注释），叠在 #65 `f752e1d` 之上，PR base=main。改动 8 个文件 +4139/−9：apps/rsia/src/main.rs、evo-engine startup_gate.rs（新）/capacity.rs/lib.rs、evo-storage lifecycle.rs（只读读取器）、新测试 startup_gate_v42.rs 与 control_plane_facts.rs、smoke_cli.py。
- 实现要点：门在加锁前评估一次（拒绝时零写入，连 .rsia.lock 都不创建），在锁内再评估一次（据此行动）；bootstrap 之后、recover_pending 与对外服务之前写 restore.admission.json（临时文件 → fsync → hard_link，不覆盖）。锚只能取自 CLI，须为绝对且 canonical 的路径、nlink==1、(dev,ino) 与 D 的库不同、不在 D 内、同目录无 backup-manifest.json、其 .rsia.lock 未被持有。两库都只读打开（query_only、integrity_check），迁移集复用备份侧同一比较。首启时水位、墓碑、受保护账目与 root_budget* 必须与锚逐项相等，D 的水位还须等于收据所记。部署配置的构造签名不接受任何代码执行、沙箱或网络输入，隔离事实取 reference_host，两条路径都真实调用部署门。
- 主控复跑（`qa/ctrl-ag018-3f96c53.log*`）：fmt 0；clippy 全工作区 -D warnings 0；全工作区 50 个测试二进制 585 通过 / 0 失败；build 0；smoke_workspace 0；smoke_cli SMOKE_CLI_OK（含新增的恢复门与真实 admission 端到端）；检查器 structure_valid、plan v4.2、errors=[]；unittest 48 OK。
- 主控反例（`qa/probe_ag018.py`，真实二进制 + 真实 restore_backup.py，`qa/ctrl-ag018-adversarial.log`）9/9：C1 锚路径经符号链接的祖先目录 → 隔离（not canonical）；C2 含 `..` 的锚路径 → 隔离；C3 restore.json 被换成目录 → 隔离（not a regular file）；C4 设置 RSIA_ALLOW_CODE_EXECUTION 等 4 个环境变量后，姿态行仍为 code_execution=disabled；C5 D 的库是指向锚的符号链接 → 隔离（same device and inode）；C6 伪造 admission 记录（摘要正确但带多余键）→ 隔离；C7b 只在 D 中修改墓碑 reason → 隔离（tombstones differ）；C7a 两侧插入相同墓碑 → 验证通过；C8 正对照 → restored_verified 并写入 admission。
- 观察（非阻断）：compare_facts 用 "{ns}/{id}" 拼接字符串作比较键。标识符字符集 [A-Za-z0-9-_.:] 不含 "/"，而可信锚一侧满足标识符约束，所以这种拼接不会造成误放行。今后可改为按元组键比较、只在报错信息中拼接。
- 实施方如实披露的边界：手工删除 restore.json 可绕过此门（属管理员行为）；未驱动 pending revoke_cleanup_jobs；启动时未把残留 dispatched 调用转为 uncertain；allow_external_network 未做一致性检查；只在 macOS 实测；无 WAL 旁文件的库在只读打开时使用 immutable。
- 另发现的既有缺陷（不属本 PR）：`rsia serve --data rsia.sqlite3`（裸文件名默认值）在 DataDirectoryLock::acquire 对空父目录做 canonicalize 时失败——另立增量。
- 结论：verified（E16.5 启动恢复隔离门与部署门接入子范围）；E16.5 整项仍 in_progress（真实沙箱、Linux 实测、清理驱动与出站一致性仍缺）。PR 已转 ready；须在 #65 之后合并。回滚点 `f752e1d`。（已合并，merged_sha `5c4f81dca79373c6871449aed287f61fb1f3cbc2`）

#### CTRL-AG019-R1 / PR #66：E14 增量 1（主控独立验收，第一轮）
- 固定输入 head `4ea62292c8646e37f0ae779e3dfd72328cb8d332`（基线 f752e1d，1 个提交，11 个文件 +3676/−60）。
- 主控复跑（`qa/ctrl-ag019-4ea6229.log*`）：fmt 0；clippy 0；全工作区 50 个测试二进制 577 通过 / 0 失败；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例（`qa/ctrl-ag019-adversarial.log`，临时测试不入库）5/5 通过：P1 class 拼写变体（大小写、连字符、首尾空格、驼峰）全部拒绝；P2 在 policy 内或 mechanism 同级夹带控制面键（max_nodes、w_online、caps、recovery_penalty_micros）全部拒绝；P3 abs==significant 拒绝，abs=significant−1 接受；P4 顶层重复 mechanism 键被拒；P5 经 MetaCandidate 派生路径绕过 parse 时，越界 policy、tagged enum 内重复 class、v3 schema 仍被拒绝。编译期伪造检查：`serde_json::from_str::<MechanismUsageRecordV1>` 编译失败（E0277，未实现 Deserialize），无法从 JSON 伪造使用记录。
- 退回返修 R1（实施方在设计偏差 1 中自报的升级风险，主控核实）：recover_pending 会反序列化全部管理作业（含终态作业）。已成功的旧形状 exploration.start 结果缺少两个新增必填摘要字段，导致 "damaged management job" Conflict，整个服务无法启动（数据库先在 #64 上运行、再升级到 #66 即可触发）。裁决：只给 `ManagementResult::ExplorationStarted` 的两个字段加 serde(default)，status 对旧结果返回作业级 Conflict；CoordinatorDecision 与 dispatch fact v2 保持严格。已发回原实施方在原 PR 追加提交。

#### AG-021 / AG-022 派发（叠在 AG-018 #67 head 3f96c53 之上）
- AG-021（E09 §7.7 技能组作业编排，≤2 组、组间隔离、只产生组合标记）：卡 cards/AG-021.md（SHA-256 6b3697b516a74479b600038224c84a2640f5d5fa7bf251ab247c481a75adef23）。
- AG-022（E13：修复 monitoring 撤销清理闭包缺陷 D1、四类改为纯 pass/fail 并逐任务配对 D4、巩固提案与暂存桥 D2）：卡 cards/AG-022.md（SHA-256 59deae8722927e133043848e5134d699ae8e0b24ffe1b49c6df16c2ebc75053c）。清理分类：redact 为 environment/observation/development_cycle v1+v2/report_binding/claim/proposal；preserve 为 scope/run/drift。

#### CTRL-AG019-R2 / PR #66：R1 返修验收与线性叠放
- R1 返修提交 `558d9a5`（普通快进推送；dispatch.rs +9/−2：只给 ExplorationStarted 的 policy_digest/caps_digest 加 serde(default)；meta_inheritance_v42.rs +132/−38：新增测试，把旧形状的已成功作业与一个非终态作业放在一起 → recover_pending 返回 Ok(1)、只拾取非终态作业，旧作业 status 返回 Conflict 而非 Internal，终态不被改写，新结果仍带非空摘要）。实施方自测 578/0，并做了变异检查（去掉任一 serde(default)，新测试即失败）。CoordinatorDecision 与 dispatch fact v2 仍然严格，v1 仍显式拒绝。
- 已知语义（已写入 PR）：旧作业用同一 request_key 重连时，返回存储中的原结果（两项摘要为空），这是所有终态作业重连的既有行为；HTTP GET 状态走 status，返回 409。
- 线性叠放：两个提交从 f752e1d rebase 到 AG-018 head 3f96c53 之上，得到 `9884a98`/`3dd8878`。AG-019 补丁在叠放前后逐字节相同（git diff f752e1d 558d9a5 == git diff 3f96c53 3dd8878）。以 force-with-lease（锁定 558d9a5）推送到 draft 分支后，PR 转 ready。
- 栈顶复跑（`qa/ctrl-stack-tip-3dd8878.log*`，整树 = #60 + #59…#65 + #67 + #66）：fmt 0；clippy 0；全工作区 52 个测试二进制 623 通过 / 0 失败（540 + AG-018 45 + AG-019 38）；build 0；smoke_workspace 0；smoke_cli SMOKE_CLI_OK（含 AG-018 真实恢复 admission 端到端）；检查器 structure_valid、plan v4.2；unittest 48 OK。
- 结论：verified（E14 增量 1：有界 ElasticPolicy、决策携带 policy/caps 摘要、ImproverContentV2 单机制类别、MechanismUsageRecordV1 仅由真实派发派生、删除 used_in_next_job 布尔的结构/程序子范围）。不证明继承收益；meta.start 仍 blocked；E14 整项仍 planned→in_progress。固定 head `3dd88787cd1910ad3eafc1da50a6a883b1adba3a`（已合并，merged_sha `5a0ef4419c51687534baa3296dbbf7e310a5d810`）；须在 #67 之后合并。回滚点 `3f96c53`。

#### AG-023 派发（E00/E07 缺陷修复：默认 --data 裸文件名无法加锁启动）
- 缺陷由 AG-018 实施方实测发现，主控复核代码：`DataDirectoryLock::acquire` 对 parent 为空串的路径调用 canonicalize("") 失败。启动门中已有唯一正确的定义 `startup_gate::data_directory`，裁决加锁路径改用这一处定义。卡 cards/AG-023.md（SHA-256 59fb46b7b02c3131d5dd330ee31ae371c33d00a19dae7ae646ccf47d41a404a8），基线 3dd8878（栈顶）。

#### CTRL-AG023-R1 / PR #68：裸 --data 文件名加锁启动修复（主控独立验收）
- 固定 head `44b4c90d89447d94024f2f5d8802013cc7412fcf`（叠在 #66 `3dd8878` 之上，1 个提交；apps/rsia/src/main.rs +82/−9，scripts/smoke_cli.py +104）。DataDirectoryLock 改用 `startup_gate::data_directory`（唯一定义）；新增 4 个单元测试；smoke 新增 smoke_bare_data_path（子进程 cwd 为临时目录）。
- 实施方如实披露：mcp 在 stdin 为空时以 "connection closed: initialize request" 非零退出，这是 evo-mcp 的既有行为，卡中"正常退出"的表述不准确；smoke 只接受退出码 0 或该握手错误。Store::open 自身仍独立计算父目录，但对裸文件名实测可用。
- 主控复跑（`qa/ctrl-ag023-44b4c90.log*`）：fmt 0；clippy 0；全工作区 52 个测试二进制 627 通过 / 0 失败（623 + 4）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例（`qa/probe_ag023.py`，`qa/ctrl-ag023-adversarial.log`）6/6：CE1 用修复前二进制（3dd8878）复现 "failed to resolve data directory"，新二进制可启动并创建 rsia.sqlite3 与 .rsia.lock；CE2a 真实恢复目录中以 cwd=D、裸 --data 且不带锚 → 隔离，零写入（连 .rsia.lock 都没有）；CE2b 同目录带绝对路径锚 → restored_verified 并写入 admission；CE2c 再次启动 → admitted_previously；CE3 `--data ""` → clap 报错退出 2，无 panic、无文件；CE4 相对路径锚 → 隔离（must be absolute）。
- 结论：verified（E00/E07 CLI 默认数据路径的加锁与启动子范围）。PR 已转 ready；须在 #66 之后合并。回滚点 `3dd8878`。（已合并，merged_sha `3ffe1e26a908d4c9240320d52fcba1e92dc383d4`）

#### AG-024 派发（E09/E08 缺陷：探索节点/派发事实/优化历史不在撤销清理闭包内）
- 来源：E09 侦察 §6，主控在栈顶 44b4c90 核实：exploration.rs 只有 persist_world_registration 写依赖边，节点、派发事实与历史记录都不写边，撤销时清理遍历只能到达世界本身，这三类记录尽管在 redact 白名单内也永远不会被脱敏（读侧靠水位严格相等 fail-closed）。裁决：首次写入时在同一事务内写入指向所属世界的依赖边；不改 lifecycle.rs 与任何 schema；不做回填（没有生产写入者）。卡 cards/AG-024.md（SHA-256 a03835fea111bada4fd67cbb9c9e51dc0a5c7168268ef51b7b6805d799f4fa1a），基线 44b4c90。

#### CTRL-AG021-R1 / PR #69：E09 技能组作业编排（主控第一轮，返修中）
- 交付 head `28ee9bbb90253bcd54fb1bd950ed836a2a5f2ebd`（基线 3f96c53；groups.rs 新增 335 行、lib.rs +1、skill_groups_v42.rs 新增 1979 行，10 个测试）。实施方自测 595/0，21 处变异全部被测试杀死。
- 裁决：R1 返修——不采用"剥离模板预演 + 匹配 empty_evidence_closure 错误文本"（依赖检查顺序与错误文本，属脆弱耦合）。evo-core skill_edit.rs 已有私有的 `validate_batch_scope` 做同一件事，授权只改为 pub（函数体与行为不变），groups.rs 直接调用。R2 不返修——一个作业只接一个模型端口和一个运行器，预算绑定在端口上，两组结构上必然共享同一根预算，卡中"请求级 billing_scope/root_budget 校验"的前提有误，由主控承担，测试 6 用真实 broker 证明共享。
- 环境事故（实施方如实披露）：父会话 scratchpad 根目录的 env.sh 被 AG-022 子代理覆盖。AG-021 在 13:23–13:25 期间 source 了被覆盖的版本，把 ag-021 源码的构建产物写进了 target-ag-022。实施方随后想删除相关 fingerprint，被权限分类器以 "Interfere With Workloads" 拒绝，主控不代为删除。处置：通知 AG-022 在其自身环境对 evo-core/evo-storage/evo-engine 执行 `cargom clean -p` 后重跑最终自测；主控对两者的验收都改用全新的 target 目录；今后子代理的临时文件一律放在 scratchpad/<任务>-impl/ 子目录中。

#### CTRL-AG021-R2 / PR #69：返修验收与线性叠放
- 返修 `702a343`：只把 skill_edit.rs 的 `validate_batch_scope` 改为 pub（+1/−1，函数体未改），groups.rs 直接调用它，删除剥离模板预演与错误文本匹配；零调用拒绝用例增至 31 项；evo-core 165/0 无行为变化。R2 的"预算结构性共享"已写入 PR。
- 线性叠放：两个提交从 3f96c53 rebase 到 44b4c90（#68）之上，得到 `13c4dab`/`c841d1a`，补丁逐字节相同；以 force-with-lease（锁定 702a343）推送后转 ready。
- 主控复跑（全新 target 目录 target-ctrl-ag021，`qa/ctrl-stack-ag021-c841d1a.log*`）：fmt 0；clippy 0；全工作区 53 个测试二进制 637 通过 / 0 失败（627 + 10）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例（`qa/ctrl-ag021-adversarial.log`，临时测试不入库）2/2：G6 第二个作业沿用 group-a 的 episode/request id 但来源内容不同 → group-a 记为 failed（"state conflict: same optimization key has different full input"），没有用旧 journal 冒充结果，也没有新的模型或运行器调用；group-b 与第一次结果一致。G18 全新库上不接端口运行 → 两组都记为 failed，combined 为空，作业本身不报错。
- 结论：verified（E09 §7.7 技能组作业编排程序子范围：≤2 组、组间隔离、调用前全量校验、组合只作标记）。不含组合 Bundle 编译、完整组合开发选择或正式验收。固定 head `c841d1a126dc51ffca424ff72e79105b4d8c5166`（已合并，merged_sha `31566ce87203b44c44b72986233af1c02fbe5cda`）；须在 #68 之后合并。回滚点 `44b4c90`。

#### CTRL-AG024-R1/R2 / PR #71：探索节点、派发事实与历史接入撤销清理闭包（主控独立验收）
- 交付：`6288f63`（边 + 测试）、`dbef300`（测试拆分与晚到结果覆盖）、`f164778`（R1：脱敏记录返回点名的 Conflict，不再是 Internal）；基线 44b4c90。exploration.rs +107/−6（put_world_edge 在节点、派发事实、历史首次写入的同一事务写边；read_envelope 识别 rsia.redacted.v1 墓碑），新测试 exploration_cleanup_v42.rs（15 项）。历史条目的归属已确认属于世界。不回填（没有生产写入者）。
- 第一轮裁决：清理完成后五个读入口返回 Internal，不符合错误契约 → R1 返修，按 development.rs/optimization.rs 的惯例返回 Conflict；真正的损坏仍为 Internal，并有区分测试。实施方改为先读原始正文、再判断墓碑，避免对预期状态记 error 日志，结果与裁决一致，主控接受。
- 线性叠放：三个提交从 44b4c90 rebase 到 c841d1a（#69）之上，得到 `06e4e01`/`5a3152f`/`605d92a`，补丁逐字节相同；以 force-with-lease（锁定 f164778）推送后转 ready。
- 主控复跑（全新 target 目录 target-ctrl-ag024，`qa/ctrl-stack-ag024-605d92a.log*`）：fmt 0；clippy 0；全工作区 54 个测试二进制 652 通过 / 0 失败（637 + 15）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例（`qa/ctrl-ag024-adversarial.log`）：E1 首版因测试世界的水位过时，被水位检查提前拦下，结论不成立，已记录；E1b 以当前水位重做：正对照中新 id 注册成功，而用已脱敏的 id 通过 register_world 与 register_world_idempotent 重新注册，都返回点名脱敏的 Conflict，墓碑保留，无法复活；E2 向已脱敏世界追加历史返回点名脱敏的 Conflict。
- 结论：verified（E09/E08 探索记录撤销清理闭包与脱敏读取错误契约子范围）。已知边界：分页遍历期间新写入的边可能落在游标之前、修复前的记录不回填、只在首次写入时写边。固定 head `605d92abf21eedf9ef48a3704a49de14c8c2513d`（已合并，merged_sha `9519c26f049afff63154fb2337893217b18b7e3c`）；须在 #69 之后合并。回滚点 `c841d1a`。

#### CTRL-AG022-R2 / PR #70：返修验收与线性叠放
- 第一轮（bc033df，全新 target）：647/0 全绿。主控反例 4 项中 P2 与 P3 通过，P1 与 P4 暴露问题：P1 同一提案可用不同 candidate_id 重复暂存（出现 2 条候选，给"评测失败后换 id 重评"留出空间）；P4 暂存结果直接沿用存储中被篡改的 provenance。已退回返修。
- 返修 `49cc157`：新增持久绑定 `rsia.monitoring.consolidation_staging.v1`（每个提案一条，在暂存前、与提案校验同一事务写入；同 id 幂等，换 id 为 Conflict；确定性拒绝时退回预留，I/O 未知时保留预留；绑定带指向提案的边，lifecycle 追加 redact 分支）。暂存时用 claim 的周期与环境记录重新推导 provenance，并同时推导 environment/pairs/parent 等摘要，不等即 Conflict。新增 5 个集成测试和 4 个单元测试，实施方自测 656/0，40 次重复运行零失败。
- 实施方如实披露：一个提案一个候选的约束只由暂存桥执行，ReleaseStore::stage_bundle 本身未改，持有 bundle 的 Worker 或 Admin 仍可直接调用；暂存时不与 journal 终态事实做 bundle 交叉核对；此前已披露的边界不变。
- 线性叠放：两个提交从 44b4c90 rebase 到 605d92a（#71）之上，得到 `4f9d2b7`/`551aa33`，补丁逐字节相同；以 force-with-lease（锁定 49cc157）推送后转 ready。
- 主控复跑（主控专用 target 目录 target-ctrl-ag022，`qa/ctrl-stack-ag022-551aa33.log*`）：fmt 0；clippy 0；全工作区 55 个测试二进制 681 通过 / 0 失败（652 + 29）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例第二轮（`qa/ctrl-ag022-adversarial-r2.log`）4/4：P1 换 id 暂存返回 Conflict 并点名 candidate-1，同 id 保持幂等，存储中只有 1 条候选；P2 外命名空间与 Agent 角色被拒后，正当暂存方仍能暂存，没有残留的预留；P3 篡改 bundle 被 digest 校验拒绝；P4 篡改 provenance 返回 Conflict（"provenance differs from its evidence"），不产生候选。
- 结论：verified（E13 巩固子范围：monitoring 撤销清理闭包、纯 pass/fail 逐任务配对 v2、巩固提案持久化、一个提案至多一个暂存候选、可信标签在暂存时重新推导）。巩固调用计费阶段、E03 可信周期收口、管理入口与任务族保留矩阵仍未做。固定 head `551aa33a8efdd0f92f5cfbf30820493443bbc020`（已合并，merged_sha `c81b258b6f890ab78bc9c7111798f75560c2ea23`）；须在 #71 之后合并。回滚点 `605d92a`。

#### AG-025 / AG-026 派发与 E13 触发侦察
- AG-025（E12/E08 缺陷：课程信封的 9 种 record_kind 不在清理白名单中，撤销后作业会 Failed；由 AG-022 实施方实证）：卡 cards/AG-025.md（SHA-256 883fd2e56bab79139a50c39785c9f30cb2b5bb86882817530d14a454fa986234），基线 bc033df。分类：来源派生的 6 种 redact，profile/job/receipt 3 种 preserve。
- AG-026（E09 §8.5 同题对比实践 PracticeAttemptSetV1 程序子范围）：卡 cards/AG-026.md（SHA-256 e6cdc2cc9a0a97b13fb346b43739d1b6b7fb2140e1bda35eb9a76a85c4065276），基线 551aa33。裁决：attempt 不写成开发观察事实（防止 E12/E13 把它们计为独立周期）；K=3 须有 Admin 持久化的登记；独立簇数只按任务族计算，绝不含 K；缓存命中不计为新实践；沿用 DevelopmentExecution 记账并披露 Practice 阶段尚未使用。
- E13 "巩固触发接入"：monitoring.rs 经 AG-022 大改，已在栈顶 551aa33 派出只读侦察（可信周期路径、自触发守卫、Consolidate 预算阶段、周期完成即触发的可行性，以及真源 §6.1 的管理操作清单是否包含巩固操作）。

#### CTRL-AG025-R1 / PR #72：E12 课程信封接入撤销清理闭包（主控独立验收）
- 交付 head `2df06e77f6dd1442fe1f04f07a6a2e8d79f5a8ee`：实施方发现 #70 已改栈，自行把单个提交 rebase 到 551aa33，无冲突，属合理处置。lifecycle.rs +25（preserve：profile/job/receipt；redact：state/cycle receipt/attempt/proposal/validity/selection），新测试 curriculum_cleanup_v42.rs（9 项）。9 种记录逐一核对了内容与依赖边：preserve 类不含原文、模型输出或答案（job/receipt 只含 state/trigger 的 SHA-256 摘要）；无需补边，curriculum.rs 未改。
- 主控复跑（全新 target 目录 target-ctrl-ag025，`qa/ctrl-stack-ag025-2df06e7.log*`）：fmt 0；clippy 0；全工作区 56 个测试二进制 690 通过 / 0 失败（681 + 9）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控确认探针（`qa/ctrl-ag025-adversarial.log`）复现了实施方披露的两项缺陷，都在本卡白名单之外：(a) 经管理面提交 curriculum.step 后撤销 run-a，清理以 Failed 结束（blocked_unknown_scope:job:rsia.management_job.v1:，已处理 51 个节点），即 management_job/management_private_input 未分类，生产路径的撤销清理仍不完整；(b) begin_revoke 之后，用已记录的 key 重放 schedule_probe_idempotent 仍返回旧 job（fail-open）。另经代码核实：课程记录脱敏后的读取返回 Internal。
- 结论：verified（E12 课程信封清理分类子范围）；上述缺陷另立 AG-027 修复，E12 生产路径的撤销闭包在 AG-027 前仍属 blocked。固定 head `2df06e7`（已合并，merged_sha `15b140ddef25c8e794d586fecae56f129226a5c6`）；须在 #70 之后合并。回滚点 `551aa33`。

#### AG-027 派发（E07/E08/E12 缺陷：管理作业与私有输入未进清理闭包；课程重放 fail-open；课程脱敏读取返回 Internal）
- 依据：AG-025 实施方披露，主控用探针复现（qa/ctrl-ag025-adversarial.log）。裁决：management_job 为 preserve（已发生动作的审计记录；ManagementResult 只含结构化字段，实施前须逐字段核对）；management_private_input 为 redact（完整请求载荷）；dispatch 读到脱敏私有输入时返回点名的 Conflict，执行中的作业以带 error_code 的 Failed 结束，recover_pending 不受影响；课程回执重放先重验水位与来源；课程脱敏读取返回点名的 Conflict。卡 cards/AG-027.md（SHA-256 f9a8cb78c6f93aec79dc9e44591b83218793d12bb4b8c649722af457ce0f5092），基线 2df06e7（#72）。

#### AG-028 派发（E13 巩固触发接入，含可信周期护栏与真实运行器绑定）
- 依据：E13 第二次侦察（栈顶 551aa33）。close_development_cycle 对真实 E03 事实会 NotFound，因为它对原始 id 执行 need，而真实回执存于 typed 存储 id 下，所以 E13 只接受手写 fixture 周期；provenance 取自自报值；巩固自身的开发运行可被当作新周期收口（自触发）；claim_consolidation 与 close 均无生产调用者；§6.1 管理操作清单中没有巩固操作。
- 裁决：不新增管理操作（新增即扩公共接口，须先修订真源）；非 Fixture 的 report 在同一 session 内过 E03 门（verified_development_observation_in_session），逐项核对 view、report 与 scope，provenance 不再由自报决定；episode_id 命中巩固 claim 即拒绝；新增 close_development_cycle_and_trigger（先 close 后 claim，claim 失败返回 Unavailable 且周期保留）；RegisteredDevelopmentRunner 实现 ConsolidationDevRunner。ModelStage::Consolidate 与预算阶段拆分另立后续任务。卡 cards/AG-028.md（SHA-256 c88c2703ea430c5b7d5fc2f9e209dd8979ec0df3f8c4fec33f236718110e9c2a），基线 2df06e7。

#### CTRL-AG026-R1 / PR #73：E09 同题对比实践（第一轮，返修中）
- 交付 head `77156f7`（基线 551aa33；practice.rs 新增 1513 行、practice_v42.rs 新增 2317 行共 21 项、lifecycle.rs +5、lib.rs +1）。实施方自测 709/0。
- 实施方的 8 项偏差全部接受：复用 E03 验证器所用的 StageFact 只作内存载体、从不落盘（有测试扫描存储作证）；集合有两个 id；请求摘要不含 created_at；不信任 runner 报告（回执、分数、control 不符都记为 incomplete）；停止原因；计划任务须等于 control 任务集；额外依赖边；对比规则按 (score, passed) 取较好与较差。
- 裁决 R1（返修）：一个 K=3 登记只授权一个实践集合，同一登记下第二个 set_id 为 Conflict；依据 §8.5"不为制造差异无限增加 K"。K=1 不设配额。
- 线性叠放：77156f7 rebase 到 #72 的 2df06e7 之上。lifecycle.rs 与 AG-025 的课程分支在两处相邻位置冲突，主控按两边都保留解决；engine 补丁逐字节不变，lifecycle 新增行不变，新 head `05801a5`，以 force-with-lease（锁定 77156f7）推送到 draft。R1 在其上追加。

#### CTRL-AG027-R1 / PR #74：管理作业与私有输入接入清理闭包（第一轮，返修中）
- 交付 head `bfe68af`（基线 2df06e7；lifecycle.rs +13，dispatch.rs +126/−8，curriculum.rs +58/−14，management_cleanup_v42.rs 新增 17 项）。实施方自测 707/0，稳定性多轮并发零失败，5 类变异全部被杀死；撤销 lifecycle 两个分支后，8 个新测试以原缺陷文本失败。
- 字段核对：ManagementJob 的 18 个字段与 ManagementResult 的 6 个变体都不含来源派生的自由文本（BatchActionV1 的 reason 只有 5 个固定字面量），按裁决把 job 分为 preserve、私有输入分为 redact。可达性：curriculum.step、exploration.start、replay.run 可达；experiment.register 仅当 holdout 输入为 run 派生时可达；evaluation.* 与 meta.start 不可达（有测试佐证不变）。
- 接受：status 闸门只对 Succeeded 作业生效且只认墓碑；新增 step/error_code 字面量 private_input_redacted、source_revoked；recover_pending 不读私有输入，因此不受影响。
- 退回 R1：清理完成之后才提交的管理请求，其私有输入会带着指向已撤销来源的边以明文落库，清理不会再跑，违背"新访问即时拒绝"。裁决在提交与持久化路径中，写入之前、同一事务内，对全部私有依赖做存活检查；已撤销或已脱敏的依赖一律拒绝，返回 Conflict，不写任何记录。
- 记为后续事项（不在本 PR 处理）：evo-storage replay.rs 的 replay 存储读路径对墓碑返回 Internal；重提交时 "subject was deleted" 文案不准确（仍 fail-closed）；E16 import_source 作为撤销源的端到端夹具。

#### CTRL-AG028-R1 / PR #75：E13 巩固触发接入（主控独立验收）
- 固定 head `145f9cff261de6dbac9bbede1bf28080101bec3d`（直接基于 #72 的 2df06e7，1 个提交；monitoring.rs +656/−24、optimization.rs 新增 1 个 pub(crate) 函数 +19、development.rs 新增 ConsolidationDevRunner 的 impl +38、新测试 monitoring_trusted_cycle_v42.rs 13 项，另有 5 个单元测试）。
- 实现：非 Fixture 的 report 在同一 session 内经 E03 门（由 observed 事实推导 request 事实 id），逐项核对 view、report 与 scope；可信路径改用 typed 回执边；周期标签由门证明，固定为 RegisteredPureFunction。自触发护栏：episode 命中巩固 claim 或其脱敏墓碑即拒绝，对所有路径生效。新增 close_development_cycle_and_trigger（先 close 后 claim，claim 失败返回 Unavailable 且周期保留）。RegisteredDevelopmentRunner 实现了 ConsolidationDevRunner。实施方变异检查：在可信路径保留原始 id 的 need 时，13 项中 8 项失败，证实"E13 原先只接受 fixture 周期"的推断。
- 实施方如实披露：真实撤销使 scope 水位变化，所以两次 close 之间的撤销会让第二次 close 直接报错，而不是返回 Unavailable；Unavailable 路径只能用手写墓碑模拟。测试环境为 ProgramFixture（TrustedExecution 环境需要走正式批准）。close 与 wrapper 仍没有生产调用者；巩固仍按 Reflection/Ranking 计费；真实链只能到 NoContrast 或 Rejected。
- 主控复跑（全新 target 目录 target-ctrl-ag028，`qa/ctrl-stack-ag028-145f9cf.log*`）：fmt 0；clippy 0；全工作区 57 个测试二进制 708 通过 / 0 失败（690 + 18）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例（`qa/ctrl-ag028-adversarial.log`）2/2：Q1 经 wrapper 重复收口同一可信观察是幂等的（同一周期、NotEligible），第二周期为 Claimed{NoContrast}，再次收口返回 AlreadyClaimed（同一 claim id），索引中 2 个周期、1 个 claim，没有多出的 generation；Q4 把真实可信观察收口到 grader 或 manifest 不同的 scope → Conflict（"cycle differs from frozen environment"），环境 id 不同 → NotFound，之后仍能收口到它真实的 scope。
- 结论：verified（E13 巩固触发接入子范围：E03 门控的可信周期、自触发护栏、周期收口即触发、真实运行器预算绑定）。ModelStage::Consolidate 的预算拆分、生产周期驱动与管理入口仍未做。须在 #72 之后合并。回滚点 `2df06e7`。（固定 head `145f9cf`，已合并，merged_sha `6a23cb3697f656e1f5132cbe62ae766b4c0383f0`）
- AG-027 R1 阻塞裁决：提交时拒绝与两个既有测试冲突（curriculum_cleanup_v42.rs:1271–1281；dispatch_management.rs:2761–2777、2793–2808），它们编码的是"提交时接受、作业内失败"的旧契约。实施方按卡的约束停下请求授权，处置正确。主控依 §11"新访问即时拒绝"裁定：采纳选项 A，仅授权修改这三处，改为断言 Conflict 并点名依赖，保留全部不写入断言，不放宽其他断言；dispatch_management.rs:1541–1556（只标记撤销的 typed source）的既有契约不动；只 begin_revoke 时存活 artifact 依赖在提交时无法判定的边界，接受并记录。

#### CTRL-AG026-R2 / PR #73：返修验收与线性叠放
- R1 `b0cc0f5`：新增持久绑定 `rsia.practice_registration_binding.v1`（每个登记一条，preserve），先只读早拒绝，在首次调用运行器前认领登记，并在持久化事务中再确认；同一登记下第二个 set_id 为 Conflict，同一 set_id 重放幂等，K=1 无配额。实施方自测 726/0，并发 40 轮零失败，变异全部命中预期测试。
- 偏离裁决的一步（主控接受）：在首次调用前认领，而非仅在持久化时写绑定。只在持久化时写，挡不住并发两个集合各跑 K 次，也挡不住中途失败后换 set_id 重跑，两者都是变相补采。代价是：认领后失败的集合也耗掉登记，续跑只能用同一 set_id，否则须重新登记。
- 线性叠放：两个提交从 2df06e7 rebase 到 #75（145f9cf）之上，得到 `dbc7b36`/`6a90635`，补丁逐字节相同；以 force-with-lease（锁定 b0cc0f5）推送后转 ready。
- 主控复跑（全新 target 目录 target-ctrl-ag026，`qa/ctrl-stack-ag026-6a90635.log*`）：fmt 0；clippy 0；全工作区 58 个测试二进制 744 通过 / 0 失败（708 + 36）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例（`qa/ctrl-ag026-adversarial.log`）2/2：S1 在第 2 次尝试 uncertain 而停止的集合（Incomplete）已耗掉登记，同一登记下另一个 set_id → Conflict 并点名 ctrl-set-a，健康运行器调用 0 次，原 set_id 重放返回已存集合且无新调用；S4 Agent 与 Evaluator 角色均为 Forbidden，运行器调用 0 次。
- 结论：verified（E09 §8.5 同题对比实践程序子范围：K=1 默认、K=3 须 Admin 登记且一个登记只授权一个集合、attempt 不计为独立样本或周期、缓存命中不计为新实践、撤销闭包）。确定性运行器只能得到 no_contrast；Practice 预算阶段、课程额度、对比规则进入有界编辑链与管理入口仍未做。固定 head `6a906355fdfb33a9e663123092a3938340850cbd`（已合并，merged_sha `7ad4994c7a9208f523612d305660dc8ca9529620`）；须在 #75 之后合并。回滚点 `145f9cf`。

#### CTRL-AG027-R2 / PR #74：返修验收与线性叠放
- R1 `c8dc2a3`：dispatch.rs 的 persist_management 在同 key 重放分支之后、任何写入之前、同一会话内检查私有依赖；依赖有 tombstone 或正文为脱敏墓碑时，整个提交返回 Conflict，点名依赖的 kind 与 id，不写任何记录。经主控授权修改三处既有测试（curriculum_cleanup_v42.rs:1267–1290 一带；dispatch_management.rs 同一测试的 2761–2854 一带），改为断言点名依赖的 Conflict，不写入断言全部保留，并补充了"对象数不变、无幂等行"断言。另新增 5 个测试。实施方自测 712/0。
- 线性叠放：从 2df06e7 rebase 到 #73（6a90635）之上，得到 `0eef4ef`/`d2929c7`；engine 补丁逐字节相同，lifecycle 新增行相同。首次强推失败：zsh 把 `$B:c8dc…` 当作变量修饰符（:c）处理，lease 被改写，git 以非快进为由拒绝；此前已误把 PR 转为 ready，随即转回 draft，改用 `${B}` 重推成功，再转 ready。事故与规避写入本地环境记忆。
- 主控复跑（全新 target 目录 target-ctrl-ag027，`qa/ctrl-stack-ag027-d2929c7.log*`）：fmt 0；clippy 0；全工作区 59 个测试二进制 766 通过 / 0 失败（744 + 22）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控探针（`qa/ctrl-ag027-adversarial.log`）：M3 命名空间 n 撤销并清理后，命名空间 m 对同名 run 的提交被接受，无跨命名空间误拒；M5 清理后同 key 重提交返回 Conflict（"subject was deleted; request cannot be replayed"），我的预期（返回已存 job）有误，实际为实施方已披露的 fail-closed 行为，只是文案不准。代码审查的非阻断观察：提交检查按 id 查 tombstone，不比对 source_kind；在现行 id 方案（带前缀的哈希存储 id 与 run id）下碰撞不可达，留作后续精度改进。
- 结论：verified（E07/E08/E12 管理作业与私有输入清理闭包、提交时拒绝已撤销依赖、课程重放与脱敏读取 fail-closed 子范围）。只 begin_revoke 时存活 artifact 依赖在提交时无法判定（作业内失败，清理后输入被脱敏），属已知边界。固定 head `d2929c7e16c0f535068922bd16945a8dbfdbcb3a`（已合并，merged_sha `c72aae14c9f35c1bcc090b5a374d2696d57dacfc`）；须在 #73 之后合并。回滚点 `6a90635`。

#### AG-029 / AG-030 派发（基线 d2929c7 栈顶 #74）
- AG-029（E16.5/E08 恢复覆盖）：本轮新增的动作、幂等与花费事实（巩固 claim/run/staging/proposal、实践登记/绑定/集合、课程信封、管理作业）不在 restore_backup.py 与启动门的受保护口径内，旧备份恢复后可能重做这些动作，违背 §11.5。裁决把这些 schema 同步加入受保护口径（Python 与 Rust 两处，靠漂移守卫保证一致），私有输入不纳入，判定规则仍是逐项相等。卡 cards/AG-029.md（SHA-256 e431e582e24c3f4fde931279ccba0138fd306dbd26ff6da079dacb2c43484b0d）。
- AG-030（E13/V096.a）：巩固调用目前记在 Reflection/Ranking 名下。裁决新增 ModelStage::Consolidate → BudgetStage::Consolidation，并在 journal 阶段新增对应变体；只有巩固请求使用该阶段（双向校验）；补五类故障注入；不改 storage。卡 cards/AG-030.md（SHA-256 39b6ca49547c859906540050bbaf46b99c89eaaa724346c7f2d455a324cd7ff9）。
- 真源层面的待决事项（须先修订真源才能实施）：§3.3.1 v4.2 补充要求"后续新增的内部对象在引入时同步登记上限"，而本轮新增的内部对象（巩固 claim/proposal、实践集合、技能组作业等）在真源中没有容量上限；按规定须先修订方案，再由 E16.5 实施与重验，不能由实现方单方设定。

#### CTRL-AG030-R1 / PR #77：E13/V096.a 巩固调用单独计量（主控独立验收）
- 固定 head `400f707805084199a9046008a52a2afd6c038f96`（直接基于 #74 的 d2929c7，1 个提交；8 个文件 +2077/−15，其中新测试 consolidation_budget_stage_v42.rs 12 项）。改动：ModelStage::Consolidate（serde "consolidate"）→ BudgetStage::Consolidation；journal 阶段新增 Consolidate；call_stage 只在 base stage 为 Consolidate 时切换；入口拆为公开的 run_optimization_step（拒绝 Consolidate）、crate 内的 run_consolidation_step（要求 Consolidate）与私有的 run_step（原函数体）；run_consolidation 在 claim 转 Running 之前校验阶段。五类 V096.a 注入（超时、取消、无效 JSON、费用缺失、晚到）都用真实 broker 与账本。金值测试钉住了旧阶段的标签与摘要，五个阶段的 cache key 互不相同。
- 越出白名单一处（主控接受）：optimization.rs 入口拆分，是在不给请求加字段的前提下双向约束阶段的唯一做法。生产调用点已核对：exploration.rs:1070 与 groups.rs:148 走公开入口，monitoring.rs:1154 走巩固入口。
- 实施方如实披露：step 级的 StepPrepared/StepCompleted 仍记在 Merge 下，只有模型调用级事实记为 consolidate；以 Merge 执行过的旧 claim 不能用 Consolidate 续跑（生产中无此数据）；开发运行器调用仍记为 development_execution。
- 主控复跑（全新 target 目录 target-ctrl-ag030，`qa/ctrl-stack-ag030-400f707.log*`）：fmt 0；clippy 0；全工作区 60 个测试二进制 778 通过 / 0 失败；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例（`qa/ctrl-ag030-adversarial.log`，与 AG-021 技能组的交互）2/2：A2 某组请求带 Consolidate 阶段 → 只有该组失败（permission denied），另一组照常产出候选，组合标记只含该组；A3 两组都带 Consolidate → 模型与运行器调用 0 次。
- 发现的既有缺陷（另立 AG-031）：monitoring.rs 的 persist_terminal_run 拒绝超过 128 字节的终态类别。合法 JSON 但含未知字段的巩固答案，其 serde 错误原因过长，导致 run_consolidation 返回 Invalid，claim 卡在 Running 且重试同样失败（资金侧安全，但缺少终态记录）。
- 结论：verified（E13/V096.a 巩固调用单独计量与阶段越权拒绝子范围）。须在 #74 之后合并。回滚点 `d2929c7`。（固定 head `400f707`，已合并，merged_sha `887b1d714581ae8a71d45243d38be1759697600d`）
- AG-029（#76）第一轮：主控复跑 777/0 全绿；反例 D2 通过（管理作业正文不同会隔离）。反例 D1 暴露卡中遗漏：锚在备份之后记录环境漂移（environment_drift.v1）时，恢复旧备份不隔离，旧备份会静默忘记 scope 已失效。已退回 R1，把 drift 加入受保护口径；consolidation_scope 索引不纳入，因为 drift 受保护后失效事实已可比较，索引中的 claim 也由受保护的 claim 覆盖。AG-029 返修后将叠到 #77 之上。

#### AG-031 派发（E13 缺陷：终态类别超过 128 字节时 claim 卡在 Running）
- 裁决：终态类别由确定性的有界函数推导（≤128 字节原样使用；超长时在 UTF-8 字符边界截断，并加上原因全文 sha256 前 16 位作后缀；空原因映射为固定字面量），所有终态写入路径统一经过它；不改 schema 与上限，不另存原因全文。交给 AG-030 实施方（有上下文）。卡 cards/AG-031.md（SHA-256 451847869aadb4a71d244efcd66309e681b9efc3a29e8f5e2172297b989ea3c6），基线 400f707（#77）。

#### CTRL-AG029-R2 / PR #76：返修验收与线性叠放
- R1 `35d0f81`：environment_drift.v1 加入受保护口径（Python 元组与 PROTECTED_ACTION_SCHEMA_VERSIONS 同步，后者由 9 项增至 10 项）；新增两个用真实 record_environment_drift 的测试（备份之后的漂移 → 恢复隔离；恢复之后的漂移 → 启动门隔离，且只点名 drift 记录）。consolidation_scope 索引仍不纳入，理由已按代码核实：claim_ids 与 invalidated_by 分别只与受保护的 claim、drift 在同一事务中写入。实施方的变异实验揭示了漂移守卫的局限：两侧同时缺同一类时守卫仍通过，所以必须有逐类覆盖测试，本 PR 已具备。
- 线性叠放：从 d2929c7 rebase 到 #77（400f707）之上，得到 `9f740e4`/`43984ab`，补丁逐字节相同；以 force-with-lease（锁定 35d0f81，使用 ${B} 形式）推送后转 ready。
- 主控复跑（主控专用 target 目录 target-ctrl-ag029，`qa/ctrl-stack-ag029-43984ab.log*`）：fmt 0；clippy 0；全工作区 61 个测试二进制 791 通过 / 0 失败（778 + 13）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例第二轮（`qa/ctrl-ag029-adversarial-r2.log`，真实 restore_backup.py）：D1 备份之后在锚上记录环境漂移 → 现在隔离（第一轮不隔离，R1 已修复）；D2 管理作业正文不同 → 隔离。
- 已知边界（实施方披露，主控接受）：development_cycle.v2 与 report binding、scope 索引中的 cycle_ids 不受保护。周期数比锚少的备份仍可恢复，但关闭周期不花钱、按 report fact 幂等，且只要有 claim 就受保护，没有重做花费或动作的路径。备份之后产生、恢复前已被脱敏的派生内容无从比较，这是既有口径的性质。
- 结论：verified（E16.5/E08 本轮新增动作、幂等与失效事实的恢复覆盖子范围）。固定 head `43984ab3ecc0833e435bb25ddc4b6b3f2adadbf5`（已合并，merged_sha `7120842a1f586ae394b76e146c8298f11bec52bc`）；须在 #77 之后合并。回滚点 `400f707`。

#### AG-032 派发与下一批侦察（基线 43984ab 栈顶 #76）
- AG-032（E10/E07 精度修复）：回放存储读路径遇脱敏墓碑时返回点名的 Conflict（沿用 AG-024 与 AG-027 的惯例）；提交检查比对 tombstone 的 source_kind 与依赖 kind。卡 cards/AG-032.md（SHA-256 fbcfcebb4215704416d1f2338eee5dd5ec3eb1fa0fb5cee7d850fe19407b72c4）。
- 侦察：E09 run_next 的可信性缺口（Fixture 报告被记为 Valid、从不构造 RepairableFailure、优化历史不进入请求与签名）；E14 第二增量（MetaTrial 最小程序子范围、Improver 批准登记、下一项 Improver 作业使用 I1）的可行性。

#### CTRL-AG031-R1 / PR #78：E13 终态类别有界（主控独立验收）
- 交付 `1bc3961`（基线 400f707；monitoring.rs +61/−8，新增公开纯函数 `terminal_category`：1..=128 字节原样使用，空原因为 "unspecified"，超长时在 UTF-8 字符边界截断并加上 `…#<sha256 前 16 hex>` 后缀，总长不超过 128 字节；唯一出口 persist_terminal_run 与 complete_no_contrast 都经过它；删除原先的 Invalid 拒绝；原因全文不另存。新测试 consolidation_terminal_category_v42.rs 共 10 项）。实施方先复现了旧的 Invalid 失败，再修复；9 处变异全部被测试杀死。
- 线性叠放：从 400f707 rebase 到 #76（43984ab）之上，得到 `892ed1c`，补丁逐字节相同；以 force-with-lease（锁定 1bc3961，使用 ${B} 形式）推送后转 ready。
- 主控复跑（全新 target 目录 target-ctrl-ag031，`qa/ctrl-stack-ag031-892ed1c.log*`）：fmt 0；clippy 0；全工作区 62 个测试二进制 801 通过 / 0 失败（791 + 10）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例 2/2：(1) 随机性质检查（`qa/ctrl-ag031-adversarial.log`）：4000 个原因，其中 3253 个超长，混合 1–4 字节字符及标记字符 … 与 #，全部满足有界、确定、短原因原样、截断部分为原因前缀、摘要正确；(2) 撤销后检查（`qa/ctrl-ag031-adversarial-b.log`）：长原因 Rejected 终态之后撤销 source-1，清理到 Complete，任何存储对象都不再含原因全文，只有 consolidation_run 记录保留有界类别。实施方原先"读代码得出"的结论由此变为实测事实。
- 设计观察（非阻断）：一个特意构造、恰好等于"某长原因截断加摘要"的短原因，会得到相同的类别。类别不参与身份或幂等比较，只影响显示，无安全影响。
- 结论：verified（E13 巩固终态类别有界、claim 不再因长原因悬挂）。固定 head `892ed1cebaf50ea3a7ae2a38ddd05d70313637b1`（已合并，merged_sha `84d8a7054c2428db0f821fcd478c615b09ade76b`）；须在 #76 之后合并。回滚点 `43984ab`。

#### AG-033 派发（E09 PR-A：run_next 的可信观察门与证据标签）
- 依据 E09 第二次侦察：run_next 把 Fixture 报告记为 Valid，未调用 E03 门，分数为 runner 自报；可修复故障在生产代码中零写入；历史不进 decide、请求与签名；NoChange 与 KeepIncumbent 被混记为 HardFailure。拆分为 PR-A（可信门）、PR-B（可修复故障来源与 Recover 端到端）、PR-C（历史进入请求与签名），先做 PR-A。
- 裁决：Candidate 且非 Fixture 时同 session 过 E03 门，质量取服务端重算分；Fixture 或门拒绝 → HardFailure 加证据标签（FixtureDeclared/EvidenceRejected），dispatch 仍为 Observed（付费后不能事后报错，否则会卡死）；不新增 ObservedStatus 变体、不升 DISPATCH_SCHEMA、不改 Outcome 形状；final_candidate_request 只接受 Trusted 且 Valid 的节点；授权最小修改两个既有测试（exploration_v41 的 chain、meta_inheritance_v42:1175–1217）。卡 cards/AG-033.md（SHA-256 d69d91241fc1212ca0fd72ca1323db2d09eaa953738bbdfb098194f09c4ec13f），基线 892ed1c。

#### AG-034 派发（E14.2a MetaTrial 分叉，程序范围）与 E14 侦察要点
- E14 第二次侦察要点：增量 1 的各部件在生产路径上互不相连（没有非测试调用者，world.policy 来自请求、未与获批内容绑定）；真批准在结构上不可达（streaming_evaluator 的 promotion_eligible 恒为 false，评估证据只有 ProgramFixture，release_store 的批准恒 Forbidden），因此 E14.2 只能做 ProgramFixture 范围的授权登记，不得称为生产批准或改进；meta.start 只能接成"登记与绑定"型消费者（受信的 OptimizationStepRequest 无法由 JSON 构造），且须排在 AG-032（dispatch.rs 相邻改动）之后。
- 主控裁决 D1–D5：ProgramFixture 范围的登记（命名不用 release 或 approved improvement）；Improver 作业在本增量只作登记型标注，真正由 I1 提出或筛选 I2 另立 E14.3；存储用探索信封内的新 record_kind；meta.start 后置；每流额度 = 世界级声明上限 + 共享根额度，"一流可能饿死另一流"如实记录，不拆 billing scope（拆分与 E04 相悖）。
- 既有缺陷（记为后续任务，不在 AG-034 中修）：registration_fingerprint 纳入了 run_next 会改写的 remaining_root_micros/remaining_recovery_dispatches（与代码注释不符），dispatch 之后重放 register_world_idempotent 得到 Conflict；exploration.start 的 status 重验的是"当前"纯决策，首次 dispatch 之后必为 Conflict。run_next 目前没有生产调用者，暂无生产影响。其他附带发现：evo-core lib.rs 中 MetaEvidence 的 "Equal total budgets" 注释与 §9/V035 相悖；contract.rs 中 v1 improver 字段的 consumer 为 "worker"，但没有对应模块（E02/V044）。
- AG-034 卡 cards/AG-034.md（SHA-256 3b1cd8c125a63c8afb9924825933409fd6b216c105b27e16a00d9772f5de5253），基线 892ed1c；exploration.rs 只做纯增量，以免与 AG-033 冲突。

#### CTRL-AG032-R1 / PR #79：E10/E07 回放读路径点名脱敏记录、提交检查比对 tombstone 的 source_kind（主控独立验收）
- 交付 `5880ba6`（基线 43984ab；3 文件 +1924/−12，生产代码 +110/−12）：
  - evo-storage replay.rs 新增私有 `read_envelope`：先读原始正文，若为 `rsia.redacted.v1` 则返回 `Conflict("replay <record kind> <id> was redacted because its source was revoked")`。覆盖世界、池、报告的读取，以及 register_replay_pool 与 put_replay_report 对既有记录的读取。其余解码失败保持原分类（池与报告为 Internal，世界为 Invalid）。
  - 当时 dispatch.rs 的 `ensure_dependencies_live`（AG-046/#96 后逻辑已移入 `crates/evo-engine/src/revocation_gate.rs`，dispatch.rs 保留包装）只在 tombstone 的 source_kind 等于依赖 kind 时才拒绝；tombstone 无法解析或 source_kind 不是 run/artifact 时 fail-closed；其他依赖 kind 只做脱敏正文检查（生产中的依赖 kind 只有 run 与 artifact，已核对 private_dependencies）。
  - 新测试 replay_redacted_reads_v42.rs 共 8 项；dispatch 直接调用矩阵 16 例；storage 单测 1 项。
- 线性叠放：从 43984ab rebase 到 #78（892ed1c）之上，得到 `8954b6c`，补丁逐字节相同；以 force-with-lease（锁定 5880ba6，使用 ${B} 形式）推送；PR 正文改为叠在 #78 之上，然后转 ready。
- 主控复跑（全新 target 目录 target-ctrl-ag032，`qa/ctrl-stack-ag032-8954b6c.log*`）：fmt 0；clippy 0；全工作区 63 个测试二进制 811 通过 / 0 失败（801 + 10）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例（探针源 `qa/probe-ag032/append.rs`，SHA-256 310e3ddc…；head 日志 `qa/ctrl-ag032-adversarial.log`；基线 892ed1c 对照日志 `qa/ctrl-ag032-adversarial-base892ed1c.log`）：
  - P1（通过）：撤销 source-2 后，真实清理每步只处理 1 个节点，共 12 个状态（Pending → Running ×10 → Complete）。每个状态下调用 12 个读入口：报告（admin/evaluator）、verified view（两种角色）、池、两个世界、register_replay_pool、put_replay_report、run_persisted_replay、run_and_persist_pool_replay、dispatcher status。没有任何一个返回 Ok 或 Internal：清理前为 watermark Conflict，之后依次点名世界、池、报告的脱敏。基线 892ed1c 同一探针在第 6 步返回 Invalid("sealed world is not ReplayWorldV2")，第 8 步返回 INTERNAL，即修复前失败、修复后通过，而且覆盖的是真实的中间状态，不是手写的部分脱敏。
  - P2（同 id 碰撞，误放行方向）：artifact X（严格 import_source）先被撤销，再对同 id 的 run X 调 begin_revoke，返回 **INTERNAL**；水位不变，没有写入任何东西，run 行仍在。之后依赖 run X 的 exploration.start：head 在提交闸门放行（tombstone 的 source_kind 为 artifact），作业以 forbidden 失败（作业内按 id 判 tombstone 的检查兜住），没有使用该 run；基线在提交闸门拒绝。结论：AG-032 按裁决行事，端到端仍拒绝；暴露的是 lifecycle.rs 的既有缺陷，见下文 AG-036。
  - P3（清理期间提交，超出本卡范围的信息性探针）：撤销 source-2 后，每个清理步提交一次 replay.run。在池被脱敏之前（第 0–19 步）的 20 次提交全部被接受，作业都以 conflict fail-closed，没有 Succeeded；从第 20 步起提交被拒。但清理 Complete 后，**20 个被接受作业中有 8 个的私有输入仍是明文 `rsia.management_private_input.v1`**（第 9、12、14–19 步提交的）。基线 892ed1c 结果相同（20/8），所以这是 AG-027（#74）管理清理闭包的既有缺口，不由 AG-032 引入。立为 AG-035。
- 结论：verified（E10/E07 子范围：回放读路径在撤销后的每个真实中间状态都返回点名的 Conflict，不再返回 Internal 或 Invalid；提交检查按 source_kind 精确判定，无法解析时 fail-closed）。固定 head `8954b6c86cbe1bc1a5c0b957828080b59f1d1704`（已合并，merged_sha `b262bbb2a0ea9ead68bc33fe1e58235e85bcffea`）；须在 #78 之后合并。回滚点 `892ed1c`。
- 实施方如实披露的边界（主控接受）：
  - validate_sources_live、evidence.rs、development.rs、curriculum.rs、practice.rs、import.rs、packages.rs 仍按 id 判断 tombstone，所以"提交被接受"不等于作业一定通过（P2 实测就是作业以 forbidden 兜住）。
  - seal_replay_world 与 put_replay_world_draft 对脱敏正文本来就返回 Conflict（消息较通用），未改。
- 新发现的缺陷（均为既有缺陷，另立任务）：
  - **AG-035（§11 清理闭包，高）**：begin_revoke 之后、直接依赖被脱敏之前接受的管理提交，其私有输入不一定能被清理到达；清理 Complete 之后仍有明文。漏掉的序号不单调，疑为清理枚举依赖的游标与并发插入发生竞争。待侦察后定卡。
  - **AG-036（§11 撤销登记，中）**：tombstone 只以 id 为键。同 id 的 run 与 artifact 中，后撤销的一方 begin_revoke 返回 Internal（load_status_by_source 找不到该 kind 的状态），撤销无法登记，run 行与明文永不清理；外部只看到 Internal。待侦察后定卡。
- 合并链模拟（17:55，origin/main 0c43e0f，git merge-tree 按 A–U 顺序逐个合并 21 个固定 head）：全部无冲突；最终树 `25e6f13b62d5…` 与 #79 head 的树逐字节相同，所以按顺序合并后 main 的内容就是主控已复跑（811/0）的那棵树。

#### CTRL-AG033-R1 / PR #80：E09 run_next 的可信观察门与证据标签（主控独立验收）
- 交付 `3d01a4f`（基线 892ed1c；5 文件 +3113/−82）。
  - exploration.rs：
    - Candidate 且非 Fixture 时，在付费步骤之后的同一写节点 session 内调用 E03 门（`judge_candidate_evidence`）。
    - 质量取门重算分的均值。
    - Fixture 短路为 FixtureDeclared；门的 Forbidden/Conflict/NotFound/Invalid 为 EvidenceRejected；Internal/Budget/Cancelled 走既有 Uncertain 路径（固定字面量 `development_evidence_unverified`）。
    - 节点与 dispatch fact 各加 `#[serde(default)] evidence`，默认 NotObserved。
    - 未新增 ObservedStatus 变体，DISPATCH_SCHEMA 仍为 v2。
    - final_candidate_request 只接受 Valid 且 Trusted 的节点。
    - MechanismUsageRecordV1 增加 `evidence()`。
  - optimization.rs 只新增 `development_stage_fact_ids`（镜像 `optdev-` 改写，漂移时 fail-closed）。
  - 新测试 exploration_trust_v42.rs 共 11 项，另有 6 个单测。
  - 授权修改的两个既有测试：exploration_v41 的 chain 测试改为 FixtureDeclared + Widen root2 + 末尾 Stop；meta_inheritance_v42 通过 `promote_to_trusted_valid` 预置节点，所有原断言保留。
  - 实施方做了 16 处变异，全部被测试杀死。
- 偏差裁决（主控接受）：
  - NotFound/Invalid 归入 EvidenceRejected：无回执的伪造声明在门内表现为 NotFound，畸形事实为 Invalid。
  - provenance 从同 session 已存的 DevelopmentObserved 事实读取。
  - 门无法回答时走既有 Uncertain 路径而非保持 Claimed：保持 Claimed 会重现卡死。
- 线性叠放：从 892ed1c rebase 到 #79（8954b6c）之上，得到 `04e6309`，补丁逐字节相同；以 force-with-lease（锁定 3d01a4f，使用 ${B} 形式）推送；PR 正文改为叠在 #79 之上，然后转 ready。
- 主控复跑（全新 target 目录 target-ctrl-ag033，`qa/ctrl-stack-ag033-04e6309.log*`）：fmt 0；clippy 0；全工作区 64 个测试二进制 828 通过 / 0 失败（811 + 17）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例 2/2（探针源 `qa/probe-ag033/append.rs`，SHA-256 cb1b77b8…；日志 `qa/ctrl-ag033-adversarial.log`，第一轮因探针辅助函数要求恰有 1 个 observed fact 而中断，日志留作 `.run1`）：
  - C1（跨世界回执重放）：world-a 用诚实的 ContrastRunner 得到 Trusted。
    - world-b 原样回放 world-a 的报告：步骤自身的绑定检查拒绝（development report binding mismatch），不写 DevelopmentObserved；节点为 HardFailure/NotObserved；dispatch 已消费；final_candidate_request 拒绝；重连得到同一终态。
    - world-c 把报告身份改写为本请求、回执仍是 world-a 的：门拒绝（直接问门得到 Conflict "execution receipt does not bind the registered control and request"）；节点为 HardFailure/EvidenceRejected，原因为固定字面量；dispatch 已消费，不卡死；final_candidate_request 拒绝。
  - C2（付费之后、门之前撤销来源）：runner 诚实执行后撤销 run-failure。run_next 走既有的"派发后来源闭包变化"路径：节点为 usage_uncertain/NotObserved，dispatch 为 uncertain 终态，没有 Trusted。重连、新一步、final_candidate_request 都以 watermark Conflict 拒绝；新一步的 runner 与模型调用数不变（零新增花费）。
- 主控核实：回执签发函数（issue_execution_receipt、issue_grader_receipt、settle_registered_execution_call、reserve_budget_call_with_sources、begin_budget_dispatch）在 evo-http、evo-mcp、apps 中均无引用；生产中唯一调用方是 RegisteredDevelopmentRunner（development.rs:1959/1967/2103）。
- 设计观察（非阻断，记入 E03 后续）：E03 门验证的是回执链（预算行、结算、执行与评分回执、重算分、来源存活），不重算已登记纯函数的输出。测试中的 ContrastRunner 以 Worker/Evaluator 身份经公开签发函数，为已登记纯函数不可能产生的输出（两侧不同）签发 RegisteredPureFunction 回执，门判为 Trusted。所以 Trusted 的含义是"进程内执行方按预算结算签发的回执链自洽"，不是"已登记函数可复算地产生了该输出"。接入真实执行器时，应把执行方身份绑定到登记的执行器；对 RegisteredPureFunction 来源，可在门内复算输出摘要。
- 结论：verified（E09 子范围：run_next 只把经 E03 门验证的开发证据记为 Valid 节点；Fixture、伪造、重放或分数不符的证据被标记为 HardFailure 终态，不能驱动后继，也不能成为最终候选；付费后的拒绝不会卡死 dispatch）。固定 head `04e63092f2995c3668e14640c985012fd6a6a2b2`（已合并，merged_sha `9e3e350ed5fbef2975117f12a7f8d3ca3b9cdcbf`）；须在 #79 之后合并。回滚点 `8954b6c`。
- 已知边界（如实披露，主控接受）：
  - 可信 Valid 对生产 runner 仍端到端不可达（RegisteredDevelopmentRunner 两侧相同）。
  - 旧 Valid 节点读作 NotObserved：仍可被 Deepen，但永远不能成为最终候选。目前没有生产调用者产生节点，所以没有存量；建议在 E09 PR-C 中让 Deepen 也要求 Trusted。
  - 门返回 Internal/Budget/Cancelled 的路径只有单元测试。
  - 线格式只前进不后退。
  - run_next 付费之后既有的 Err 路径（prefix changed，以及 skill_snapshot_digest/fingerprint 上的 `?`）仍可能卡死 claim，记入 E09 PR-B。
  - NoChange 与 KeepIncumbent 仍记为 HardFailure。

#### CTRL-AG034-R0 / PR #81：E14.2a MetaTrial 分叉第一轮验收（退回 R1）
- 交付 `530a03d`（基线 892ed1c；4 文件 +4573/−6）。
  - meta.rs：
    - MetaTrialV1 存在探索信封内，record_kind 为 meta_trial_v1；读后写，同 id 同内容幂等，异内容 Conflict。
    - 可信 `MetaTrialCoordinator::fork`：请求只含 trial_id、S0 模板、new_content、billing_scope，带 deny_unknown_fields；模板策略必须是 I0；new_content 过 ImproverContentV2::parse 与 MetaCandidate::validate；两世界只差 id 与 policy。
    - persist_trial 在同一 session 内先 validate_stored_sources（水位、tombstone、可信来源），再写记录和依赖边（试验 → 每个来源 run、试验 → 两个世界）。
    - 读侧 `verified_meta_trial`：视图不可从 JSON 构造；派生状态为 candidate_only 或 usage_observed。
    - `stream_request_id` 为纯函数。
  - exploration.rs 只做纯增量：4 处 pub(crate) 加 registered_world。
  - lifecycle.rs 追加一个 redact 分支。
  - 新测试 meta_trial_v42.rs 共 25 项，另有 8 个单测；实施方做了 6 类变异，全部被杀死。
- 偏差（主控接受）：
  - fork 不在同一事务内：单连接存储无法嵌套 register_world_idempotent 的事务；以"重放收敛"保证，并有删除记录模拟中间状态的测试。
  - 比卡片更严：billing_scope 必须是本 namespace 已授权的根预算；模板必须是全新世界；候选不能等于 I0；trial id 不超过 64 字节。
- 线性叠放：从 892ed1c rebase 到 #80（04e6309）之上，得到 `df4a23d`。改动行逐行相同，只有 exploration.rs 的 hunk 偏移随 AG-033 的新增下移。以 force-with-lease（锁定 530a03d）推送，PR 正文改为叠在 #80 之上。叠放后先跑定向测试：meta_trial_v42 25、meta_inheritance_v42 15、exploration_trust_v42 11，全部通过，说明与 AG-033 的语义兼容。
- 主控复跑（全新 target 目录 target-ctrl-ag034，`qa/ctrl-stack-ag034-df4a23d.log*`）：fmt 0；clippy 0；全工作区 65 个测试二进制 861 通过 / 0 失败（828 + 33）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控核读：
  - fork 前置检查对不可信或缺失的来源闭包在写记录之前就拒绝（validate_stored_sources → load_stored_source 要求 `rsia.optimization.source.v1` 且 authority 校验通过），不会留下孤儿记录。
  - 实施方的撤销测试已覆盖：撤销后 fork 新 trial 不留记录；清理后读为点名 Conflict；重新 fork 被拒。
- 主控反例（探针源 `qa/probe-ag034/append.rs`，SHA-256 前缀见本节命令输出；日志 `qa/ctrl-ag034-adversarial.log`）：
  - T2（通过，恢复覆盖）：fork 之后，read_control_plane_facts 的 protected_objects 从 0 增至 3，包含试验记录与两个流世界（探索信封 schema 已在 PROTECTED_SCHEMA_VERSIONS 与 restore_backup.py:254 中，按 schema 选取）。
  - T1（**发现**，V035 实耗如实记录）：namespace 另授权 scope-2；old 流在 scope-1 下花 2 次 14 micros；new 流用 scope-2 的 broker 真实花 2 次 26 micros。verified_meta_trial 却报 new 流 calls=0、actual=0，status=UsageObserved：静默少报。
- 裁决 R1（真源 §9 L1000–1023、V033/V035）：两流比较的前提是同一预算上限；任一流在声明 scope 以外有调用即破坏前提，必须失败闭合。
  - evo-storage budget.rs 新增一个只读、按 namespace 隔离的查询（dispatch group 出现过的 billing scope 列表），不加迁移；
  - verified_meta_trial 发现声明 scope 以外有调用时返回固定格式的 Conflict；
  - 白名单只为该查询增加 budget.rs；
  - PR 正文的实耗表述改准确。
  - 已退回原实施方（保留上下文），在 df4a23d 之上追加提交。

#### AG-035 / AG-036 派发（E08/§11 撤销闭包的两个既有缺陷；基线 04e6309 = #80）
- 侦察（只读，树 8954b6c）结论：
  - AG-035 根因在 E08 清理本身。闭包惰性展开，依赖按 (src_kind, src_id) 以严格 `>` 的前向键集游标翻页；节点末页之后置 expanded，永不复扫；pending==0 即置 Complete，没有完整性复核。用探针的 20 个私有输入 id 离线复算，与漏网集合 {9, 12, 14–19} 精确吻合。
  - 侦察同时列出 begin_revoke 之后仍能挂新依赖者的写入点：管理提交、store_source_selection、DispatchObserved 迟到事实、Uncertain 探索节点、预算预留（budget-ref → run）。
  - AG-036：tombstone 主键只含 id，begin_revoke 命中他 kind 的 tombstone 时 `ok_or(Internal)`（lifecycle.rs:185）；run（Host 自选 id）与 import_source（派生 id）在两条创建路径上都不互查。
- AG-035 裁决：cleanup_step 在 pending==0 时于同一事务内做不动点复扫（依赖边；并核实或补上迟到预算调用的请求正文），复扫为空才置 Complete。
  - 终止性由 frontier 单调、节点有限保证；迟到的未分类节点使作业 Failed（失败闭合）。
  - 写入方闸门与 Complete 之后的迟到写入另立后续。
  - 卡 cards/AG-035.md（SHA-256 567344f66f45b2bdd527326dfd30bb7e356b3b5567d7eb08171f10d0400856a3）。
- AG-036 裁决：
  - begin_revoke 遇他 kind 的 tombstone 时：对象不存在为 NotFound，存在为点名 Conflict，什么都不写（绝不落到 upsert 覆盖第一枚 tombstone）；
  - store_trace_authority 与 import_source 创建互查同 id 的撤销源，冲突时 Conflict 且不写入；
  - tombstone 键带 kind 的长期方案另立（信任锚格式变更）。
  - 卡 cards/AG-036.md（SHA-256 a748fb5e84f68356401311ed99f61c85a84aecc33c8f662cdef55db05a88ed02）。
- 两卡并行：AG-035 只改 cleanup_step 完成判定一带，AG-036 只改 begin_revoke，与 AG-034 的分类分支互不相邻；新测试各放新文件。
- 后续候选（未派）：
  - AG-035b 写入方闸门：提交时上游闭包检查（会翻转 dispatch_management.rs:1541–1556 与 management_cleanup_v42.rs:3153–3208 两处钉住"提交接受、作业失败"的测试）；预算预留的来源检查；store_source_selection 的闸门；replay_experiment start 的 TOCTOU 及其信封未分类；Complete 之后的迟到写入。
  - E09 PR-B 与 PR-C；E14.2b 与 E14.2c；registration_fingerprint 既有缺陷。

#### CTRL-AG034-R1 / PR #81：返修验收
- R1 `c5c04ba`：在 df4a23d 之上快进追加，未 force。3 文件 +546/−28。
  - budget.rs 新增只读的 `Session::budget_call_scopes_for_group`：按 namespace 隔离，DISTINCT 加 ORDER BY，无迁移。
  - meta.rs 的 `stream_spend` 在同一账本快照内先查 scope 列表，有声明 scope 以外的就返回固定格式的 Conflict（只点名流）。
  - 测试由 25 项增至 29 项。实施方的变异检查：关掉检查即回到假账；去掉 namespace 过滤或 DISTINCT 会被存储层测试抓到；去掉 ORDER BY 测试抓不到（SQLite 恰按索引序返回），已在 PR 中如实写明。
- 主控复跑（target-ctrl-ag034，`qa/ctrl-stack-ag034-c5c04ba.log*`）：fmt 0；clippy 0；全工作区 65 个测试二进制 865 通过 / 0 失败（861 + 4）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例第二轮（`qa/ctrl-ag034-adversarial-r1.log`；探针 `qa/probe-ag034/append.rs` 只把 authorize_second_scope 调用补成 R1 的新签名，原版留作 append-r0.rs）：
  - T1：修复前 new 流在 scope-2 真实花费 26 micros，视图却报 0 且为 UsageObserved；修复后视图返回 Conflict("meta trial new stream: its dispatch group was billed outside the trial's billing scope")。
  - T2：恢复覆盖仍成立（protected_objects 0 → 3，含试验记录与两个流世界）。
- 结论：verified（E14.2a 程序子范围）。
  - 同一 S0 的两条探索流由可信 fork 构造，只差 id 与 policy；声明上限相同。
  - 各流实耗按试验声明的 billing scope 如实读出；任一流在其他 scope 下有调用时试验不可验证。
  - 撤销后封锁并被清理脱敏；试验记录受恢复保护。
  - 固定 head `c5c04bad5c349f00c815bb02c9a3eccc439b0da7`（已合并，merged_sha `d9b3f6f5234ad14da18956a78c804e4c6818fa10`）；须在 #80 之后合并。回滚点 `04e6309`。
- 不可声明：I1 更优或任何元收益；逐步同证据；按流的硬额度（共享根额度，一流可饿死另一流，已有测试钉住）。
- 已知边界：
  - 在没有任何调用之前，billing_scope 只是试验自己的声明；
  - fork 重放不读账本；
  - meta.start 接线（E14.2c）尚未做，流的 broker scope 目前由调用方保证。
- 合并链模拟（18:55，origin/main 0c43e0f，A–W 共 23 个固定 head，git merge-tree 逐个合并）：全部无冲突；最终树 `fe7dfbbf8b65…` 与 #81 head 的树逐字节相同，主控已在该树上复跑（865/0）。合并脚本 `merge-all.sh` 由 user-commands.md 自动生成：23 条命令按 A–W 顺序排列，每条都钉住 head，遇第一处失败即停；生成时已与线上 23 个 PR 的 head 逐一核对，全部一致且都不是 draft。

#### 合并进展（19:05，由用户在终端运行 merge-all.sh）
- A #60 `dc036d5`、B #59 `69a3b3e`、C #61 `b220971`、D #62 `9c3ec97`、E #63 `3e2fc06`、F #64 `f8fa1a1` 已合并（main = f8fa1a1）。
  - #60 由主控在用户加入 `Bash(gh pr merge:*)` 允许规则后执行；随后主控查询其状态被分类器以 Merge Without Review 拦截，主控停止，其余由用户运行脚本完成。
- G #65 无法合并（GitHub：merge commit cannot be cleanly created），原因：
  - #65 的 head f752e1d 是把 #60 合进来的合并提交，与新 main 有两个合并基（71c0fa8、2e5c668）。
  - git ort（虚拟基）干净合并，结果树 `4908527b` 与 f752e1d 的树逐字节相同；只用单一合并基 71c0fa8 时，docs/implementation-ledger.md 冲突。GitHub 判为不可合并，与后者一致。
- 修复方案：在 #65 分支上加一个合并提交，父为 f752e1d 与 main，树保持 f752e1d 不变。主控试图创建它，被分类器以 Auto-Mode Bypass 拦截。已把现成命令交给用户：命令只创建该提交、快进推送并钉住新 head 合并 #65，然后重跑脚本继续 H–W。
- 推理：H–W 都线性叠在 f752e1d 之上，各自只有一个合并基，不会再遇到这一问题。最终模拟同样被拦，主控未实际跑。

#### CTRL-AG035-R1 / PR #83：E08/§11 撤销清理只在闭包不动点处完成（主控独立验收）
- 交付 `44fe003`（基线 04e6309；lifecycle.rs +287/−1，两个新测试文件共 11 项）。
  - cleanup_step 在 pending==0 时，于同一事务内先调用 `closure_is_open` 再置 Complete：
    - (a) `queue_late_dependents`：找 dst 为本作业已 expanded 节点、src 不在本作业 frontier 的依赖行，每步至多入队 edge_page_limit 个；
    - (b) `redact_late_budget_calls`：用与 `__budget_scan` 相同的判据，复核尚未脱敏的预算请求正文。
  - 实施方查明 (a) 不覆盖迟到的预算调用：budget-ref 属保留类，不带 source_ids 的调用没有边。已按同一原则补上 (b)。
  - 迟到的未分类节点或未知请求 schema 会让作业 Failed（失败闭合）。
  - 新增两种审计事件 late_dependents_queued 与 late_budget_calls_redacted。
  - 无迁移，未改提交闸门与各写入方。
- 修复前失败、修复后通过：
  - 存储层 7 项、引擎 P3 1 项在基线失败；基线上漏网集合正是 {9, 12, 14–19}。
  - 无迟到边的步数常量取自基线实测，修复后不变。
  - 3 处变异被测试杀死。
- 线性叠放：从 04e6309 rebase 到 #81（c5c04ba）之上，得到 `fee5897`。改动行逐行相同，只有偏移变化。以 force-with-lease（锁定 44fe003）推送；PR 正文改为叠在 #81 之上，然后转 ready。
- 主控复跑（全新 target 目录 target-ctrl-ag035，`qa/ctrl-stack-ag035-fee5897.log*`）：fmt 0；clippy 0；全工作区 67 个测试二进制 876 通过 / 0 失败（865 + 11）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 恢复兼容核查：revoke_cleanup_events.event_kind 没有 CHECK 约束；restore_backup.py 只插入自己的 logical_block_restored 事件，不校验已有事件种类。新事件种类不影响恢复。
- 主控反例 2/2（探针源 `qa/probe-ag035/append.rs`；日志 `qa/ctrl-ag035-adversarial.log`）：
  - Q1（主控 P3 加断言）：清理期间每步提交一次 replay.run，共 20 次被接受；63 步后 Complete，明文私有输入 0 个（892ed1c 与 8954b6c 上为 8/20）。
  - Q2（两个闭包重叠的清理作业）：先撤销 source-2；第 3 步再撤销 source-1（两者都是 world-1 的来源）。两作业交替单节点步进，每步之前都提交一次 replay.run，共 141 次被接受。724 步后两作业都是 Complete（无 last_error，未落入 unknown scope），明文 0 个。
  - 观察：持续提交会推迟池的展开，从而推迟 Complete（只延迟、不漏删）。这再次说明提交侧闸门（AG-035b）有价值。
- 结论：verified（E08/§11 子范围：Complete 时本作业依赖边闭包内没有未处理的依赖者，也没有符合判据却未脱敏的预算请求；P3 的明文残留已消除；崩溃重启后成立；两个重叠作业各自收敛）。固定 head `fee5897ff45070fc198c0f46daac9e364ce7e38d`（已合并，merged_sha `51db9b6c9a251def4e6566c96641ed8b63d43a85`）；须在 #81 之后合并。回滚点 `c5c04ba`。
- 不可声明：撤销后在提交时即拒绝；Complete 之后的迟到写入；超出 MVP 规模的闭包；写入方以 upsert 覆盖已脱敏节点。

#### CTRL-AG036-R1 / PR #82：E08/§11 同 id 的第二个撤销源点名拒绝、run 与 import_source 不再碰撞（主控独立验收）
- 交付 `4b351da`（基线 04e6309；5 文件 +1191/−0）。
  - lifecycle.rs 的 begin_revoke（:177–200）：遇他 kind 的 tombstone 时，对象不存在返回 NotFound，存在返回点名的 Conflict，什么都不写（绝不落到 upsert）。
  - evidence.rs 的 store_trace_authority：在创建分支中，若同 id 有 import_source artifact，返回 Conflict。
  - import.rs：在 register 写第一行之前，对每个 prepared source id 做 `ensure_import_source_id_is_free`（tombstone 为 Forbidden，同 id run 为 Conflict）。
  - 新测试：存储 8 项、引擎 9 项。"什么都没写"由第二只读连接逐表比对。
- 修复前失败、修复后通过：存储 4/8、引擎 5/9 在基线失败。3 处变异被杀死，其中包括"落到写入路径"这一陷阱。
- 偏差（主控接受）：
  - import 创建遇 tombstone 用 Forbidden，并先于 run 检查，与各读闸口径一致；
  - 解码得出但 kind 未知的 tombstone 不回显存储串；
  - store_trace_authority 只在创建分支检查，对已有同 id 对的同一 run 重存行为不变；
  - 不可解码的 tombstone 仍为 Internal（Session::get 在比较 kind 之前就失败），有测试钉住。
- 主控核读：prepared import source 在 register 时就以 `rsia.e16.import_source.v1` 写入（import.rs:1729、1790），所以 register 与 finalize 之间不存在 run 抢占同 id 的窗口。
- 线性叠放：从 04e6309 rebase 到 #83（fee5897）之上，得到 `f15a42f`，改动行逐行相同。以 force-with-lease（锁定 4b351da）推送；PR 正文改为叠在 #83 之上，然后转 ready。
- 主控复跑（全新 target 目录 target-ctrl-ag036，`qa/ctrl-stack-ag036-f15a42f.log*`）：fmt 0；clippy 0；全工作区 69 个测试二进制 893 通过 / 0 失败（876 + 17）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例（探针源 `qa/probe-ag036/append.rs`；head 日志 `qa/ctrl-ag036-adversarial.log`；基线 fee5897 对照日志 `qa/ctrl-ag036-adversarial-basefee5897.log`）：
  - R1（P2 复验）：artifact X 先被撤销后，对同 id 的 run X 调 begin_revoke，返回 Conflict("cannot revoke run run-failure: the id is already revoked as artifact (a revocation tombstone is keyed by id)")。水位、第一枚 tombstone、run 行都不变；重复调用仍为同一 Conflict。基线 fee5897 上为 INTERNAL（修复前失败）。
  - R2（与在途清理的交互）：被拒之后，第一次撤销（artifact）的清理照常到达 Complete，last_error 为空、pending 为 0。artifact 被脱敏，run 行原样保留；tombstone 的 source_kind 仍为 artifact。Complete 之后再次撤销 run，仍是同一点名 Conflict。
- 结论：verified（E08/§11 子范围：同 id 的第二个撤销源得到点名的 Conflict 而不是 Internal，且不改动任何既有撤销记录；两条生产创建路径不再产生 run 与 import_source 的同 id 碰撞）。固定 head `f15a42fe94426a49e8967869f8087a99837fd4a5`（已合并，merged_sha `8d959dee844fb16f0763b0684ba3adf48ad75f1f`）；须在 #83 之后合并。回滚点 `fee5897`。
- 不可声明：存量碰撞对象可以撤销（需要 tombstone 键带 kind 的后续方案，涉及信任锚格式变更与恢复双读）；经 raw put 的碰撞被阻止。
- merge-all.sh 已重新生成并覆盖 A–Y（25 条）。生成时核对：A–F 已合并（不在 open 列表），G–Y 全部 open 且 head 与钉住值一致。#65 仍为 f752e1d，等待用户按方案 (a) 或 (b) 处理。

#### 下一批侦察（19:30，只读，树 f15a42f）
- E09 PR-B：run_next 付费后的 Err 路径会卡死 claim；RepairableFailure 与 Recover 端到端；NoChange 与 KeepIncumbent 的终态；registration_fingerprint 的可变字段缺陷；exploration.start status 在首次 dispatch 后必为 Conflict；Deepen 要求 Trusted。
- AG-035b：写入方闸门，包括提交时上游闭包检查、预算预留的来源检查、store_source_selection、replay_experiment 的 TOCTOU 及其信封未分类、stage_package、Complete 之后的迟到写入。

#### 侦察结论与派发（19:45–20:05，基线 f15a42f = 栈顶 #82）
- AG-035b 侦察（写入方闸门）要点：
  - 经济回放信封（rsia.replay_economic_artifact_envelope.v1）在清理器里未分类。撤销链 run → world → report → experiment → job 可达，清理会以 blocked_unknown_scope 失败，而不是 Complete。
  - replay_experiment 的 start 存在 TOCTOU：会话 2 写 job 前不再重验来源。
  - 预算预留无来源闸。begin_budget_dispatch 无闸：预留之后才撤销的调用仍会派发，属于出站泄露。
  - 迟到响应在请求被脱敏之后仍以明文结算。
  - 迟到的 DispatchObserved 在 Complete 之后留下明文。
  - 提交时缺上游闭包检查（翻转测试已列出）；grant 与 stage_package 无闸（无生产调用方）。
- E09 PR-B 侦察要点：
  - 付费后有三类确定性 Err 会永久卡死 claim：前缀漂移、digest 的 `?`、world id 过长使节点 id 超过 128 字节；
  - RepairableFailure 需要类型化来源；
  - NoChange 与 KeepIncumbent 都记为 HardFailure；
  - 两个新缺陷：X1，已有后继的父仍被重复 Deepen，命中同一 dispatch id，世界卡死；X2，子节点的最佳祖先口径与 PrefixViewV2::validate 不一致，父节点退步后世界变砖；
  - registration_fingerprint 包含可变计数器；exploration.start 的 status 在首次 dispatch 之后必为 Conflict。
- 磁盘：已合并的 AG-012 至 AG-017 各 worktree 均干净，其 head 都已在 origin/main 中；已移除这 6 个 worktree 与 target-ag-014 至 017，释放约 8 GB，可用空间 30 GB。
- 派发（均基于 f15a42f，彼此不相邻，可以并行）：
  - **AG-037**（卡 SHA-256 16e7e434…）：经济回放记录逐个精确列入 preserve 分类；start 在写 job 前重验来源。
  - **AG-038**（卡 SHA-256 ec2d4b5e…）：预留闸与派发闸，按 kind 精确判定（run 类 tombstone 或不可解码时返回 Forbidden；派发被拒则释放预留、transport 调用 0 次）；结算类路径一律不拦；授权最小修改 AG-035 测试中的迟到预算调用构造。
  - **AG-039**（卡 SHA-256 26e26d96…，B2）：Deepen 要求 Trusted；X1 已展开的父不再列为合法动作；X2 在 engine 侧按回放的 max 规则对齐校验口径（不改 evo-core 校验器）。每个缺陷先复现再修。
  - **AG-040**（卡 SHA-256 9dac483b…，B3）：
    - 指纹去掉两个可变计数器，计数器改为按登记时的值精确比对（由存储的当前值加已扣量还原，不足以还原则停下回报）；
    - 新增 verified_world_registration_view；status 在世界已启动后改为比对首决策；run_exploration_start 重跑时取请求中的世界。
- 排队中：
  - AG-041（B1，付费后收敛，与 AG-039 在 run_next 中区域重叠，等 AG-039 完成后再派）；
  - AG-042（B4，RepairableFailure 来源与 Recover 端到端，依赖 B2）；
  - AG-043（迟到输出：结算时标为不可用并脱敏，以及 DispatchObserved 写入时脱敏）；
  - AG-044（提交时上游闭包检查）；
  - AG-045（grant 与 stage_package 闸门）；
  - tombstone 键带 kind（信任锚格式变更）。

#### CTRL-AG038-R1 / PR #85：E08/§11 来源已撤销的预算调用不预留、不派发，结算照常入账（主控独立验收）
- 交付 `e06bfdd`（基线 f15a42f，即栈顶，无需叠放）：budget.rs +110；新增存储测试 13 项、引擎测试 7 项；按授权对 cleanup_fixpoint.rs 做了最小修改（+8/−7）。
  - 预留闸（budget.rs:593–598）：在幂等提前返回之后、读根预算与首次写入之前，于同一事务内检查。来源以 run 撤销、tombstone 不可解码或 kind 未知时返回 Forbidden，什么都不写；同 id 的 artifact tombstone 不拒。
  - 派发闸（budget.rs:1732–1740，同时覆盖 Store 与 Session 两条路径）：从调用的 budget-ref 读出已登记的闭包，命中已撤销来源时返回既有的 Cancelled。broker 随之走 release_undispatched，transport 调用 0 次，额度归还。
  - 结算类路径一律不拦（finalize、settle、mark_uncertain、release、cancel、reconcile、close、refence），有测试钉住，"给 finalize 也加闸"的变异会被抓住。
- 修复前失败、修复后通过：存储测试 6/13、引擎测试 5/7 在基线上失败；7 处变异全部被杀死。
- 主控复跑（全新 target 目录 target-ctrl-ag038，`qa/ctrl-stack-ag038-e06bfdd.log*`）：fmt 0；clippy 0；全工作区 71 个测试二进制 913 通过 / 0 失败（893 + 20）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控核读：两个闸门的 tombstone 查询都按调用的 namespace 隔离（`WHERE namespace=? AND kind='tombstone' AND id=?`）。
- 主控反例 2/2（探针源 `qa/probe-ag038/append.rs`；日志 `qa/ctrl-ag038-adversarial.log`）：
  - G1（租户隔离）：m 撤销了自己的 run-r。n 有同 id 的存活 run，在 n 中对它预留得到 Reserved，派发得到 new_dispatch=true；m 中预留得到 Forbidden。
  - G2（恢复路径不可绕过）：撤销之前预留，撤销之后首次派发得到 Cancelled，调用留在 Reserved，占用 20 micros。租约过期后 refence 成功（epoch 变为 2），用新租约派发仍得到 Cancelled；release 之后额度归零。
- 结论：verified（E08/§11 子范围：来源撤销之后，对其不再产生新的预算预留，也不再向提供方派发预留在其上的调用，额度归还；已派发调用的真实费用照常对账）。固定 head `e06bfdd513efbbf87952bd8c822e2de12e803df7`（已合并，merged_sha `f4b1ef66340412f5c0edf69a48549cd1386324cf`）；须在 #82 之后合并。回滚点 `f15a42f`。
- 已知边界（如实披露，主控接受）：
  - 迟到响应在结算时不可用与脱敏的问题未做（AG-043）。
  - 无 sources 的预留不拦，由 AG-035 的复扫兜底。
  - 派发被拒时，broker 以通用理由 root_stopped_before_dispatch 释放，未写明真因。这是既有的笼统标记，记为后续：需要携带原因的决策。
  - development runner 在派发被拒时不释放其 1 micro 的预留，与 root stop 和 group stop 时的既有做法一致；租约过期后可由 refence 加 release 收回（G2 已证）。
  - tombstone 仍只以 id 为键。

#### CTRL-AG039-R1 / PR #84：E09 合法动作与前缀卫生（主控独立验收）
- 交付 `11b7019`（基线 f15a42f）。exploration.rs +52/−5；新测试 exploration_legal_actions_v42.rs 共 11 项（约 1000 行夹具复制自 exploration_trust_v42.rs，原文件未改）。
  - Deepen 要求 evidence 为 Trusted（exploration.rs:1803），与 final_candidate_request 同口径。
  - X1：节点已有子节点（search_parent_seq）后，不再提供它的 Deepen（:1780–1804）。
  - X2：子节点的 best 取 max(父 quality, 父 best, 基线)，Valid 子节点再与自身 quality 取大，与回放（replay.rs:665–674）及 PrefixViewV2::validate 一致（:1293–1309）。gains 不变。
  - 三个缺陷都先在 f15a42f 上复现：8/11 失败，其余 3 项钉住"不变"的行为。消融矩阵显示每处修复各自被对应测试抓住。
- 偏差（主控接受）：X1 的判据用"已有子节点"，而不是卡中举例的"dispatch_ids 中已有该 action_seq"。claim 先于节点写入，后一判据会让 claim 中的 Deepen 从合法集消失，既不能 resume，付费后重核 legal 摘要也会失配。实施方实测该变体，7/11 失败。
- 线性叠放：从 f15a42f rebase 到 #85（e06bfdd）之上，得到 `d4837f5`，补丁逐字节相同；以 force-with-lease（锁定 11b7019）推送；PR 正文改为叠在 #85 之上，然后转 ready。
- 主控复跑（全新 target 目录 target-ctrl-ag039，`qa/ctrl-stack-ag039-d4837f5.log*`）：fmt 0；clippy 0；全工作区 72 个测试二进制 924 通过 / 0 失败（913 + 11）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例 2/2（探针源 `qa/probe-ag039/append.rs`；head 日志 `qa/ctrl-ag039-adversarial.log` 与 `-m.log`，第一轮因探针解析 seq 字段失误而中断，日志留作 `-m.run1.log`；基线对照日志 `qa/ctrl-ag039-adversarial-basee06bfdd.log`）：
  - L（随机化混合结局）：5 种结局脚本（改进、持平、退步交替，含"先强后连续退步"）× 4 种世界形状与基线（1/2/3/5 个根，基线 0.1/0.3/0.5/0.9），证据全部经真实 E03 回执为 Trusted。20 个世界全部走到 Stop，没有任何动作被派发两次，结束后 decide_next、status 视图与 evo-core 前缀校验都通过。基线 e06bfdd 上第一个世界在第 4 轮即因 dispatch idempotency conflict 卡死（X1）。
  - M（链中伪造标签）：两轮 Trusted 之后，把第 2 个节点的标签伪造为 fixture_declared（状态仍为 Valid）。deepen-2 立即离开合法集；此后 4 轮派发 [2, 1000006, 1000008, 1000010]，从未派发 1000004；世界走到 Stop，结束后各项读取正常。
- 结论：verified（E09 子范围：后继选择只建立在可信观察之上；已展开的父不再重复 Deepen，世界能走到 Stop；父节点退步后前缀校验不会变砖）。固定 head `d4837f5f53c4ecae67f8e528b3e4e9f06758ef0a`（已合并，merged_sha `42f89bd602db0c2783de38d87d91e80cd65f220b`）；须在 #85 之后合并。回滚点 `e06bfdd`。
- 已知边界（如实披露，主控接受）：
  - 只向前修复：已按旧口径写入的节点不回填，已变砖的世界不会被修好。
  - 旧构建开出的 claim 在本构建下 resume，可能在付费后重核时 Conflict，记入 AG-041。
  - run_next 对 repair_failures_dispatched 的计数口径与回放不同，记入 AG-042。
  - 付费后 Err 路径收敛、Recover 端到端、可信 Valid 对生产 runner 可达：均未声明。

#### AG-040 第一轮（#87）与新一批派发（21:10）
- AG-040 交付 `9094ba6`（基线 f15a42f）：exploration.rs +203/−13，dispatch.rs +58/−20，新测试 18 项。
  - 指纹去掉两个可变计数器；
  - registered_budget 由完成的 dispatch fact 精确还原登记时计数器，对不上时 Conflict；
  - 新增 verified_world_registration_view；status 在世界已启动后比对首决策；run_exploration_start 重跑时取请求中的世界。
  - 修复前失败、修复后通过：基线上 16/18 失败；7 处变异被测试抓住。
- 主控裁决：
  - 已启动世界的 status 不复核存储世界的不可变字段与登记时计数器，不接受为边界，退回 R1：复用登记处的两项检查。
  - "预填调度状态"的缺口（ensure_new_world_shape 不要求 decision_round=0、无 current_branch、无 waits）并入 AG-041。
  - meta.rs:649–653 的过期文字与台账第 93 行由主控在台账 PR 中处理。
- 磁盘：仅剩 14 GB 时，删除已验收任务的构建缓存（target-ag-018 至 036、038、039，target-ctrl-ag021 至 036）。worktree 与全部验收日志保留，可用空间回到 91 GB。
- AG-037（#86）已线性叠放到 #84（d4837f5）之上，得到 `0c1bacd`，补丁逐字节相同；已推送，PR 正文已改。主控复跑进行中。
- 派发（均以 0c1bacd 为基线）：
  - **AG-043**（卡 SHA-256 ae10a724…）：结算时检查来源闭包。命中撤销来源时，费用照常入账，block_reason 记为 source_revoked、response_usable=0，transport 正文脱敏。
  - **AG-044**（卡 SHA-256 df941032…）：提交时做上游闭包两阶段判定。阶段 1 保持 AG-032 的直接判定；阶段 2 用新 Session::upstream_closure（递归 CTE，上限 10_000，超限返回 Conflict），按 kind 精确判定。授权最小修改三处钉住旧行为的测试。

#### CTRL-AG037-R1 / PR #86：E08/E10 经济回放记录纳入撤销清理分类、start 写作业前重验来源（主控独立验收）
- 交付 `2156225`（基线 f15a42f）：lifecycle.rs +25（一个 preserve 条目，精确列出 4 个 record_kind）；replay_experiment.rs +41/−1（抽出 `write_started_job`，会话 2 在写入前于同一会话内调用 verify_experiment_sources）；新测试 9 项。
- 修复前失败、修复后通过：
  - 基线上的整链撤销清理以 Failed 结束，原因为 blocked_unknown_scope:…:replay_economic_experiment_v1；
  - 两个窗口测试在基线语义下写入了 job；
  - 用临时 Notify 钩子做的确定性窗口：基线 start 返回 Ok 并留下 job 与边，本 PR 返回 Conflict；
  - 7 处变异全部被抓住。
- 偏差裁决（主控接受）：
  - job.cancel_reason（Evaluator 输入）与 Admin receipt reason（Admin 输入）为操作员文本，各 ≤512 字节，不是来源派生，保留。这与 tombstone 的 reason 同理。
  - `write_started_job` 为 `#[doc(hidden)] pub`，因为集成测试无法调用 pub(crate)；它在进程内使用，并复核角色、actor、实验与来源。后续可收窄为 cfg(test) 单测加 pub(crate)。
  - 撤销之后对已启动 key 的重放改为 Conflict，与会话 1 的口径一致。
- 线性叠放：从 f15a42f rebase 到 #84（d4837f5）之上，得到 `0c1bacd`，补丁逐字节相同。以 force-with-lease（锁定 2156225）推送，PR 正文改为叠在 #84 之上，随后转 ready。
- 主控复跑（全新 target 目录 target-ctrl-ag037，`qa/ctrl-stack-ag037-0c1bacd.log*`）：fmt 0；clippy 0；全工作区 73 个测试二进制 933 通过 / 0 失败（924 + 9）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例 2/2（探针源 `qa/probe-ag037/append.rs`；日志 `qa/ctrl-ag037-adversarial.log`）：
  - E1（另一条路径）：撤销 Train 世界的 run，经池与报告到达实验。limit 为 1 与 8 时都是 Complete，last_error 为空，processed 为 8；4 类经济记录逐字节不变；池、报告与 world-train 被脱敏，world-select 未动。
  - E2（清理运行期间的经济写入）：以 limit 1 步进，在 11 个状态下各尝试一次新的 start 与重复 register。start 全部返回 Conflict("economic source watermark changed")。register 依清理进度依次返回：水位 Conflict 6 次，点名 world 脱敏 1 次，点名 pool 脱敏 1 次，点名 report 脱敏 3 次。没有任何一次返回 Ok 或 Internal。清理 Complete，经济记录没有任何新增或变化。
- 结论：verified（E08/E10 子范围：撤销链经过经济实验时清理能完成，经济历史被保留；start 不再在来源撤销后写入作业）。固定 head `0c1bacd1752e20a7f1de2ab6058fec6271781d8f`（已合并，merged_sha `16bdb08e1ac118e3e732a912358add468c49173b`）；须在 #84 之后合并。回滚点 `d4837f5`。
- 不可声明：经济记录受恢复保护（不在 PROTECTED_* 中，另立任务）；cancel、record_budget_cost 等账务路径重验来源（按裁决不重验）。

#### CTRL-AG040-R1 / PR #87：第二轮验收（退回 R2）
- R1 `9b523e3`：已启动世界的 status 也复核存储世界的 registration_fingerprint，以及 registered_budget 还原出的登记时计数器。
  - 实现：共用私有函数 `ensure_registered_as`，status 的入口为 `ensure_world_registered_as`，世界与 fact 在同一快照内读取。
  - 新增 3 个测试，覆盖 29 种篡改。修复前这 29 种篡改全部返回 Ok(Succeeded)。
- 线性叠放：两个提交从 f15a42f rebase 到 #86（0c1bacd）之上，得到 `3d62150`，改动行逐行相同。以 force-with-lease 推送，并改写了 PR 正文。
- 主控复跑（全新 target 目录 target-ctrl-ag040，`qa/ctrl-stack-ag040-3d62150.log*`）：fmt 0；clippy 0；全工作区 74 个测试二进制，954 通过 / 0 失败（933 + 21）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例（探针源 `qa/probe-ag040/append.rs`，日志 `qa/ctrl-ag040-adversarial.log`）：
  - K2（通过）：claim 在途时，status Ok、重登记 AlreadyRegistered；用原请求 resume 后（结局 usage unknown，花费照扣，计数器 990/2），以及第二次 dispatch 后（965/2），status 与重登记都成立；换 key 的 start 首决策一致。
  - K1（**发现**）：一致地篡改两条记录（第一个已完成 fact 的 decision 成本由 10 改为 5，世界剩余由 965 改为 970），status Ok(Succeeded)、重登记 AlreadyRegistered、换 key 的 start Succeeded。根额度被抬高 5 micros 而未被发现。原因是 registered_budget 把 fact 自报的花费当真，而该成本本由世界中受指纹保护的不可变事实决定：root opportunity 的成本，或 successor_cost_upper_micros。
- 裁决 R2：registered_budget 对每个 fact 核对 selected_action 与 decision.action 的成本，必须等于世界不可变登记事实中该动作应有的成本（动作种类与 action_seq 的对应规则以 derive_legal_actions 为准）；不符时返回 Conflict。已退回原实施方，在 3d62150 之上追加提交。

#### CTRL-AG043-R1 / PR #88：E08/§11 来源撤销之后到达的模型响应照常计费、不可用、不留明文（主控独立验收）
- 交付 `b1c1bde`（基线 0c1bacd，即当时的栈顶，无需叠放）：budget.rs +82/−16；新增存储测试 11 项、引擎测试 4 项；broker.rs 未改。
  - settle_model_budget_call 在同一事务内用 AG-038 的辅助函数读取该调用登记的闭包。任一来源以 run 被撤销时：
    - 费用照常入账（apply_charge、金额、usage id 不变）；
    - 响应被拦，原因固定为 source_revoked（排在既有原因之首）；
    - transport 存为 redacted_artifact 形态；
    - 事件中附带 source_revoked 与 transport_original_digest。
  - 对 AG-038 辅助函数的调整：redacted_artifact 遇已脱敏的正文保持不变，保留原始摘要；结算重放可接受同一 transport 的脱敏形态。
  - 闭包不可解码时，结算以 Internal 失败且不写入，调用仍为 Dispatched，可由 reconcile 记账。
  - 修复前失败、修复后通过：存储测试 9/11、引擎测试 3/4 在基线上失败；7 处变异全部被抓住。
- 摘要一致性：存储的 digest 是脱敏正文的 digest（artifact_from_row 每次读都会复核 hash(body)==digest）。原始 schema 与 digest 放在脱敏正文和事件中。
- 主控复跑（全新 target 目录 target-ctrl-ag043，`qa/ctrl-stack-ag043-b1c1bde.log*`）：fmt 0；clippy 0；全工作区 75 个测试二进制 948 通过 / 0 失败（933 + 15）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例 2/2（探针源 `qa/probe-ag043/append.rs`；日志 `qa/ctrl-ag043-adversarial.log`）：
  - S1（来源存活时结算、之后撤销）：结算时 usable=true，transport 与 response 含明文。撤销并清理到 Complete 后，request、transport、response 全部为 rsia.redacted.v1，usable=false，cost=10 照记，任何表行中都没有模型输出；只有 WAL 文件里还有物理残留（已披露的边界）。
  - S2（撤销后重放存活期结算）：重放返回 Ok，账目不变，未写入新的明文。观察：重放返回的是原先可用的结算，清理到达之前仍为 usable=true、transport 为明文；能否被下游使用由消费者的存活检查决定，归 AG-045（迟到 DispatchObserved）。
- 结论：verified（E08/§11 子范围：来源撤销之后才结算的模型响应照常计费、不可用，账本中不留明文；来源存活时结算的响应，在撤销清理之后不留在任何表行中）。固定 head `b1c1bdefaf8e538f36287ca9b287c3aa2f41f18d`（已合并，merged_sha `ca117c45d87986500b38bc5afee3d6e161fea4f5`）；须在 #86 之后合并。回滚点 `0c1bacd`。
- 不可声明：
  - 迟到的 DispatchObserved/ResponseObserved 事实不留明文（AG-045）；
  - 无闭包的调用（只靠复扫兜底）；
  - SQLite 空闲页与 WAL 中的物理残留（未启用 secure_delete，也未 VACUUM）；
  - 其他拦截原因（租约过期、root/group 停止、超支、模型不符）下的 transport 仍按"留作审计"保留明文，与此前相同。

#### CTRL-AG040-R2 / PR #87：返修验收（verified）
- R2 `dc8b1c2`：新增 `registered_action_cost`。
  - Widen 用 `derive_legal_actions(world, &[])` 做整体比对，成本取 root opportunity 的成本；Deepen 与 Recover 取 successor_cost_upper_micros，且不得带 root 的 action_seq。
  - registered_budget 对每个 fact（含 Claimed）核对 decision 与 selected_action 的成本，加回时用世界的成本。selected_action 没有成本时拒绝。
  - 新增 4 个测试；登记与 status 共用的用例表增加 8 例。修复前，这 22 种篡改在各检查面上全部被接受；6 处变异被抓住。
- 线性叠放：3 个提交从 0c1bacd rebase 到 #88（b1c1bde）之上，得到 `c44aa8d`，补丁逐字节相同。以 force-with-lease（锁定 dc8b1c2）推送，PR 正文改为叠在 #88 之上，然后转 ready。
- 主控复跑（target-ctrl-ag040，`qa/ctrl-stack-ag040-c44aa8d.log*`）：fmt 0；clippy 0；全工作区 76 个测试二进制，973 通过 / 0 失败（948 + 25）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例复验（`qa/ctrl-ag040-adversarial-r2.log`）：
  - K1：修复前 status 为 Ok、重登记为 AlreadyRegistered；修复后 status 与重登记都是 Conflict("the dispatch facts of the exploration world do not account for its budget")，换 key 的 start 为 Failed/conflict。
  - K2：仍然成立，计数器 990/2 → 965/2。
- 结论：verified（E09/E07 子范围：探索世界的登记指纹只含不可变登记字段；计数器按登记时的值从 dispatch fact 精确还原，每笔成本绑定到世界不可变的成本；dispatch 之后的重登记幂等；exploration.start 的 status 与崩溃重跑在世界启动后成立，已启动世界的 status 复核指纹与登记时计数器）。固定 head `c44aa8d7db9d79f0b62379239e06184e49b5f73b`（已合并，merged_sha `32a2f5090fe5136b21c7d3454f53964b36990247`）；须在 #88 之后合并。回滚点 `b1c1bde`。
- 已知边界（如实披露，主控接受）：
  - fact 与节点尚未互相绑定。把 root dispatch 改写为 Deepen，并配套修改世界计数器，这种多记录篡改目前查不出来；已并入 AG-041。
  - registered_action_cost 依赖派生规则"Deepen 与 Recover 的成本 = successor 成本"。
  - 预填调度态的缺口已并入 AG-041。
  - meta.rs:649–653 与台账第 93 行的过期文字，由主控在台账 PR 中处理。
- 派发（21:45，基线 c44aa8d = 栈顶 AD）：
  - **AG-041**（卡 SHA-256 eeefd30c…，E09 B1）：
    - 付费之后的路径一律收敛为终态：前缀漂移、候选材料不可得、Recover 目标缺失、claim 输入变化，各用固定字面量记为 Uncertain；
    - 节点 id 超长与新世界的预填调度态在付费之前拦截；
    - registered_budget 把每个 fact 与其节点互相绑定。
  - **AG-045**（卡 SHA-256 86262728…）：来源已撤销时，迟到写入的 DispatchObserved 与 ResponseObserved 事实以脱敏形态落盘，并同步脱敏其幂等缓存。
  - AG-044（提交时上游闭包）仍在实施，基线为 0c1bacd，届时叠放到栈顶。

#### AG-044 交付与叠放（21:55）
- 交付 `170ce02`（基线 0c1bacd），一个提交。
  - lib.rs 新增 `Session::upstream_closure`：递归 CTE；用 CROSS JOIN 固定连接顺序，LIMIT 写在 CTE 内部；根以一个 JSON 参数传入。
  - dispatch.rs 采用两阶段判定，阶段 2 的上限为 10_000。
  - 新测试：存储 14 项、引擎 8 项。按卡授权，对三处既有测试做了最小修改。
- 实施方实测：
  - CROSS JOIN 是必需的。若让规划器自选连接顺序，它把 dependencies 表放在外层循环；11 个节点时为 193 ms，而现写法为 0.175 ms。
  - 恰好 10_000 个节点的闭包，经真实调用耗时 47 ms。
  - 主控探针在 0c1bacd 上为：Q1 接受 20 次、63 步；Q2 接受 141 次、724 步。本 PR 上为：Q1 接受 0 次、11 步；Q2 接受 0 次、22 步。
- 线性叠放：从 0c1bacd rebase 到 #87（c44aa8d）之上，得到 `f7946f0`，改动行逐行相同。以 force-with-lease（锁定 170ce02）推送，并改写 PR 正文。主控复跑与反例正在进行。
- 设计注意（主控接受）：
  - 本 PR 唯一依赖时间的测试是 timing 守卫，界限约为实测值的 100 倍。
  - cleanup_fixpoint_v42.rs 文件头注释（6–16 行）仍描述"接受后失败"，已过时，留待后续顺手修正。
- 顺带观察（另立任务）：lib.rs:718 的 load_dependency_record_snapshots（本地导出的闭包遍历）有同样两个问题：LIMIT 写在外层且带 ORDER BY，连接顺序由规划器自选。大 namespace 下可能要扫描数分钟。
- 主控复跑（全新 target 目录 target-ctrl-ag044，`qa/ctrl-stack-ag044-f7946f0.log*`）：fmt 0；clippy 0；全工作区 78 个测试二进制 995 通过 / 0 失败（973 + 22）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例 2/2（探针源 `qa/probe-ag044/append.rs`；日志 `qa/ctrl-ag044-adversarial.log`）：
  - U1：撤销 source-2 之后，从 Pending 到 Complete 的 12 个清理状态中，replay.run 在提交时全部被拒，且什么都没写（计数不变、没有幂等行）。拒绝理由分两类：8 次为阶段 2 "closure holds run source-2"，池被脱敏之后的 4 次为阶段 1 "redacted"。清理在 11 步内到达 Complete，与无迟到写入时的基线相同。
  - U2（精确性）：撤销与池无关的 run 之后，replay.run 照常被提交闸门接受。作业因水位变化以 conflict 失败，这是既有的"任一撤销使已封存世界失效"语义，与本闸门无关。
- 结论：verified（E07/E08/§11 子范围：撤销之后，从被撤销来源闭包派生的新管理请求在提交时即被拒，什么都不写；闭包超限时拒绝而不是截断；已接受的 request_key 重放仍先返回原作业）。固定 head `f7946f02407bb5694f08bd9f357425e2f1ad2262`（已合并，merged_sha `cc85b5c71cec06763909fb0efa33c4e296b6178d`）；须在 #87 之后合并。回滚点 `c44aa8d`。
- 不可声明：非管理写入点的闸门（grant、stage_package 归 AG-046，迟到 DispatchObserved 归 AG-045）；Complete 之后的迟到写入；没有登记依赖边的来源。

#### AG-045 第一轮（#90）与 R1（22:05）
- 交付 `74504fa`（基线 c44aa8d）：
  - optimization.rs +115：StoreOptimizationJournal::commit 在同一事务内判定 model stage 的 DispatchObserved/ResponseObserved 的 run 依赖是否被撤销。判定按 kind 精确匹配，不看水位。命中时：
    - 以 rsia.redacted.v1 写入，不写 payload；
    - 保留依赖边；
    - 写入幂等行后立即 redact_cache；
    - 审计记为 optimization.stage.commit_redacted。
  - lifecycle.rs：把脱敏构造抽成 `pub fn redacted_object_body`，由清理调用，行为不变。
  - 新测试 15 项：基线上 10/15 失败，8 处变异被杀死。
- 线性叠放：从 c44aa8d rebase 到 #89（f7946f0）之上，得到 `6251250`，补丁逐字节相同；已推送。
- 裁决 R1（已退回原实施方）：
  1. 不可解码的 tombstone 改为 fail-closed，与 AG-032、AG-038、AG-043 同口径。授权最小修改 tests/optimization.rs 中 exploration_late_revocation 的断言（约 1278 行）：该断言把"畸形 tombstone 下保留明文"写成了预期，正是本卡要消除的旧行为。
  2. StoreOptimizationJournal::lookup（以及 RecoveryJournal 的 lookup）读到 rsia.redacted.v1 时，返回点名的 Conflict，不再返回 Internal，与 AG-024、AG-027、AG-032 的读侧惯例一致。若有调用方依赖 Internal 做分支，停下回报。
  3. lifecycle.rs 的重构，以及"对 fact 的全部 run 依赖生效"，主控接受。

#### AG-041（#91）交付与主控裁决（22:20）
- 交付 `0bf151a`（基线 c44aa8d）：exploration.rs +958/−195；新测试 exploration_postpaid_v42.rs 共 14 项集成测试，另有 8 个模块内单测。基线上 11/14 失败。
  - 付费之后的路径统一收敛为终态：
    - 尾部先重读 dispatch 状态，落后的 resumer 返回胜者的终态；
    - 前缀漂移、claim 输入变化、Recover 目标缺失都写成固定字面量原因的 Uncertain；
    - settle 在同一 session 内一次性写入 node、边、dispatch 与 world；若写入被存储拒绝，返回原 Err，claim 保持可续。
  - 付费之前拦截：ensure_nodes_nameable 同时检查下一个节点与第 MAX_NODES 个节点，等价于登记时 120 字节的界限；ensure_new_world_shape 要求初始调度态。
  - fact 与节点绑定：node_placement 与 node_follows_action。
  - (d) 在端到端路径上无法复现（编辑编译器在 step 成为候选之前已校验并取摘要），由单测覆盖。
- 待裁决项：把 Recover fact 绑定到节点，会使 AG-040 测试 `a_dispatch_that_spent_a_recovery_dispatch_is_added_back` 失败。该测试的夹具把真实 root dispatch 改写成"自己节点的 Recover"，正是绑定应当拒绝的不一致 fact。实施方把完整规则放在本地提交 74cf54b 上：解除豁免、新增 Recover 绑定测试，并修正 AG-040 的夹具（改写的 dispatch 改为第二个真实 dispatch，其节点改写为失败节点的子节点）。全工作区 996/0。
- 主控裁决：采纳 74cf54b。Recover 绑定现在就完成卡中第 3 项，不留给 AG-042；授权修正 AG-040 的测试夹具，测试本意不变。
  - 主控以快进方式把 74cf54b 并入 PR 分支，再连同两个提交从 c44aa8d rebase 到 #89（f7946f0）之上：fdf0cce、4254e6d。改动行与原提交逐行相同。
  - 改写第二个提交的标题，去掉 "(needs a ruling)"，正文追加裁决说明与 Co-Authored-By。以 force-with-lease（锁定 0bf151a）推送。
  - 主控复跑与反例正在进行：AG-039 的 L/M 与 AG-040 的 K1/K2 在新 run_next 上回归。
- 实施方偏差（主控接受）：
  - 终态写在检测到条件的同一 session 内，没有另开 session；
  - ensure_nodes_nameable 检查到第 MAX_NODES 个节点；
  - settle 不覆盖已存在的节点记录；
  - waits 取自当前合法集；
  - Recover 目标不是世界所列的第 n 个节点时，视为缺失。

#### AG-045 R1（#90）
- R1 `57c1814`（在 6251250 之上快进）：
  - run_dependency_revoked 只有遇到可解码且 source_kind 为 "artifact" 的 tombstone 才放过，其余一律 fail-closed；
  - 新增 read_stage_fact，lookup 读到 rsia.redacted.v1 时返回点名的 Conflict，其余解码失败仍为 Internal；
  - 按授权修改了 tests/optimization.rs 中 exercise_late_revocation 的断言。
- 新测试文件共 19 项。在 6251250 上修复前失败 6 项，tests/optimization.rs 在 1282 行失败；8 种变异全部被抓到。
- 调用方核对：没有任何调用方依赖 Internal 分支。
- 待叠放：AG-041 验收后，从 f7946f0 叠放到 AG-041 head 之上，再复跑并做反例。

#### CTRL-AG041-R1 / PR #91：E09 付费后路径一律收敛为终态、确定性失败付费前拒绝、新世界初始调度态、dispatch fact 与节点互相绑定（主控独立验收，23:25）
- 交付：fdf0cce、4254e6d（含主控裁决采纳的 74cf54b：Recover 绑定与 AG-040 夹具修正），叠在 #89（f7946f0）之上，改动行与原提交逐行相同。
- 主控复跑（全新 target-ctrl-ag041，`qa/ctrl-stack-ag041-4254e6d.log*`）：fmt 0；clippy 0；全工作区 79 个测试二进制 1018 通过 / 0 失败（995 + 23）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 回归探针（`qa/ctrl-ag041-adversarial.log`）：AG-039 的 L（20 个随机世界走到 Stop 且可读）、M（伪造标签既不扩展也不卡住世界），AG-040 的 K1（两记录一致篡改被 status、重登记、换 key 的 start 拒绝）、K2（续跑的 claim 保持 status 与登记）全部通过。
- 主控反例（探针源 `qa/probe-ag041/append.rs`；head 日志 `qa/ctrl-ag041-adversarial-n.log`，栈顶 e7c0d0f 日志 `-on-e7c0d0f.log`，基线 f7946f0 日志 `-base-f7946f0.log`；首轮两次因探针错误中断，日志留作 `*.run1.log`：一次请求的 step 未按世界 decision_round 绑定，一次把 Stop 当作拒绝）：
  - N2：每一轮付费期间改写世界的 decision_round（另一世界只改第一轮），直到世界 Stop。每个已付费 dispatch 恰好一个终态节点（uncertain + exploration_prefix_changed_after_dispatch，或 observed），成本只扣一次；重连返回同一终态且不付费；每轮之后登记与 status 都成立；指向别的轮次的请求在付费前被拒、什么都不写；第 3 轮 Stop，计数器 (965, 2)。基线 f7946f0 上第 1 轮即 Conflict("exploration prefix changed while optimization was running")，claim 留在 Claimed。
  - N3：第二步付费期间撤销世界的来源，清理在 0–119 步时返回（Pending → Running → Complete 共 120 个状态）。run_next 从不返回 Internal（世界可读时为终态 uncertain，世界被脱敏后为点名的脱敏 Conflict）；清理无错误完成后，世界的探索记录无一明文残留（含未结的 claim）；随后同一请求与新请求都不付费、不写入。基线上同样成立（无回归）。
- 结论：verified（E09 子范围：付费之后不再有永久卡死的 claim，前缀漂移、claim 输入变化、Recover 目标缺失、候选材料不可得均收敛为恰好一个终态节点；节点 id 超长与新世界预填调度态在付费前拒绝；登记与 status 把每个已完成的 Widen/Deepen/Recover fact 与其节点对齐）。固定 head `4254e6d9408dfe4bdba4526a19de4987e6244198`（已合并，merged_sha `a38b1452ede8fc5755bcab65686edd2f2817b51a`）；须在 #89 之后合并。回滚点 `f7946f0`。
- 不可声明：Recover 端到端（AG-042）；瞬时存储错误之后的自动续跑（Claimed 仍需原请求 resume）。
- 主控观察（记入 AG-042 卡）：Recover 的 action_seq 若只由失败节点决定，同一节点的第二次 Recover 会撞上第一次的 dispatch id，需在 AG-042 中核对。

#### CTRL-AG045-R1 / PR #90：E08/§11 来源撤销之后写入的 stage fact 以脱敏形态落盘、不留模型输出（主控独立验收，23:25）
- 线性叠放：两个提交从 f7946f0 rebase 到 #91（4254e6d）之上，得到 `8ca50a1`、`e7c0d0f`，补丁逐字节相同；以 force-with-lease（锁定 57c1814）推送，PR 正文已改写，转 ready。
- 主控复跑（全新 target-ctrl-ag045，`qa/ctrl-stack-ag045-e7c0d0f.log*`）：fmt 0；clippy 0；全工作区 80 个测试二进制 1037 通过 / 0 失败（1018 + 19）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例 2/2（探针源 `qa/probe-ag045/append.rs`；head 日志 `qa/ctrl-ag045-adversarial.log`，基线 4254e6d 日志 `qa/ctrl-ag045-adversarial-base4254e6d.log`）：
  - W1（租户隔离）：n 撤销 run-failure（清理到 Complete），m 只在自己的 namespace 写 run-success 的 run tombstone。四种组合中，事实只在"本 namespace 的 tombstone 命中其 run"时被脱敏（对象与幂等行都脱敏，输出在库文件与 WAL 中都不存在）；run 只在另一 namespace 被撤销时，事实照原样存储（输出物理存在，作为对照）。基线上两种撤销组合都以明文落盘。
  - W2（每个清理状态）：迟到观察在清理的 5 个状态（Pending、Running×3、Complete）分别落地，之后清理跑到 Complete。每个状态下提交都被接受，输出既不在库文件与 WAL 中，也读不到；事实即清理所做的脱敏（边保留、行脱敏）；清理无错误完成；只依赖存活 run 的对照事实保持原样、输出物理存在。基线上从第一个状态起输出就物理留在 WAL 中。
- 结论：verified（E08/§11 子范围：来源以 run 撤销之后写入的、含模型输出的 stage fact 以 rsia.redacted.v1 落盘，不留模型输出明文，也不形成可用观察，幂等行同步脱敏，依赖边保留；判定按 namespace 与 kind 精确，不看水位；读取时为点名的 Conflict）。固定 head `e7c0d0f94c2915cc95663f8673d6c62f87ada2e6`（已合并，merged_sha `ecdfe31faabb21a368f08d341b2a6e15223b0849`）；须在 #91 之后合并。回滚点 `4254e6d`。
- 不可声明：撤销之前已写入明文的 SQLite 物理残留（空闲页与 WAL）；无 run 依赖的事实；开发 runner 只含分数的事实（不含模型输出，不处理）。
- runbook 增加 AF（#91）与 AG（#90）；merge-all.sh 覆盖 A–AG，共 33 项，与 runbook 逐项核对一致。全部 27 个开放 PR 对 main 显示 CONFLICTING，根因同 #65 的 criss-cross，待用户按方案 (a) 或 (b) 处理 #65 后按序合并。

#### AG-042 侦察与派发（23:40）
- 侦察（rsia-scout，树 4254e6d，只读）要点：生产代码仍零构造 RepairableFailure；settle 就地改写失败节点计数，且新节点 rfd 不继承，与回放（replay.rs 629-707）口径不同；推导出 S1（rfd 达 2 后 decide/status 全 Err，世界卡死）、S2/S3（engine 与回放的 rfd 不一致，可致决策分歧）、S4（存量非受支持类别使前缀校验失败）；改回存储计数即可重新打开已用的修复机会。NoChange 与 KeepIncumbent 都记为 HardFailure，Rejected/Uncertain 的原文 reason 直接入库。
- 切分：AG-042 = B4a（计数派生、与回放同口径、Recover 提供条件与目标绑定，只改 exploration.rs）；AG-047 = B4b（类型化来源）；AG-048 = B4c（NoChange/KeepIncumbent 终态与固定字面量 reason）。
- 派发 AG-042（卡 SHA-256 03703dba…），worktree ag-042，基线 e7c0d0f（栈顶）。

#### #65 修复与 AG-042 裁决（2026-10-01 00:20）
- 用户首次运行 G-fix 时 fetch 遇 SSL_ERROR_SYSCALL，什么都没做。第二次在 zsh 下 `"$X:refs/..."` 的 `:r` 被当作修饰符，推送引用变成 `…c35aefs/heads/…`，五次推送都失败，没有推送任何东西。
  - 已生成的 X = eb64a4d 经主控只读核对：树 = f752e1d 的树，父为 f752e1d 与 f8fa1a1。
  - 已给出字面 SHA 版命令（user-commands.md"G-fix 更正"）。
  - merge-all.sh 加固：状态读取重试、关闭 gh 交互提示、#65 冲突时给出指引并停止。
  - 此前一次运行中 EOF 让 gh 对已合并的 #61 弹出"删除分支"提示，无害。
- AG-042 实施方回报：按卡严格读取 fact 会使 meta_inheritance_v42.rs:1054 失败（该测试把唯一的 fact 改为不可读后仍断言 decide_next 成功）。
  - 主控裁决 B：仅当世界存在 RepairableFailure 节点时读取 fact，此时不可读返回点名 Conflict；无 RepairableFailure 节点时不读（自洽世界里此时派生视图恒为空，不自洽世界由 registered_budget 拒绝）；该测试不改。加两例钉住测试与两项变异。
  - 按 episode 计数（§787）确认采纳；与回放按节点计数在同 episode 跨节点时的差异写入已知边界，回放对齐留给 AG-047，并加一例钉住。
  - rfd 的保守判定（fact 选了 Recover，或父为 RepairableFailure）确认采纳。
- 2026-10-01 00:25：用户运行字面 SHA 版 G-fix：快进推送 f752e1d..eb64a4d，并以 eb64a4d 钉住合并 #65，merged_sha `c763973`（main = c763973）。主控核对：main 的树等于 f752e1d 的树；H（#67）与 main 的合并基唯一为 f752e1d；GitHub 显示 #67 MERGEABLE。用户接着运行 merge-all.sh（H–AG）。

#### 合并完成（2026-10-01 07:24Z，用户运行 merge-all.sh：ALL MERGED: A-AG）
- main = `ecdfe31faabb21a368f08d341b2a6e15223b0849`；主控核对：main 的树等于主控验收的栈顶 e7c0d0f 的树（逐字节同一内容）。开放 PR 为 0。
- 各 PR 的 merged_sha（PR、验收 head、merged_sha 全长、mergedAt；原始数据 `qa/merged-59-91.txt`）：
  - #59 head 9dec327 → merged_sha `69a3b3ed04377c30ee7193c0679a3747f1606a1e`（2026-10-01T02:07:23Z）
  - #60 head 2e5c668 → merged_sha `dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b`（2026-10-01T02:02:19Z）
  - #61 head 3a0e26a → merged_sha `b220971c4e5ad44453c9d3e5787fbdfda6eb01ac`（2026-10-01T02:07:33Z）
  - #62 head 0d6ec3d → merged_sha `9c3ec979fd2ee3c53dfbf4ddf96967e94808b11e`（2026-10-01T02:07:40Z）
  - #63 head b1d008f → merged_sha `3e2fc067190077b8cf8401875d6c627b8d9e13fc`（2026-10-01T02:07:49Z）
  - #64 head 71c0fa8 → merged_sha `f8fa1a19d5e968f83a52385ecb43d07d56442610`（2026-10-01T02:08:14Z）
  - #65 head eb64a4d → merged_sha `c763973ddf57c6b7babcd9bd6fa87025f367aef3`（2026-10-01T07:15:14Z）
  - #66 head 3dd8878 → merged_sha `5a0ef4419c51687534baa3296dbbf7e310a5d810`（2026-10-01T07:19:05Z）
  - #67 head 3f96c53 → merged_sha `5c4f81dca79373c6871449aed287f61fb1f3cbc2`（2026-10-01T07:18:53Z）
  - #68 head 44b4c90 → merged_sha `3ffe1e26a908d4c9240320d52fcba1e92dc383d4`（2026-10-01T07:19:15Z）
  - #69 head c841d1a → merged_sha `31566ce87203b44c44b72986233af1c02fbe5cda`（2026-10-01T07:19:26Z）
  - #70 head 551aa33 → merged_sha `c81b258b6f890ab78bc9c7111798f75560c2ea23`（2026-10-01T07:19:52Z）
  - #71 head 605d92a → merged_sha `9519c26f049afff63154fb2337893217b18b7e3c`（2026-10-01T07:19:42Z）
  - #72 head 2df06e7 → merged_sha `15b140ddef25c8e794d586fecae56f129226a5c6`（2026-10-01T07:20:04Z）
  - #73 head 6a90635 → merged_sha `7ad4994c7a9208f523612d305660dc8ca9529620`（2026-10-01T07:20:36Z）
  - #74 head d2929c7 → merged_sha `c72aae14c9f35c1bcc090b5a374d2696d57dacfc`（2026-10-01T07:20:50Z）
  - #75 head 145f9cf → merged_sha `6a23cb3697f656e1f5132cbe62ae766b4c0383f0`（2026-10-01T07:20:27Z）
  - #76 head 43984ab → merged_sha `7120842a1f586ae394b76e146c8298f11bec52bc`（2026-10-01T07:21:11Z）
  - #77 head 400f707 → merged_sha `887b1d714581ae8a71d45243d38be1759697600d`（2026-10-01T07:21:00Z）
  - #78 head 892ed1c → merged_sha `84d8a7054c2428db0f821fcd478c615b09ade76b`（2026-10-01T07:21:21Z）
  - #79 head 8954b6c → merged_sha `b262bbb2a0ea9ead68bc33fe1e58235e85bcffea`（2026-10-01T07:21:37Z）
  - #80 head 04e6309 → merged_sha `9e3e350ed5fbef2975117f12a7f8d3ca3b9cdcbf`（2026-10-01T07:21:49Z）
  - #81 head c5c04ba → merged_sha `d9b3f6f5234ad14da18956a78c804e4c6818fa10`（2026-10-01T07:21:59Z）
  - #82 head f15a42f → merged_sha `8d959dee844fb16f0763b0684ba3adf48ad75f1f`（2026-10-01T07:22:22Z）
  - #83 head fee5897 → merged_sha `51db9b6c9a251def4e6566c96641ed8b63d43a85`（2026-10-01T07:22:11Z）
  - #84 head d4837f5 → merged_sha `42f89bd602db0c2783de38d87d91e80cd65f220b`（2026-10-01T07:22:45Z）
  - #85 head e06bfdd → merged_sha `f4b1ef66340412f5c0edf69a48549cd1386324cf`（2026-10-01T07:22:32Z）
  - #86 head 0c1bacd → merged_sha `16bdb08e1ac118e3e732a912358add468c49173b`（2026-10-01T07:22:56Z）
  - #87 head c44aa8d → merged_sha `32a2f5090fe5136b21c7d3454f53964b36990247`（2026-10-01T07:23:22Z）
  - #88 head b1c1bde → merged_sha `ca117c45d87986500b38bc5afee3d6e161fea4f5`（2026-10-01T07:23:12Z）
  - #89 head f7946f0 → merged_sha `cc85b5c71cec06763909fb0efa33c4e296b6178d`（2026-10-01T07:23:32Z）
  - #90 head e7c0d0f → merged_sha `ecdfe31faabb21a368f08d341b2a6e15223b0849`（2026-10-01T07:24:20Z）
  - #91 head 4254e6d → merged_sha `a38b1452ede8fc5755bcab65686edd2f2817b51a`（2026-10-01T07:24:08Z）
- 下一步（新窗口执行，属合并后的收尾）：台账 PR（把本草稿逐项折入 docs/implementation-ledger.md，写明每个 merged_sha）；reports/support-scope.json 登记各子范围（plan_version/plan_sha256 用 v4.2 那一对，plan_recheck=not_affected）；运行检查器与 `cargom test --locked -p evo-core --test support_scope`；清理已合并的 worktree（ag-018…ag-045）、target 与本地分支；本地 main 快进到 origin/main。

#### CTRL-AG042-R1 / PR #92：E09 恢复计数改为派生视图、与回放同口径；失败节点不再被改写；Recover 只对受支持类别且有余量的失败提供（主控独立验收，2026-10-01 01:00）
- 交付 `3d68d30`（基线 e7c0d0f，已在 main 中；对 main 的差异只有这一个提交，无需叠放）：exploration.rs +1269/−93（生产代码 +384/−77，模块内单测 12 个）；新测试 exploration_recovery_counting_v42.rs 15 个；按卡授权改动 3 处夹具（legal_actions、postpaid、start_after_dispatch）。实施中途按主控裁决 B 处理了 meta_inheritance_v42 的冲突（仅当存在 RepairableFailure 节点时读取 dispatch fact）。
- 实施方实测：基线上新测试 13/15 失败；18 处变异全部被杀。Cargo.lock 曾被裸 `cargo clean` 重排，提交前已还原，主控核对 diff 中没有 Cargo.lock。
- 主控复跑（全新 target-ctrl-ag042，`qa/ctrl-stack-ag042-3d68d30.log*`）：fmt 0；clippy 0；全工作区 81 个结果行 1064 通过 / 0 失败（1037 + 27）；build 0；两组 smoke 通过；检查器 structure_valid；unittest 48 OK。
- 主控反例 2/2（探针源 `qa/probe-ag042/append.rs`；head 日志 `qa/ctrl-ag042-adversarial.log`，基线 e7c0d0f 日志 `qa/ctrl-ag042-adversarial-base-e7c0d0f.log`）：
  - P1（§7.2.1 不靠改 episode 名重置修复机会）：节点 1 的修复 dispatch 之后，把它存储的 episode id 改名，单独改名或连同计数器归零都试。均不再提供 Recover，run_next 报 Stop，世界可读，登记成立。基线上"改名+归零"重新提供 recover-1，run_next 撞上 "dispatch idempotency conflict"。
  - P2（AG-041 × AG-042）：修复付费期间世界被改写，修复以 uncertain（exploration_prefix_changed_after_dispatch）终结。失败节点存储逐字节不变；恢复额度 2→1；子节点 rfd 为 0（回放口径）；不再提供该修复；重连返回同一终态且不付费；世界走到 Stop 并可读；登记成立。基线上失败节点被改写。
- 回归探针（head 上）：AG-041 的 N2 与 N3（120 个清理状态，无明文残留、无付费与写入）、AG-039 的 L（20 个随机世界）与 M、AG-040 的 K1 与 K2，全部通过（`qa/ctrl-ag042-regress-{n,lm,k}.log`）。
- 结论：verified（E09 子范围：恢复计数由 dispatch fact 与谱系派生，rfd 与 recovery_dispatches_used 与回放同口径；dispatched_repairs 按 episode 计数（§787，比回放的按节点计数更严，差异已钉住）；失败节点不再被改写；Recover 只对受支持类别、episode 有余量、rfd 低于上限的失败提供，前缀不会超过校验器上限；改存储计数或改名已修复节点的 episode 都不能重新打开已用的修复机会；Recover fact 与目标状态绑定）。固定 head `3d68d30a43c3fc0582053d48cf14ac35c3d4e841`（已合并，merged_sha `696252faf8ef2b2391102f4e6b0e9c0ee20c53d5`），PR 已转 ready，runbook AH。回滚点为合并前的 main（ecdfe31）。
- 已知边界（P1 发现，接受为本步边界，并入 AG-047）：同一 episode 的两个根中，修复花在根 1 之后，把根 2 存储的 episode id 改名会重新提供 recover-2。节点所属 episode 读自其存储状态，需要在 AG-047 铸造 episode 时绑定到不可改写的事实。目前没有任何 step 产生可修复失败，存储世界里尚不能出现这种情形。
- 也接受：既非信封也非墓碑的损坏 fact 正文仍为 Internal（沿用探索读取方的既有约定）；不支持的 schema、pre-E14、撤销墓碑、信封不匹配为点名 Conflict；缺失为 NotFound。
- 不可声明：生产路径能产生可修复失败，或 Recover 端到端（AG-047）；NoChange/KeepIncumbent 终态（AG-048）。
- 2026-10-01 07:59Z：用户合并 #92，merged_sha `696252faf8ef2b2391102f4e6b0e9c0ee20c53d5`；main = 696252f，其树等于验收 head 3d68d30 的树。本轮全部 34 个 PR（A–AH，#59–#92）均已合并，开放 PR 为 0。

### 派生索引登记（本 PR：reports/support-scope.json、索引测试与检查器单测）

- 按 §18.5/§18.8 与 `scripts/check_support_scope.py` 现行结构登记 31 条第二轮验证记录：E03 1、E07 4、E08 6、E09 8、E10 1、E12 1、E13 4、E14 2、E16.5 3、E16.6 1。每条 `plan_version` v4.2、`plan_sha256` `70ec06e4…`、`plan_recheck` not_affected；`source_sha` 为主控验收 head，`merged_sha` 与 `pr_state` merged 取自上表；`test_entry` 为该 PR 的主测试目标；`log_path` 各不相同（主控复跑的 `.test` 日志；#60–#64 第一轮的日志只有汇总行，于 2026-10-01 在各验收 head 上做单目标复跑，日志 `qa/ctrl-reg-*.log`，结果全部 exit 0）。
- `input_ref` 指向本地输入清单 `out/audit-20260930/qa/inputs/ctrl-input-<任务>-<head>.json`（PR、head、树、回滚点、merged_sha、变更文件及其 blob、测试目标 blob、实际命令与环境、日志与任务卡的 SHA-256），`input_digest` 为该清单的 SHA-256。清单与日志只在本地，不上传。
- `command` 字段：检查器的有限文法只接受 `cargo test --locked --offline -p <包> --test <目标>`。本机实际运行一律为 `cargom test --locked …`（rsproxy 镜像源替换、不带 `--offline`、Cargo.lock 不变），实际命令写在每条记录的 `actual_result` 与输入清单中。
- 未登记（证据不在检查器单目标 cargo test 文法内，只在本台账记录）：#59 AG-013（`scripts/smoke_cli.py`）、#68 AG-023（apps/rsia 二进制 crate 单测与 smoke）、#65 台账。
- 状态与范围：E14 由 planned 改为 in_progress（AG-019、AG-034 程序子范围已验）；其余 E 状态不变，status_reason 与 remaining 按本轮事实更新（含 E03/E07/E12/E16.6 原文中已过时的“回执 schema 缺失”“exploration/curriculum 管理适配未接”“AG-012 待验收”）。implementation_files：E03 加 development.rs，E09 加 groups.rs、practice.rs，E14 加 evo-core improver.rs，E16.5 加 startup_gate.rs。overall_remaining 增加 §3.3.1 修订事项。V 场景与 B/U/K 追踪条目未改。
- 索引测试与检查器单测：`crates/evo-core/tests/support_scope.rs` 的已审合并表加入 #60–#64、#66、#67、#69–#92 的 merged_sha；其中两处“planned 示例”由 E14 改为仍为 planned 的 E15。`scripts/test_check_support_scope.py` 中以 E14 为 planned 夹具的用例同步改为 E15；“真源绑定”用例改为让绑定当前版本的记录随夹具摘要变化（历史 v4.1 记录保持不变），断言改为夹具至少含一条 v4.1 记录。检查器 `scripts/check_support_scope.py` 未改。
- 主控复跑（AG-049 worktree，输入清单与日志复制到被忽略的 out/ 下）：检查器 `--source-of-truth <v4.2>` structure_valid=true、plan_binding_available=true、errors=[]，新登记的 31 条输入清单摘要全部核对一致（needs_verification 中 22 条“输入清单不可得”均为旧机丢失的历史记录）；`cargom test --locked -p evo-core --test support_scope` 14 passed；unittest 48 OK。全量复跑结果见本 PR 的主控验收说明。

### 已知边界与后续队列（2026-10-01，按真源依赖顺序排队，详见各卡）

- E09：AG-047（B4b：RepairableFailure 的类型化可信来源；episode 铸造并绑定不可改写事实——AG-042 反例 P1 发现改写兄弟根节点存储的 episode id 会重开已用的修复机会；回放 dispatched_repairs 按 episode 计数对齐）；AG-048（B4c：区分 NoChange 与 KeepIncumbent 终态，Rejected/Uncertain 的原文 reason 改为固定字面量）；E09 PR-C（优化历史进入请求与签名）。
- E08/§11：AG-046（grant/store_source_selection、stage_package、seeds 的撤销闸门）；tombstone 键带 kind（§11.1，涉及信任锚格式变更与恢复双读）；经济回放记录纳入恢复保护集合（PROTECTED_*）；SQLite 物理残留（secure_delete 或 VACUUM）。
- 性能与小项：lib.rs `load_dependency_record_snapshots`（LIMIT 在外层并带 ORDER BY、连接顺序由规划器决定）；收窄 AG-037 的 `write_started_job`；broker 派发被拒时写明真实释放原因；development runner 派发被拒时释放 1 micro。
- 过期文字：meta.rs:649–653 注释（仍称登记指纹覆盖 run_next 会扣减的计数器，AG-040 后已不成立）；cleanup_fixpoint_v42.rs 文件头注释（仍描述“接受后失败”，AG-044 后提交时即拒绝）。代码注释另派任务修正。
- 需先修订真源的事项（未派发，待用户决定）：§3.3.1 v4.2 补充要求新增内部对象引入时同步登记容量上限，而本轮新增的巩固 claim/proposal、实践集合、技能组作业、MetaTrial 等在真源中没有上限；§6.1 管理操作清单不含巩固操作（E13 管理入口）。
- 外部阻塞（只登记，不推进）：真实模型凭据；付费实验（E01 小试与正式样本、E11、E15）；额外真实宿主 Claude Code（E16.4）；真实沙箱（E16.5）；历史包与 T001–T126 归并（E00）；E14 机制冻结合同。

## 2026-09-30 第一真源前置审计与 v4.1→v4.2 切换（主控）

用户规则（2026-09-30）：除系统技能文件与本台账外，方案正文、归档、内部合同与原始 QA 一律不上传远程；台账必须完整记录。本机 `.gitignore` 已排除 `/RSIAgent-*.md`、`/archive/`、`/out/`，本轮 `git status` 干净。

### 审计输入与方法
- 输入：`RSIAgent-v4.1定稿-工程实施方案-2026-09-19.md`，322,750 字节，SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`（与本台账原绑定一致），全文 2,215 行逐节阅读；`out/handoff-20260930/HANDOFF.md`；本台账；main `0c43e0f`。
- 机械核查（脚本与日志仅本地 `out/audit-20260930/`）：§3.4.1/V085 的 k=12/13/14 上界（0.0310417011 / −0.0092392266 / −0.0449493587）、§7.5.1 的 P=2/5、V081.a 的 0.4/0.4375/0.0375 与立即 Stop=0.5，按正文公式独立重算一致；80 处 § 引用全部指向存在标题；E 任务前置图无环；V001–V098 全部被 E 任务引用且在 §13 定义；K/SO/B/U/RH/R 编号无缺口；H00–H24↔E 映射双向一致；§12 各 E 任务 V 清单与 `scripts/check_support_scope.py` 的 EXPECTED_E_SCENARIOS 一致（E16/E16.5 的 V081–V086 为范围写法）。

### 审计发现与裁决（详见真源 §19.4）
| 编号 | 发现 | 裁决 |
|---|---|---|
| R-A 关联链条 | §13.3 场景族表"主要责任任务"列与 §12 各任务清单在 9/12 族不一致（如 E16.1 按 V087.b 验证导入方伪造 used 却未列入 V087 族；V097 族列 E09 而 E09 清单无 V097）；派生索引已按 §12 清单反推 V→E，若按族表生成会断链 | 族表改为 §12 清单并集并声明以 §12 为准；E09 补列 V097（§3.1.1 消融义务） |
| R-B 内部一致性 | E14 模块写 `meta.rs（新增）`，与 §1.5/§16.5 W41-08"该文件已存在于固定源码"矛盾 | 改为按 E00 基线扩展、不重复建文件 |
| R-C 合同完整性 | §3.3.1 MVP 容量合同未覆盖探索节点/回放世界/lease/staging 包/管理并发；E16.5 已按 500/100/10/20/1 实施而正文无依据 | 在 §3.3.1 登记为设计初值；修改须先修订正文并由 E16.5 重验 |
| R-D 修订治理 | 正文无版本切换时历史验证记录、派生索引与检查器的绑定规则；单一常量摘要会使升级后所有历史记录失效或被静默改写 | 新增 §18.8 谱系、记录级绑定与 recheck_required 规则 |
| R-E 过程合同 | 增量任务（AG-nnn）分 PR、主控验收、管理员合并的实际流程无正文合同；模块名与实际文件不一致易被误判未实现 | §12 增补增量交付合同与"模块列表为逻辑落点" |
| R-F 证据可重现性 | 本地-only 冻结合同（F08）随旧机损坏丢失，导致 E16.2/E16.3 只能对照提案文本 | §18.5 要求机器可核部分随 PR 入库；合同不可得标 contract_unavailable |
| R-G 真源身份 | 页首/§1.1/§16.1/§17/§18.1/§18.2/V071/V098.c 仍指 v4.1 | 更新为 v4.2 并保留谱系与历史记录 |

不变项：E00–E18、E16 六子包、B01–B10、U01–U08、K01–K10、SO01–SO18、V001–V098 全部保留；无新增 E/V/数值门槛；默认关闭功能、支持范围、W_online=1、货币预算缺省 0 不变。审计确认无需修改：统计公式与判定顺序、W_sim、12/4/1、§6.3/§6.7/§8/§11.2 数值上限、E 前置与 G 门禁、K/SO 来源与许可边界。

### v4.2 切换动作（真源 §18.8）
- 生成 `RSIAgent-v4.2定稿-工程实施方案-2026-09-30.md`：332,938 字节，SHA-256 `70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455`；与 v4.1 的 diff 共 97 行，全部对应 §19.4 修订清单（脚本 `out/audit-20260930/make_v42.py` 逐锚点唯一替换）。
- v4.1 原件移入本地 `archive/2026-09-30-pre-v4.2/`，移动前后 SHA-256 均 `45f3ba06…`；仓库根目录只剩 v4.2；方案与归档不上传。
- 派生索引、检查器与索引测试的版本绑定与谱系：本节所在 PR（AG-012）。

### 2026-09-30 第二轮结论按真源正文复核（原"真源缺席、依台账记录范围验证"）
- #54（E16.2+E16.3）对 F08 提案的四处偏差逐条对照 §11.1–§11.3：无 `expires_at`（正文未要求）；预算以 `E04BudgetRef` 引用（符合 §3.5/§5.7 单一根账本与 §5.8 不另建可写事实库）；状态集 Prepared/Staged/Quarantined/Aborted/Failed、审批留在 ReleaseStore（符合 §11.1 staging≠Active、审批走 E06）；撤销后拒绝+redaction 而非字面 Quarantined 迁移（符合 §11.3 先提水位再分页清理）。四处均与正文一致；F08 冻结合同本身仍缺，按 §18.5 标 `contract_unavailable`。
- #44 对 §6.3/§5.6/E16.1、#47 对 §5.5/E16.4、#48 对 §3.3.1/§11.3–11.4/E16.5、#50/#51 对 §10/E17/E18、#56 对 E16.6、#57 对 §18.5 逐条对照，未发现与正文冲突；各 PR 引用的 V 场景均在对应任务 §12 清单内。结论：由"依台账记录范围验证"升级为"已按真源正文复核"，作用域不变，仍不构成效果或发行声明。
- 受 v4.2 修订章节影响需 recheck 的历史记录：E00 V071（入口/归档，已按 v4.2 复核：根目录唯一 v4.2、v4.1 已归档且摘要一致）；E16.5（§3.3.1 新登记上限与 `capacity.rs` 常量逐项相等）；E16.6（索引绑定，由 AG-012 更新）。其余记录 not_affected。

### 审计发现的实现缺陷与任务队列（每任务一个 PR；主控验收后由管理员合并）
| 任务 | E 归属 | 内容 | 状态 |
|---|---|---|---|
| AG-012 | E16.6/E00 | 派生索引、检查器、索引测试绑定 v4.2 与谱系；E09 加 V097；本节台账 | verified；PR #60 head `2e5c668`，已合并，merged_sha `dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b` |
| AG-013 | E00/E07 | `scripts/smoke_cli.py` 自 AG-001 起过时（replay.run 需完整 ReplayRunRequest），CI 质量门在 main 失败；新增完整 replay.run 缺池→failed 与未知字段→400 路径；blocked 代表随 AG-015 改为 meta.start | verified；PR #59 head `9dec327`，已合并，merged_sha `69a3b3ed04377c30ee7193c0679a3747f1606a1e` |
| AG-014 | E16.5 | MVP 容量门接入 prepare/管理 claim/探索节点/回放世界/staging 包/预算 lease 六个真实入口；主控返修 F24（计数须跨命名空间实例级）。启动恢复/部署门仍未接 | verified（容量门子范围）；PR #62 head `0d6ec3d`，已合并，merged_sha `9c3ec979fd2ee3c53dfbf4ddf96967e94808b11e` |
| AG-015 | E07 | curriculum.step 接入持久课程消费者（幂等回执、读侧重验） | verified；PR #61 head `3a0e26a`，已合并，merged_sha `b220971c4e5ad44453c9d3e5787fbdfda6eb01ac` |
| AG-016 | E03 | 可信开发执行/评分回执 schema、已登记纯函数运行器、观察门禁重写 | verified；PR #63 head `b1d008f`，已合并，merged_sha `3e2fc067190077b8cf8401875d6c627b8d9e13fc` |
| AG-017 | E07 | exploration.start 接入持久探索协调器（幂等注册、纯决策读侧重验）；meta.start 保持 blocked 直至 E14 | verified；PR #64 head `71c0fa8`，已合并，merged_sha `f8fa1a19d5e968f83a52385ecb43d07d56442610` |
| AG-020 | 台账 | 本轮主控验收记录、合并顺序与固定 head（PR #65） | 已合并（以 `eb64a4d` 固定 head，树 = `f752e1d`），merged_sha `c763973ddf57c6b7babcd9bd6fa87025f367aef3` |
| 后续 | E16.5 启动门/E14/E09/E13/E15/E16.4 | 用户指示（2026-09-30）：在途任务完成后收尾，不再新派。AG-018（启动恢复/部署门）与 AG-019（E14 单机制继承合同）只完成本地任务卡草稿，未派发；E16.4 无真实 Claude Code 宿主仍 blocked | 2026-10-01 更正：AG-018/AG-019 及后续 AG-021–AG-045、AG-042 已在第二轮派发、验收并合并，见页首第二轮节 |

### 合并顺序与固定 head（2026-09-30 末；全部 PR base=main，按序合并，每条用 --match-head-commit 固定）
1. #60 AG-012 `2e5c668`（独立）；2. #59 AG-013 `9dec327`；3. #61 AG-015 `3a0e26a`（叠 #59）；4. #62 AG-014 `0d6ec3d`（叠 #61；= 主控重叠 `d8bfdd5` + F24）；5. #63 AG-016 `b1d008f`（叠 #62）；6. #64 AG-017 `71c0fa8`（叠 #63；主控解决了两处测试文件尾部并行追加冲突并重建导入块）；7. #65 台账（本记录，叠 #64）。
- 栈顶 `71c0fa8` 主控复跑（`out/audit-20260930/qa/ctrl-stack-tip-71c0fa8.log`）：fmt 0、clippy 全工作区 -D warnings 0、全工作区 48 个测试二进制 536 通过 / 0 失败、`cargo build -p rsia` 0、`smoke_cli.py` SMOKE_CLI_OK、`smoke_workspace.py` OK、支持范围检查器 structure_valid（该栈不含 #60，故顶层仍 v4.1；#60 先合并）。main 基线 `0c43e0f`：47/495/0。
- 合并命令：本地 `out/audit-20260930/user-commands.md`（A–G）。合并后需另开台账增量记录 merged_sha，并按 §18.5/§18.8 把各子范围登记进 `reports/support-scope.json`（记录级 v4.2 绑定与 plan_recheck）。
- #59 合并前 main 上 CI 质量门仍失败（smoke_cli 过时）。
- 2026-10-01 补记：A–F 由用户于 2026-10-01T02:02–02:08Z 按序合并；G #65 因 criss-cross 合并基改以 `eb64a4d` 固定 head，于 07:15Z 合并。各 merged_sha 见页首第二轮节合并结果表。

### 主控验收记录（CTRL，2026-09-30）
### CTRL-AG013-R1 / PR #59：E00/E07 CLI smoke 刷新（主控独立验收）

- 真源：v4.2（SHA-256 `70ec06e4…`），E00"固定输入运行 fmt/check/test/clippy/build 与两组 smoke；检查真实退出码"、E07 本地协议子范围、§6.1 管理操作。固定 head `9dec3274716ac513d29f6f6f4d2e30793d4feeca`（base main `0c43e0f`），仅改 `scripts/smoke_cli.py`（+157/−29）。
- 复现旧缺陷：main 版脚本对同一二进制 exit 1，`management request failed with HTTP 400`（replay.run 自 AG-001 起需完整 `ReplayRunRequest`）。日志 `out/audit-20260930/qa/ctrl-ag013-smoke-cli-original.log`。
- 主控复跑：`cargom build --locked -p rsia` 0；`python3 scripts/smoke_cli.py` exit 0，末行 `SMOKE_CLI_OK model_transport=disabled benefit_claimed=false`；`python3 scripts/smoke_workspace.py` exit 0（`ctrl-ag013-smoke-*.log`）。
- 反例：把 blocked 断言取反后 smoke 必须失败 → exit 1（`ctrl-ag013-mut.log`），断言真实生效。新增覆盖：curriculum.step blocked 终点（state/step/error_code/id）、完整 replay.run 缺池 → failed/not_found、幂等重提同 id、未知字段与不完整载荷 → HTTP 400 invalid_input（CLI 非零）。原有 401/403/重复键/零技能/MCP/锁断言全部保留。
- 结论：verified（E00 两组 smoke 与 E07 管理接线 smoke 子范围）；不构成回放成功、模型或收益声明。PR 已转 ready，待管理员合并；merged_sha 待记。回滚点 `0c43e0f`。（2026-10-01：已合并，merged_sha `69a3b3ed04377c30ee7193c0679a3747f1606a1e`）

### CTRL-AG012-R1 / PR #60：派生索引、检查器、索引测试与台账绑定 v4.2（主控独立验收）

- 真源：v4.2 §18.8（谱系与记录级绑定）、§19.4、E16.6/V071/V080/V098、E00。固定 head `2e5c6682c33b629fd9ce9cda30578f0d4eb99866`（= 实施提交 `4912136` + 主控修正 `2e5c668`：把索引中过期的"PR #49 pending"改为 #56/#57 已合并事实，台账 E16 行同步），base main `0c43e0f`。
- 主控复跑：`check_support_scope.py --source-of-truth <v4.2>` structure_valid=true、plan_binding=true、errors=[]；`unittest` 48 通过；`cargom test -p evo-core --test support_scope` 14 通过；fmt/clippy 0（`out/audit-20260930/qa/ctrl-ag012-*.log`）。
- 反例（`probe_ag012_inplace.py`，12 例）：顶层仍绑 v4.1、记录摘要谱系外、v4.2 版本配 v4.1 摘要、plan_recheck=required、非法 plan_recheck、缺 plan_lineage、谱系截断、谱系摘要篡改、E09 缺 V097、受影响任务记录未 recheck → 全部拒绝；原件与"记录绑定当前 v4.2 对"→ 接受。首轮探针误用绝对路径被检查器拒绝，属探针缺陷已修正。
- 结论：verified（派生索引 v4.2 绑定与谱系子范围）。历史 22 条 verified 记录保留 v4.1 摘要，E00/E16.5 两条标 rechecked_v4.2。PR 已转 ready，待管理员合并；merged_sha 待记。回滚点 `0c43e0f`。（2026-10-01：已合并，merged_sha `dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b`）

### CTRL-AG015-R1 / PR #61：E07 curriculum.step 管理消费者（主控独立验收）

- 真源：v4.2 §6.1/§6.2/§6.6/§6.7.4/§8.1/§11.4、E07 增量、V082.a/c/d、V034、V017/V056、V008。固定 head `3a0e26af88744f93bc0a1874aafc5ac6f67595e4`（叠在 #59 的 `9dec327` 之上，PR base=main）。文件：dispatch.rs、curriculum.rs、tests/dispatch_management.rs、evo-http tests/service.rs、scripts/smoke_cli.py（blocked 代表改为 meta.start）。
- 实现要点：`CurriculumStepRequest`（deny_unknown_fields）→ Admin → `checkpoint_claim` → `schedule_probe_idempotent(job.id, …)`：以管理作业 id 为幂等键持久 `probe_schedule_receipt_v1`，探测作业/状态更新/依赖边/回执同一事务提交；崩溃重跑回读回执而非再次触发（§6.7.4）。`status` 对 Succeeded 作业以 `verified_probe_job_view` 重验（水位变更 Conflict、来源 tombstone Forbidden、结果不一致 Conflict），终态不改写。exploration.start/meta.start 仍 blocked。
- 主控复跑（`ctrl-ag015-*.log`）：fmt 0、clippy（core/engine/http，-D warnings）0；干净 head 全工作区 47 个测试二进制 502 通过 / 0 失败（基线 495；+6 engine +1 http）。
- 主控反例 3 项（临时追加、未提交）：篡改已存结果的 terminal → status Conflict；篡改 probe_job_id → fail-closed（Conflict/NotFound）；不同 request_key、相同输入 → 新探测作业（Cooldown），不复用他人回执，存储中恰好 2 个作业。全部通过。首轮工作区跑出的 1 失败系主控临时反例文件被并发编译所致，已在干净 head 复跑消除。
- 结论：verified（E07 curriculum.step 管理接线程序子范围；离线 profile 只产生 BudgetExhausted/Cooldown 终点，不构成 G3/学习/模型声明）。PR 已转 ready，待管理员在 #59 之后合并；merged_sha 待记。回滚点 `9dec327`。（2026-10-01：已合并，merged_sha `b220971c4e5ad44453c9d3e5787fbdfda6eb01ac`）

### CTRL-AG016-R1 / PR #63：E03 可信开发执行/评分回执与已登记纯函数运行器（主控独立验收，内容提交 `f523fb4`，最终重叠 head `b1d008f`）

- 真源：v4.2 §4/§4.1（执行器与独立 grader 分离，优化器不能签发使用证明/账单/评分）、§5.8、§6.7.3/§6.7.4（每阶段真实请求 ID/输入输出摘要/调用主体/来源闭包/费用；无执行证据不得 used_real_model）、§8.2–§8.3（已登记纯函数适配器、独立 oracle、不信布尔）、E03/E04/E05 增量、V083.a、V087.b、V090、V093、V096.a。
- 实现：`DevelopmentControlV1`（Admin 登记；executor/grader/proposer 三方不同；冻结任务与 oracle 期望答案；evidence_scope ProgramFixture|RegisteredPureFunctionExecution）；`DevelopmentExecutionReceiptV1`（仅 control.executor_actor 以 Worker|Host 签发；预算行按 episode/stage DevelopmentExecution/输入摘要/已关闭/非 Fixture 复核；cost_state 由预算行推导）；`DevelopmentGraderReceiptV1`（仅 grader_actor 以 Evaluator 签发；服务端用冻结 ExactJsonAnswerV1 重算分数）；typed envelope `e03dev-…`；撤销闭包登记（lifecycle preserve/redact）；`verified_development_observation_in_session` 重写：依赖过滤 execution|grader（修复原"精确集合相等"使真实 journal 事实必被拒的潜在缺陷）、typed 加载、Fixture/ProgramFixture ⇒ Forbidden、预算行/摘要/分数比对、tombstone ⇒ Forbidden、水位漂移 ⇒ Conflict；`RegisteredDevelopmentRunner`（clamp_i64 进程内执行，零成本已知、幂等复用回执与预算行）；E12 `record_cycle` 正向用例通过。
- 主控复跑（`ctrl-ag016-workspace.log`）：重叠到 d8bfdd5 后全工作区 48 个测试二进制 522 通过 / 0 失败；clippy 全工作区 -D warnings 0；fmt 0。
- 主控反例 3 项（`ctrl-ag016-adversarial.log`，临时、未提交）：篡改已存执行输出工件、篡改执行回执 output_digest、篡改 grader 回执分数 → 门禁全部拒绝。
- 已知边界（实施方如实披露）：无 BudgetExecutionProvenance 新变体（迁移编号由主控分配，改以 typed settlement 工件承载出处）；monitoring.rs 未收紧（其 fixture 路径不能消费真实运行器事实）；E12 课程 envelope 未入 lifecycle 列表（既有）；无隔离/沙箱运行器、无模型、无正式评估、无效果声明。
- 结论：verified（E03 开发回执 schema、门禁与已登记纯函数运行器程序子范围）。最终 head `b1d008f`（2026-10-01：已合并，merged_sha `3e2fc067190077b8cf8401875d6c627b8d9e13fc`）；重叠到 #62 之上后的全工作区复跑见下文“合并顺序”节。

### CTRL-AG017-R1 / PR #64：E07 exploration.start 管理消费者（主控独立验收，内容提交 `96f0894`，最终重叠 head `71c0fa8`）

- 真源：v4.2 §6.1/§6.2、§7.1/§7.1.1（decide 为只读前缀上的纯函数；协调器校验/预留/执行）、§7.3、§7.4.1（策略不属于世界兼容签名）、§6.7.4、E07/E09 程序范围、V019、V020、V086.c/d、V017/V056、V008。
- 实现：`ExplorationStartRequest{world: ExplorationWorldV1}`（deny_unknown_fields；SimulationContext 补 deny_unknown_fields）→ Admin → `checkpoint_claim` → `register_world_idempotent`（注册指纹只含不可变登记字段【2026-10-01 更正：此措辞在 #64 时不成立——指纹还包含 run_next 会改写的 remaining_root_micros 与 remaining_recovery_dispatches 两个计数器，派发后重放登记会 Conflict；由 AG-040（#87）去除并按登记时的值还原计数器，见第二轮节 CTRL-AG040】：同 id 同指纹 ⇒ AlreadyRegistered 无写入；不同 ⇒ Conflict；来源 tombstone ⇒ Forbidden，水位漂移 ⇒ Conflict）→ `decide_next`；结果 `ExplorationStarted{world_id, context_signature, prefix_digest, legal_actions_digest, action}`；`status` 通过 `verified_world_decision_view` 重跑纯决策并逐项比对，不改写终态；依赖边 job→private_input→各来源 run 与世界 envelope。run_next/节点派发/封存/模型均未接（无生产 DevRunner/ModelPort）。
- 实施方复跑：dispatch_management 27、exploration_v41 4、evo-http service 5、smoke OK、clippy/fmt 0、全工作区 47/511/0（其基线 3a0e26a）。主控在最终重叠 head `71c0fa8` 上复跑，见下文“合并顺序”节。
- 主控反例 2 项（`ctrl-ag017-adversarial.log`，临时、未提交）：篡改已存结果 prefix_digest → status Conflict；在协调器之外把世界 remaining_root_micros 改为 0 → status 重决策不一致而失败，持久终态仍 Succeeded。全部通过。
- 结论：verified（E07 exploration.start 管理接线程序子范围）。最终 head `71c0fa8`（2026-10-01：已合并，merged_sha `f8fa1a19d5e968f83a52385ecb43d07d56442610`）。


## 2026-09-30 主控第二轮独立验收、返修与合并（真源文件缺席，依台账记录范围验证）

前提：本轮在新机执行，`RSIAgent-v4.1定稿-工程实施方案-2026-09-19.md`（SHA-256 `45f3ba06…`）与 `out/copilot-handoff/`、F08 两份冻结合同均不在本机（旧机维修中）。用户明确决定继续。所有结论只对"台账记录的 v4.1 章节/E/V 映射、各 PR 分支主控记录、HANDOFF 复验证据、#48 台账 F08 提案文本"成立，**不构成第一真源核验**；真源恢复后须按正文复核。原始日志仅本地 `out/verify-20260930/qa/s2-*`。统一环境 `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`，Cargo 均 `--locked`；本机 crates.io 缓存缺 `crc`，以 rsproxy 镜像解析，Cargo.lock 未变。

| 任务 / PR | 主控结论 | 依据/返修 | merged_sha |
|---|---|---|---|
| AG-002 / #44 E16.1 | verified（程序/持久导入子范围），已合并 | 记录测试复现通过；主控反例 8 项失败→返修 F13–F16（相对路径绕过授权、根可为 `/`/$HOME、聚簇膨胀、tool_result 角色判定与缺 content 记录） | `acda31895bb1cb42cf7985b907a4c600429573d0` |
| AG-011 / #54 E16.2+E16.3 | verified（staging/种子/本地导出子范围，F08 按提案版对照），已合并 | 自测与 E16.1 回归复现通过；反例→返修 F17–F20（中止/失败记录保活共享 blob、纯函数重置不看状态、导出 id `.`/`..`、非规范成员路径）；F08 仅能对照提案版 | `16c2817bc192bf71535d2c99d867873e6e85bcf8` |
| AG-005 / #47 E16.4 | verified（真实 Store 拒绝门与记录重载子范围），已合并 | 源提交 b9efc76/ff17969/9d04c67 重新堆叠到 main，逐文件与已验 head 一致，复测通过 | `1c824e6614667da4dc7ea74e96b46adfd1d2c089` |
| AG-006 / #48 E16.5 | verified（容量/恢复/部署门纯函数子范围），已合并 | F07 返修不成立→主控返修 F10–F12（环境变量顶替布尔、整库读取只查头、曝光计数/撤销来源可回退）；重新堆叠到 main | `4980aa498baa80fd82e83441839e606859fa4065` |
| AG-008 / #50 E17 | verified（关闭/拒绝子范围），已合并 | 关闭/拒绝子范围重新堆叠到 main 复测 | `e94042d3f2fd761455727a6c82ad13fd3d657d03` |
| AG-009 / #51 E18 | verified（关闭/拒绝子范围），已合并 | 同上，堆叠于 #50 之上 | `bef7bd1763a01ccb677ada4acad71a063366539c` |
| CP-002 / #53 → #57 | verified（只读结构检查器子范围），已合并 | HANDOFF 4.3 六个漏检：5 个结构性驳回，第 6 个（无冲突的伪造 merged_sha）无 git 不可驳回，显式列入 needs_verification；9/22 预审 2 个反例原件缺失。#53 因 base 分支被 GitHub 自动删除而关闭，同一提交重叠到 main 后以 #57 合并 | `7b425fcf4da08b8949aaa8f3426853f8938d3f3c` |
| AG-007 / #49 → #56 E16.6 | verified（派生索引结构与合并事实映射子范围），已合并 | #49 源提交重叠到 main，索引按本轮合并事实更新（7 条主控记录、pr_state、implementation_files、状态与台账对齐）；#49 因 base 分支被自动删除而关闭，以 #56 合并；support_scope 10 通过、CP-002 检查器 structure_valid | `364a0722be7d5b05cbf01c453322ba35c2fdfa67` |
| AG-003 / #45、AG-004 / #46 | 被 #54 取代，已由用户关闭 | — | null |

### AG-011 / E16.2+E16.3 持久 staging、内置种子与本地导出交付（实施方交付记录）

- 任务号：AG-011；E 归属：E16.2 + E16.3；取代原 AG-003（#45）/ AG-004（#46）
- base 分支：`wrokbot/ag-002-e16-1-source-import`；head 分支：`wrokbot/ag-011-e16-2-staging-export`；PR：[PR #54](https://github.com/acosmi/RSIAgent/pull/54)
- 实施方声明依据：v4.1 §11.1–11.3、E16.2/E16.3，及 F08 合同 v1（`F08-CONTRACT-FROZEN.md`、`F08-LOCAL-EXPORT-FROZEN.md`）；原始交付提交 `02f40932dc7d3ed91d255240887a646c205b6e76`（基于已验 E16.1 head `7fae0fe`）。
- 文件：`crates/evo-engine/src/packages.rs`（staged_asset 生命周期 Prepared/Staged/Quarantined/Aborted/Failed、注册 blob 发布、原子 NOREPLACE、export_attempt v2 与 local_namespace 交付、交付审计、重启对账、全投影隐私扫描、撤销感知中止）、`crates/evo-engine/src/seeds.rs`（持久 seed_install 绑定 B/L/U 摘要，重置只产生新 staging）、`crates/evo-storage/src/lib.rs`（有界/注册 blob 原语、本地导出树摘要与回执）、`crates/evo-storage/src/lifecycle.rs`（E16 schema 精确清理，`cleanup_import_*` 改为 `cleanup_e16_*`）、`scripts/restore_backup.py`、`fixtures/packages/golden_manifest.json` 及对应测试。
- 实施方自测（PR 正文）：packages_v41 30、seeds_v41 9、local_export 8、lifecycle 19、import_integrity 4、import_v41 28、四 crate 406；PR 正文把 local_export/lifecycle 数量写反，以主控复跑为准。

### CTRL-E16.1-R3 / PR #44：主控第二轮复核与返修

- 真源文件缺席，依台账记录范围验证（见本节前提）。固定输入：原 head `c74369e553150521104369e71375ca9fa2b3f9f4`（= 已验 `7fae0fe` + 仅台账集成）。
- 复现记录证据：import_v41+optimization 35、evo-storage 54、evo-core evidence 10、workspace 352；storage+engine 与 workspace 的 clippy `-D warnings`、fmt 均 exit 0。日志 `s2-pr44-[0-6].log`。
- 主控反例 26 项（`s2-pr44-adversarial-e16_1.rs`）：18 通过、8 失败，归并为 4 项缺陷并由主控在原 PR 返修：
  - F13 相对路径绕过根授权：`assert_authorized_path` 只校验绝对路径，持久注册可读取并落库授权根之外的文件。返修：路径与根必须为绝对且规范（无 `.`/`..`/空分量/NUL），组件级严格前缀授权，打开文件前再次断言。
  - F14 根可为 `/`、`$HOME` 或其祖先：全 home 扫描门只查来源名不查根。返修：`assert_authorized_root`/`assert_not_whole_home_scan` 在 `SourceSelection::validate` 与持久注册统一生效。
  - F15 聚簇膨胀：嵌套 `.try1:fork2` 只剥一层；内容去重把同一事故的重试拆成独立簇。返修：后缀剥至不动点；簇为"同事故 ∪ 同内容"关系的并查集闭包。
  - F16 `tool_result_is_not_preference` 只认精确 `"user"`；`claude.fixture` 缺 `content` 的记录被当成空 user chat。返修：工具输出对任意角色拼写均非偏好；缺 `content` fail-closed。
- 返修提交 `b6861a756bf1a2a5660cb9d6e0663b4385242104`（源码：evo-core evidence.rs、evo-engine evidence.rs/import.rs、import_v41.rs）。主控复跑：import_v41 34 + optimization 7、evidence 12、import 单元 4、evo-storage 54、三 crate clippy `-D warnings`/fmt exit 0（`s2-pr44fix-ctrl-[0-5].log`）；反例 25/26，余下 1 项 unwrap 了按要求 fail-closed 的解析。
- 结论：verified（程序/持久导入子范围），已合并。通过范围同 CTRL-E16.1-R2；E16.1 完整真实迁移链仍未关闭。回滚点 `c74369e`。

### CTRL-E16.2/3-R1 / PR #54：AG-011 主控独立验收与返修

- 真源文件与 F08 冻结合同缺席；F08 只能对照 #48 台账提案文本：字段语义由 `E16Envelope`（id/request_key/source_refs/revoke_watermark）+ `StagedAssetPayload` 覆盖；偏差：无 `expires_at`；预算以 `E04BudgetRef` 引用而非记录内 allocated/consumed；状态名 Prepared/Staged/Quarantined/Aborted/Failed 且审批留在 ReleaseStore（无 ApprovedForRelease）；来源撤销后以拒绝+redaction 处理而非字面 Quarantined 迁移。冻结版是否采纳这些偏差无法判定，真源恢复后须复核。
- 复现自测与 E16.1 回归（`02f4093`）：packages 30、seeds 9、local_export 8、lifecycle 19、import_integrity 4、import_v41+opt 35、storage 70、evidence 10；四 crate `--no-fail-fast` 406/0；workspace clippy/fmt 0。`dispatch_management::evaluator_job_same_key_different_ticket_conflicts_after_restart_safe_failure` 出现 1 次偶发失败（不在 PR 文件内），复跑 4 次通过，记为已知偶发。日志 `s2-pr54-*.log`。
- 主控反例（engine 22、storage 15）：engine 19/3、storage 11/4，归并为 4 项缺陷并由主控在原 PR 返修：
  - F17 `e16_payload_has_live_blob_reference` 不看记录状态：已中止/失败的 staged 记录使共享 blob 在最后活来源撤销后仍留在磁盘，违背 PR 自述"只计活引用"。返修：按 schema 状态集判活（staged_asset prepared/staged；export_attempt prepared/completed；seed_install prepared/installed；import_source prepared/ready），摘要字段单一定义并用于 SQL 预筛。
  - F18 纯函数 `safe_reset_to_baseline` 不检查 `status`。返修：非 Installed 一律拒绝，与持久路径一致。
  - F19 存储层导出 id 接受 `.`/`..`（`identifier` 允许点号），`local_export_directory("..")` 指向命名空间根之外。返修：`validate_export_id` 在全部导出入口与清理路径生效。
  - F20 `validate_local_export_path` 依赖 `Path::components` 归一化，接受 `a/./b`、`a//b`、`a/`。返修：按原始字符串逐段校验。
  - 接受为命名差异：来源撤销后 staged 记录不迁移到字面 Quarantined，而是全部消费入口拒绝并在清理作业中 redaction。
- 返修提交 `cf0fd22b31b559b8c60587762cae51a4f6433bd4`。主控复跑：storage lifecycle+local_export+import_integrity 38、engine packages_v41+seeds_v41 42、import_v41+opt 35、workspace clippy/fmt 0（`s2-pr54fix-ctrl-[0-4].log`）；子代理四 crate 416/0；反例 engine 21/22（余 1 为上述命名差异）、storage 15/15。
- 集成：`c57397f4ffbf9bff39c32a17e67e962bff3a8d45` = cf0fd22 + merge b6861a7（无冲突，cf0fd22 未改台账，台账 = b6861a7）；整树 workspace 428/0、四 crate 425/0、clippy/fmt 0（`s2-int54-*.log`）。
- 结论：verified（staging/种子/本地导出子范围，F08 按提案版对照），已合并。通过范围：程序/持久 staging、seed、local_namespace 导出与 E16 清理子范围；不含真实第三方包、真实远端销毁、真实用户目录种子演练。#45/#46 由本 PR 取代，关闭须经用户确认。回滚点 `7fae0fe`。

### CTRL-E16.4-R3 / PR #47：重新堆叠到 main 与复核

- 源提交 `b9efc76`/`ff17969`/`9d04c67` 以 cherry-pick 重新堆叠到 main `4dea9ef`（跳过仅台账提交 `f6f7338`）= `ade7276`；hosts.rs、release_store.rs、hosts_v41.rs、claude_code_surface.v1.json 与已验 head `f6f7338` 逐字节一致。
- 复跑：hosts_v41 6 + release_store 7、lib hosts 2、lib release_store 4、evo-engine 145；clippy/fmt 0（`s2-restack47-[0-5].log`）。
- 反例 9 项：6 通过、3 项记录：(1) `verify_host_receipt` 在本仓库没有任何接受路径（ProgramFixture 证据不能激活发布），执行回执门 (b)(c)(d) 只能代码检视，属 fail-closed；(2)(3) 注册门做精确字符串比较，`Unverified-` 与 `claude-code-mcp-tool-only:v1` 可绕过。返修 F21：宿主 id 与版本小写后按家族判定，`11b42d2`，回归测试覆盖 4 种改写；复跑 hosts_v41 7 + release_store 7、lib 2，clippy/fmt 0（`s2-restack47-final-*.log`）。
- 分支已强推为 `11b42d2`，base 改为 main。结论：verified（真实 Store 拒绝门与记录重载子范围），已合并；E16.4 整项仍 blocked（无真实 Claude Code 证据）。回滚点 `f6f7338`。

### CTRL-E16.5-R2 / PR #48：F07 返修复审、主控返修与重新堆叠

- 复审 `76f94e5` 的 F07 返修不成立：F10 `probe_sandbox_capability` 以环境变量 `RSIA_SANDBOX_CAPABILITY_VERIFIED` 放行，等同被退回的布尔开关；F11 锚校验整库读入内存只查 16 字节头，且跟随符号链接；F12 `verify_recovery_state` 允许 `total_exposure_count` 回退、允许备份丢失已撤销来源。
- 主控返修 `ad8104a`：部署门改为依赖 E04 `IsolationPolicy` 事实（参考宿主永不放行代码执行）；锚必须为常规文件（拒绝符号链接/目录）且只读 16 字节头；曝光计数与已撤销来源集合不可回退。复跑 capacity_v41 21、lib 3，clippy/fmt 0（`s2-pr48-ctrl-*.log`）。
- 重新堆叠到 main：`5eed6dc`/`7ce8387`/`5ed6088`（capacity 文件与 ad8104a 逐字节一致；capacity_v41 21、evo-engine 161、clippy/fmt 0，`s2-restack48-*.log`）。结论：verified（容量/恢复/部署门纯函数子范围），已合并；容量/恢复门仍未接入 dispatch/prepare 真实入口，E16.5 整项仍 blocked。回滚点 `76f94e5`。

### CTRL-E17/E18-R2 / PR #50、#51：重新堆叠到 main 与拒绝门硬化

- #50 源提交 `5d89154`、#51 源提交 `86064b5` 依次堆叠到 main（跳过仅台账提交）：`0b5b4e6`、`c1c0759`；features.rs 与各自测试与已验 head 逐字节一致。复跑 e17 4 + e18 12、lib features 3、evo-core 138；clippy/fmt 0（`s2-restack5051-*.log`）。
- 反例 26 项：21 通过、5 项为默认关闭标志掩盖下的门内漏洞：E17 门做大小写敏感精确比较、自评只信自报布尔；E18 受保护路径漏 Dockerfile 变体/CI/cargo 配置/首尾空白，空审批 token 过门，空文件列表无门。主控硬化 F22/F23：#50 head `4e0cabe`（0b5b4e6 + F22）、#51 head `2d909c4`（+ 112a92b 重堆叠 86064b5 + F23）；e17 7 + e18 18、lib 3、evo-core 147、clippy/fmt 0、反例 26/26（`s2-e1718fix-*.log`）。
- 结论：verified（关闭/拒绝子范围），已合并/verified（关闭/拒绝子范围），已合并；E17/E18 保持默认关闭，不代表真实扩展运行通过。

### CTRL-CP002-R3 / PR #53：检查器漏检返修

- HANDOFF 4.3 六个漏检在 `acfdc79` 全部复现放行（`s2-pr53-repro-before.txt`）。返修 `56f8644`：planned 不得携带 verified 证据；父范围不得强于最弱子包；optional_disabled 不得 verified；证据须以本任务命名空间登记、不得复用他任务 log/测试运行、目标须落在本任务 crate；各对象封闭键集；merged_sha 跨记录一致（一 PR 一合并、源/合并不互指、可选 `pr_state` 一致）。与清单内任何事实都不冲突的伪造 merged_sha 无 git 不可驳回，显式列入 needs_verification。
- 主控复跑：40 单测、真实清单 structure_valid=true、6 反例 5 驳回（`s2-pr53-ctrl.log`）。9/22 预审的 2 个反例原件在旧机，未复核。结论：返修完成、未验收合并；base 仍为 #49 分支，等 E16 链落定。

合并（用户在本机终端按主控固定的 head 执行，主控本机审查器拒绝合并动作）：#44 `acda318`（树 = b6861a7）、#54 `16c2817`（树 = c57397f）、#47 `1c824e6`、#48 `4980aa4`、#50 `e94042d`、#51 `bef7bd1`；主控逐一核对每个合并提交的非台账源码与已验 head 逐文件一致（`s2-controller-notes.md`）。合并后 main `bef7bd1763a01ccb677ada4acad71a063366539c` 收口：workspace 47 个测试二进制 486 通过 / 0 失败，clippy `-D warnings`、fmt、build 均 exit 0，定向 E16.x/E17/E18 套件 104 + 26 通过（`s2-main-*.log`）。发现 `scripts/smoke_cli.py` 在 main 上确定性失败（HTTP 400）：自 AG-001 `f9500d9`（#43，2026-09-19）起 `replay.run` 改为完整 `ReplayRunRequest`，E07 期的 smoke 载荷与"blocked"断言已过时；与本轮合并无关，本轮未修改，列为待办（更新 smoke 或裁定 E07 smoke 作废）。

收尾（2026-09-30 末）：台账 PR #55 merged_sha `243c6a5e44854c28c01b6911ea8ad5fe4585ae35`；#56（E16.6 索引，取代 #49）merged_sha `364a0722be7d5b05cbf01c453322ba35c2fdfa67`；#57（CP-002 检查器，取代 #53）merged_sha `7b425fcf4da08b8949aaa8f3426853f8938d3f3c`；main `7b425fcf4da08b8949aaa8f3426853f8938d3f3c` 上 support_scope 10 通过、检查器 structure_valid、40 单测通过。#49/#53 均因 GitHub 在合并后自动删除 base 分支而被关闭，替代 PR 使用同一提交内容（脚本/索引逐字节一致）。已合并与已关闭 PR 的远端/本地分支全部删除；本地只剩主工作树。

## 2026-09-19 主控独立验收与合并结论（历史，已被 2026-09-30 第二轮取代）

本批未全量通过。Antigravity提交的原测试由主控在冻结副本重跑：380 passed，Clippy/fmt exit0；主控补充9个反例，9个均实际失败。完整源码输入为 `c78e05c2f9518aa73cee3d0b50671f3042e2b3fb`，日志和反例仅本地 `out/review-antigravity-20260919/`。自测数量不能代替实际消费者、安全边界和完整场景验收。

| 任务 / PR | 主控结论 | 依据/未完成项 | merged_sha |
|---|---|---|---|
| AG-001 / [#43](https://github.com/acosmi/RSIAgent/pull/43) | verified（回放管理子范围），已合并 | 对精确head `0aef9345679e674988e67e4208c60aba63b5271b` 独立26项管理/回放＋3项真实HTTP、clippy/fmt通过；包含持久报告、幂等恢复、身份和撤销读取门 | `59afd1663a0d5f5212077dd92beaaf7920cde0ff` |
| AG-002 / [#44](https://github.com/acosmi/RSIAgent/pull/44) | implemented_not_verified，退回 | 未知格式/版本被猜测接收；UTF-8探测及默认事件截断panic；新locator无法回读未变源。导入持久学习/撤销消费者亦未完成 | null |
| AG-003 / [#45](https://github.com/acosmi/RSIAgent/pull/45) | implemented_not_verified，退回 | manifest元数据可绕过隐私扫描；staging/导出及撤销持久消费者未接通 | null |
| AG-004 / [#46](https://github.com/acosmi/RSIAgent/pull/46) | implemented_not_verified，退回 | 过旧当前撤销水位仍允许重置；StagingSession仅内存bool，无持久中断恢复链 | null |
| AG-005 / [#47](https://github.com/acosmi/RSIAgent/pull/47) | implemented_not_verified，退回 | Offered＋自报VerifiedBenefit可通过回执gate；未消费可信使用/评测闭包；实际额外宿主仍blocked | null |
| AG-006 / [#48](https://github.com/acosmi/RSIAgent/pull/48) | implemented_not_verified，退回 | sandbox布尔＋不存在的锚库路径通过安全校验；容量/恢复接口未接实际运行入口 | null |
| AG-007 / [#49](https://github.com/acosmi/RSIAgent/pull/49) | implemented_not_verified，退回 | 编号/数组长度不等于完整作用域证据映射，SO已核验声明缺实际commit/path/blob/许可落点支持 | null |
| AG-008 / [#50](https://github.com/acosmi/RSIAgent/pull/50) | 关闭/拒绝子范围已复验，合并blocked | E17仍关闭；依赖栈含未通过PR，不带入main，不代表真实评分器演化通过 | null |
| AG-009 / [#51](https://github.com/acosmi/RSIAgent/pull/51) | 关闭/拒绝子范围已复验，合并blocked | E18仍关闭；依赖栈含未通过PR，不带入main，不接受bool/token作为未来真实隔离或授权证据 | null |

原E00–E13的14个已验PR源码再次核对无变化，并与AG-001一起按依赖顺序合并（#29–#43）。台账冲突仅在隔离工作树解决，每次核查非台账源码与已验head相同，并以expected_head_sha固定合并。合并后的代码main `59afd1663a0d5f5212077dd92beaaf7920cde0ff` 与独立测试PR #43的非台账源码逐文件一致。未通过的新代码未进入main，未派发Actions/付费调用/发布。

详细本地结论 `REVIEW.md`、返修交接 `RETURN-TO-ANTIGRAVITY.md`、反例 `controller_adversarial.rs` 和实际合并列表 `verified-merges.json` 均位于本轮review目录。修复后需按新head再次验收。下文早期draft/未合并/null及实施方覆盖声明是当时记录；当前结论以本节及实际merged_sha为准，不能把历史自报作为当前验收。

## 当前基线与保护

| 字段 | 当前事实 |
|---|---|
| 核对时间 | 2026-09-19 UTC |
| 原始固定 source_sha | `9d4ef199c64275a0d4025cfa410f6475f635b0cd` |
| 本轮代码合并后 main | `59afd1663a0d5f5212077dd92beaaf7920cde0ff`；2026-09-19 19:46 UTC已fetch并逐文件核对；后续仅台账提交另记 |
| 初始分支 | `implementation/v4-e01-sequential-reject-holdout-20260918` |
| 初始未提交文件 | `evaluation.rs` 已修改；`holdout.rs`、`sequential.rs` 未跟踪；已逐文件和 diff 保存本地快照 |
| 既有 stash | `wip-host-cli-http-mcp: uncommitted at v4 E00 start`；已只读备份，未 apply/drop |
| 固定输入 | 从 source_sha 导出独立基线副本后运行门禁，工作树新改动不冒充基线测试输入 |
| 迁移 | 当前 main 已有 0001/0002/0004/0005/0006；0003_v3_assets.sql 为历史占用，不复用；完整字节摘要见第四轮折入的 GA-1 基线清单；新编号仍由主控统一分配 |
| v4 归档 | 已移动至本地 `archive/2026-09-19-pre-v4.1/`，前后 SHA-256 均 `d26ab3506736849f3ec1d286b49fcfa581a09c8be2243681fcc8e93b758f9d6d` |
| 旧台账 | 原字节另存本地归档；历史远程版本可由 source_sha 查阅 |
| 协作索引 | 当前仓库未发现 AGENTS.md 或独立协作索引；本轮按用户明确规则与当前真源执行，本地建立派发/合同索引 |
| 保护范围 | 不修改前端、只读参考仓；不改变架构/阶段/默认启用范围；禁止 cargo xtask ci |

## 单任务 PR 顺序

每个任务独立审阅；下表保留已交付E任务PR，后续增量用独立任务号并标明E归属。实施方先自测提PR，主控最终验收后合并。以下同时登记任务顺序与实际 GitHub PR；采用依赖栈，每个 PR 只审阅本任务差异。当前实际合并状态见下表；未通过或受依赖阻塞的后续PR仍保持未合并。

| 顺序 | 任务 | 范围与依赖 | 负责人/修改白名单 | 状态 | PR / source_sha / merged_sha |
|---|---|---|---|---|---|
| 01 | E00 | 固定基线、真源切换、历史/当前证据范围；无依赖 | 子代理复验；主控已核对源码与原始日志并定向重测 | verified（固定基线与治理脚本局部） | [PR #29](https://github.com/acosmi/RSIAgent/pull/29) 已合并；源码 `ef8244d` 已推送；merged_sha `28f892c3d68591d201a8029345257f051fb123ef` |
| 02 | E01 | 统计与独立留出静态合同；依赖 E00 核心基线 | sol/high实施，主控独立验收 | verified（静态合同）/ blocked（真实小试） | [PR #30](https://github.com/acosmi/RSIAgent/pull/30) 已合并；源码 `c5ac47d` 已推送；merged_sha `faec991036f03f27cf6d2ad3db86f841815fc828` |
| 03 | E02 | 有界原子 Skill 编辑纯编译作用域；依赖 E01 合同验收 | sol/high实施，主控独立验收 | verified（编译子范围） | [PR #31](https://github.com/acosmi/RSIAgent/pull/31) 已合并；源码 `79af6c5` 已推送；merged_sha `dfc861ce7572f8f9f49a8aaf9f0cc1687cba57f9` |
| 04 | E03 | 真实应用诊断、小批反思、建议来源与开发消费者；依赖 E02 | sol；core/engine optimization、model、evidence及定向测试 | verified（程序消费者/恢复子范围） | [PR #32](https://github.com/acosmi/RSIAgent/pull/32) 已合并；源码 `727e9cc` 已推送；merged_sha `9b13237af884c525652813ea508eb8c5f100d6f8` |
| 05 | E04 | 持久根预算、broker、取消/对账；依赖 E02 | sol/high；storage budget、0005_root_budget、engine broker/executor及定向测试 | verified（根预算/broker子范围） | [PR #33](https://github.com/acosmi/RSIAgent/pull/33) 已合并；源码 `f02e535` 已推送；merged_sha `c1cf6a2e91253218033b5883a11baa02c85a5d43` |
| 06 | E05 | 独立评测/留出、连续前缀与早停证书；依赖 E01/E03/E04 | sol/high；engine evaluator/streaming_evaluator及定向测试 | verified（持久评测程序子范围）/ blocked（真实评测） | [PR #34](https://github.com/acosmi/RSIAgent/pull/34) 已合并；源码 `86e53b9` 已推送；merged_sha `63824fff2e8d0cd0767d31140d064972542f6b53` |
| 07 | E06 | 组合发布门禁、CAS、实际应用快照与实时撤销；依赖 E02/E05 | sol/high实施；主控代码复读、修复回退与读取旁路后隔离复验 | verified（持久门禁/拒绝路径）/ blocked（真实发布） | [PR #35](https://github.com/acosmi/RSIAgent/pull/35) 已合并；源码 `071c54d` 已推送；merged_sha `c017e6a32da3cdf692c3321dd29d6510edada0c4` |
| 08 | E07 | HostService、HTTP/MCP/CLI与参考宿主；依赖E03–E06 | sol/high实施；主控独立代码与实际进程验收 | verified（本地协议/E05管理子范围）/ blocked（真实G1） | [PR #36](https://github.com/acosmi/RSIAgent/pull/36) 已合并；源码 `0ed5ec9` 已推送；merged_sha `794cf0f517a57e48166ef2ad44cb5762f3a61d5e` |
| 09 | E08 | 持久撤销、内容清理、备份与可信锚恢复；依赖E06 | sol实施、Astra/low恢复修复；主控独立验收 | verified（当前对象/本机离线恢复） | [PR #37](https://github.com/acosmi/RSIAgent/pull/37) 已合并；源码 `644d95d` 已推送；merged_sha `b0662ca1b7a2fe27d4a4622918679eed4934e214` |
| 10 | E09 | 受限策略/持久探索/E03真实消费者；依赖E03/E04/E07 | sol/high实施，主控独立验收 | verified（程序协调子范围）/ blocked（真实G2及恢复场景） | [PR #38](https://github.com/acosmi/RSIAgent/pull/38) 已合并；源码 `c71db74` 已推送；merged_sha `7a8a35908a2f7cc72ed601cbc902a2e4b49120bf` |
| 11 | E10 | 不可变观察/世界池/纯回放/持久报告；依赖E08/E09 | sol/high实施，主控独立验收 | verified（程序回放子范围） | [PR #39](https://github.com/acosmi/RSIAgent/pull/39) 已合并；源码 `77177cd` 已推送；merged_sha `96e62412809610a72d2ab593aaca59a8bcebd6d2` |
| 12 | E11 | 经济实验合同/全成本/持久阻塞报告；依赖E05/E07/E10 | sol/high实施，主控独立验收 | verified（静态与持久准备）/ blocked（真实经济实验） | [PR #40](https://github.com/acosmi/RSIAgent/pull/40) 已合并；源码 `289d920` 已推送；merged_sha `09b3ebe8bad12d59bd3c2f926d3050802372e96f` |
| 13 | E12 | 零预算课程/固定纯函数/可信事实拒绝门；依赖E04/E07/E08/E09 | sol/high实施，主控独立验收 | verified（离线子范围）/ blocked（真实学习闭环） | [PR #41](https://github.com/acosmi/RSIAgent/pull/41) 已合并；源码 `e612fcb` 已推送；merged_sha `b1127a228c4a40248631ed417aba0ad7ea1a0884` |
| 14 | E13 | 持久监测/巩固触发/根预算与恢复；依赖E07/E08 | sol/high实施，主控独立验收 | verified（程序监测子范围）/ blocked（长期实测） | [PR #42](https://github.com/acosmi/RSIAgent/pull/42) 已合并；源码 `2c79608` 已推送；merged_sha `2e209dcea36d63610c59d3797e039f35d063ec89` |

后续依真源依赖图按 E14、E15、E16.1–E16.6、E17、E18 分任务交付；E16 为六子包总门禁，不能用总勾选隐去未完成子包。

## 当前 v4.2 主任务状态

状态：planned / in_progress / blocked / implemented_not_verified / verified / explicitly_out_of_scope。verified 只对明确作用域成立，不等于效果 improved 或可发行。

| 编号 | 范围 | 状态 | 未完成项/边界 |
|---|---|---|---|
| E00 | 归并真实源码与可重建输入 | implemented_not_verified（索引同口径；核心固定基线子范围 verified，历史归并 blocked） | 原始112测试与门禁均复验通过；脚本已完成10项主控正负验证；源码或fixture变更后旧日志不能用于新输入；历史缺包不隐藏。2026-09-30/10-01：`smoke_cli.py` 过时使 CI 门失败，AG-013 已修并合并（#59，merged_sha `69a3b3ed04377c30ee7193c0679a3747f1606a1e`）；派生索引/检查器绑定 v4.2（AG-012，#60，merged_sha `dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b`）；裸 `--data` 默认路径无法加锁启动已修（AG-023，#68，merged_sha `3ffe1e26a908d4c9240320d52fcba1e92dc383d4`）。历史包与 T001–T126 归并仍为外部阻塞。  当前基线为 8043026，第三轮完整工作区 1124 通过/0失败（86 个结果行，含 doc-test，不等同二进制数）；GA-1 测试与迁移清单见本轮节。|
| E01 | 先冻结实验、任务分区和预算可行性 | blocked（索引同口径；静态合同子范围已 verified） | 本地bfbed84；显式n/统计前提、alpha/留出/比较/完整费用合同已验；开发小试/正式样本与付费授权仍缺。 |
| E02 | 最小版本化契约与宿主能力边界 | in_progress | 纯编译作用域：有界原子编辑；运行/宿主/其他新增契约尚待后续消费者。 |
| E03 | 把跨任务证据真正接入生成消费者 | in_progress（消费者/恢复已verified；开发回执子范围已verified并合并） | 主控144项core/engine测试、fmt/clippy通过；实际ModelPort请求、同清单开发选择及持久恢复已验。2026-09-30 AG-016（#63，merged_sha `3e2fc067190077b8cf8401875d6c627b8d9e13fc`）：DevelopmentControl/执行回执/grader 回执 typed schema、已登记纯函数运行器与真实观察门禁。E03 门验证的是回执链自洽，不复算已登记纯函数的输出，也未把执行方身份绑定到登记的执行器（AG-033 验收观察）。真实提供商/样本/收益、隔离运行器未验。 |
| E04 | 可信执行、隔离与根资源预算 | in_progress（根预算/broker已verified） | 主控预算15（含10001调用）、broker9、executor6项及fmt/clippy通过；group完整分页停止修复已追加PR #33；真实提供商、进程隔离尚未验。 |
| E05 | 独立验收器与有边界的统计判定 | blocked（索引同口径；持久控制/早停子范围已 verified） | 主控12流式+6旧评测+13core共31项及fmt/clippy通过；晚到回执、输出不可变、Exposure时间/终态/未知费用保留已修；真实提供商、进程隔离、完整成本及逐依赖撤销仍未验，fixture禁止晋级。 |
| E06 | 组合发布、实际应用与最小撤销闭环 | in_progress（持久门禁已verified） | 主控7集成+4单元+12 E05回归及fmt/clippy通过；审批防回退、Host完整报告闭包、所有快照读入口已验；真实生产批准/组合应用/回滚受E05证据阻塞。 |
| E07 | 第一个最小可验证真实闭环 | in_progress（协议与管理消费者子范围已verified）/ blocked（真实闭环） | 主控协议21项与管理增量27项、真实HTTP/MCP/CLI、参考宿主及fmt/clippy/build通过；E05注册/票据管理已接线。replay.run已由AG-001接通并验收；AG-015/AG-017 接通 curriculum.step 与 exploration.start（幂等、读侧重验；#61 merged_sha `b220971c4e5ad44453c9d3e5787fbdfda6eb01ac`、#64 merged_sha `f8fa1a19d5e968f83a52385ecb43d07d56442610`）；第二轮 AG-027（#74，merged_sha `c72aae14c9f35c1bcc090b5a374d2696d57dacfc`）管理作业与私有输入清理闭包及提交时拒绝已撤销依赖、AG-044（#89，merged_sha `cc85b5c71cec06763909fb0efa33c4e296b6178d`）提交时上游闭包闸门。meta.start 保持 blocked 直至 E14.2c；grant/package/seed 的来源与上游闭包写入闸门已由 AG-046/#96 验收；manifest dependency_refs 的首写前闸门仍待 AG-059；真实模型、独立数据与支付授权未取得。 |
| E08 | 可恢复的撤销、保留和备份链 | in_progress（当前对象/本机恢复与第二轮撤销闭包子范围已verified） | 主控47项及clippy/fmt通过；当前可信SQLite锚由操作者指定，不声称辨别假冒旧库。第二轮（均已合并）：清理只在闭包不动点完成（AG-035 #83）、同 id 第二撤销源点名拒绝（AG-036 #82）、已撤销来源不预留不派发（AG-038 #85）、经济回放记录保留（AG-037 #86）、撤销后到达的响应与 stage fact 不留明文（AG-043 #88、AG-045 #90），以及探索、课程、管理与回放对象接入清理闭包（AG-024/025/027/032）。第三轮 AG-051/#95 已补经济记录恢复保护；AG-046/#96 已补 grant/package/seed 来源与上游闭包首写前闸门及读侧闸。未完成：manifest dependency_refs 首写前检查（AG-059）、其他迟到写入与 stage_bundle；tombstone 键带 kind/发布期 kind 精确性与物理擦除无现行明确条款，待用户修订，不再误引 §11.1；Linux/生产演练另验。 |
| E09 | 生成/探索解耦与有状态在线探索 | in_progress（程序协调与第二轮多项子范围已verified） | 主控25项及clippy/fmt通过；完整输入幂等、两节点StoreJournal链、源水位、整批资源已验。第二轮（均已合并）：技能组作业（AG-021 #69）、同题对比实践（AG-026 #73）、探索记录清理边（AG-024 #71）、run_next 可信观察门（AG-033 #80）、合法动作与前缀卫生（AG-039 #84）、登记指纹与计数器（AG-040 #87）、付费后收敛（AG-041 #91）、恢复计数派生（AG-042 #92）。第三轮 AG-048/#97 已验封闭终态类和白名单内固定码（模型回答日志与 broker 账本不在“不落原文”声明内）。未完成：可修复故障的类型化来源与 episode 绑定（AG-047）、优化历史进入请求与签名（PR-C）；真实 G2 未取得；V097（§3.1.1 消融）作用域未验。 |
| E10 | 不可变世界池与纯查表回放 | in_progress（程序回放/池/报告已verified） | 主控57项、clippy/fmt通过；观察正文绑定实际共同输入和来源，q0/辅助来源篡改拒绝；世界/池/报告持久与实时撤销已验。replay.run管理适配经AG-001验收并合并；AG-032（#79，merged_sha `b262bbb2a0ea9ead68bc33fe1e58235e85bcffea`）：回放读路径在撤销清理的每个中间状态返回点名 Conflict。真实观测仍缺，不声称经济收益。 |
| E11 | 验证回放优化的真实经济收益 | in_progress（合同/持久准备已verified） | 主控10项及clippy/fmt通过；单票配对、九类成本、实际预算绑定/最终回执不可变、并发取消/晚到账/报告CAS已验。可信在线配对回执消费者尚未实现；真实经济实验未运行，不声称节省。 |
| E12 | 学习者条件化的经验自主获取 | in_progress（离线子范围已verified） | 主控21项和参考宿主3个进程用例、clippy/fmt通过；控制注册、精确平台期、冷却/零预算终态、事实拒绝门已验。E03 回执 schema 已由 AG-016（#63）补齐；第二轮 AG-025（#72，merged_sha `15b140ddef25c8e794d586fecae56f129226a5c6`）课程信封清理分类、AG-027（#74）课程重放与脱敏读取 fail-closed。AG-050/#94 已验文本提案默认 unverified，以及旧内存课程 step 先选题再预留并保留 typed 错误。真实隔离（§12.0：E12 代码级范围依赖 E04 真实隔离验收）、应用正例及持久学习改变下轮选题仍未验，不启用G3。 |
| E13 | 长期部署适应与能力保留监测 | in_progress（程序监测与第二轮巩固子范围已verified） | 主控22项及clippy/fmt通过；两周期触发、单claim、真实根绑定前置校验、异常/撤销持久终态和漂移已验。第二轮（均已合并）：巩固撤销闭包、纯 pass/fail 配对与单候选暂存（AG-022 #70）、E03 门控可信周期触发（AG-028 #75）、巩固单独计量（AG-030 #77，V096.a）、终态类别有界（AG-031 #78）。第三轮 AG-048/#97 已将本卡白名单内巩固终态改为固定类型码，日志中的模型回答仍随来源闭包清理。生产周期驱动与管理入口未做（§6.1 无巩固操作，须先修订真源）；真实提供商、连续轮次保留/长期效果仍未取得。 |
| E14 | 受限改进器自身的继承控制器 | in_progress（增量 1 与 E14.2a 程序子范围已verified并合并） | AG-019（#66，merged_sha `5a0ef4419c51687534baa3296dbbf7e310a5d810`）：有界 ElasticPolicy、决策携带 policy/caps 摘要、ImproverContentV2 只开放 exploration_policy、MechanismUsageRecordV1 只由真实派发派生；AG-034（#81，merged_sha `d9b3f6f5234ad14da18956a78c804e4c6818fa10`）：MetaTrial 分叉（同一 S0 的两条流只差 id 与 policy，按 billing scope 如实读出实耗）。未完成：ProgramFixture 范围的改进器登记（E14.2b）、meta.start 登记与绑定型消费者（E14.2c）；真批准在结构上不可达；E14 机制冻结合同为外部阻塞；不声称继承或元收益，不启用G4。 |
| E15 | 后继质量与跨代收益实验 | planned | 真实后继实验未运行。 |
| E16 | 产品支持范围与最终交付门禁 | in_progress（E16.1–E16.5 子范围已验并合并） | E16.6 索引已按合并事实更新（#56）并由 AG-012 绑定 v4.2；真实宿主/沙箱/第三方包证据未取得；已按真源正文复核（2026-09-30，见顶部审计节）。2026-10-01：E16.5 容量门接线（AG-014 #62）、启动恢复/部署门（AG-018 #67）与恢复覆盖（AG-029 #76）已合并；索引登记第二轮 31 条验证记录（AG-049）。 |
| E16.1 | 来源导入与版本化读取器 | in_progress（程序/持久导入子范围已verified并合并） | PR #44 merged_sha `acda31895bb1cb42cf7985b907a4c600429573d0`（主控返修 F13–F16 后）；完整真实迁移链未关闭。 |
| E16.2 | 资产导入／分享与隐私门禁 | in_progress（staging/本地导出子范围已verified并合并） | PR #54 取代 #45，merged_sha `16c2817bc192bf71535d2c99d867873e6e85bcf8`（主控返修 F17–F20 后）；F08 只对照提案版；真实第三方包/远端销毁未验。 |
| E16.3 | 内置种子与本地修改保护 | in_progress（持久种子安装/重置 staging 子范围已verified并合并） | PR #54 取代 #46，merged_sha `16c2817bc192bf71535d2c99d867873e6e85bcf8`；真实用户目录演练未验。 |
| E16.4 | 额外真实宿主与配置面漂移 | in_progress（拒绝门/记录重载子范围已verified并合并）/ blocked（真实宿主） | PR #47 重新堆叠后 merged_sha `1c824e6614667da4dc7ea74e96b46adfd1d2c089`（含主控 F21）；无真实 Claude Code 证据，`verify_host_receipt` 在本仓库无接受路径（fail-closed）。 |
| E16.5 | 持久恢复、容量、依赖与部署安全 | in_progress（门函数、容量门接线、启动恢复/部署门与恢复覆盖子范围已verified并合并）/ blocked（真实沙箱） | PR #48 merged_sha `4980aa498baa80fd82e83441839e606859fa4065`（F10–F12）；AG-014（#62，merged_sha `9c3ec979fd2ee3c53dfbf4ddf96967e94808b11e`）：容量门接入六个真实入口并实例级跨命名空间计数（F24）；AG-018（#67，merged_sha `5c4f81dca79373c6871449aed287f61fb1f3cbc2`）：启动恢复隔离门与部署门接入 rsia serve/mcp；AG-029（#76，merged_sha `7120842a1f586ae394b76e146c8298f11bec52bc`）：本轮新对象的恢复覆盖。AG-051/#95 以精确 schema 增补经济实验/作业/成本回执/报告恢复保护。未完成：真实沙箱（外部）、Linux 实测、启动时驱动未完成的撤销清理与残留 dispatched 调用、出站一致性；本轮新增内部对象的容量上限须先修订 §3.3.1。 |
| E16.6 | 发行、证据台账与唯一真源交接 | in_progress（派生索引与检查器子范围已verified并合并） | #56 merged_sha `364a0722be7d5b05cbf01c453322ba35c2fdfa67`、#57 merged_sha `7b425fcf4da08b8949aaa8f3426853f8938d3f3c`；AG-012（#60，merged_sha `dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b`）绑定 v4.2 与谱系；2026-10-01 登记第二轮 31 条验证记录（AG-049），AG-060 补登记第三轮四条已验子范围；索引仍声明 subset_only，全路线未完成；已按真源正文复核（2026-09-30，见顶部审计节）。 |
| E17 | 可选：开发代理评分器演化 | in_progress（关闭/拒绝子范围已verified并合并） | PR #50 重新堆叠后 merged_sha `e94042d3f2fd761455727a6c82ad13fd3d657d03`（主控硬化 F22）；默认关闭，不代表真实扩展运行通过。 |
| E18 | 可选：自动提出代码修改，不自动部署 | in_progress（关闭/拒绝子范围已verified并合并） | PR #51 重新堆叠后 merged_sha `bef7bd1763a01ccb677ada4acad71a063366539c`（主控硬化 F23）；默认关闭，不代表真实扩展运行通过。 |

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
- Antigravity按任务自测、推送和提PR后连续推进；Codex独立验收通过后负责最终合并。Actions派发、付费运行和部署/发布仍未授权。

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

此前用户要求的E07管理增量及E10–E13在途子范围已收口。现按最新指示切换为Antigravity持续实施、自测及逐任务PR，Codex负责规范裁决、合同冻结、最终独立验收和合并；不再限制首包后停止。后续按真源依赖和冻结合同推进，E14合同建议不自动视为定版，默认关闭范围不变。方案/合同/原始QA仍仅本地。

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

E07管理增量、E10–E13已按上述子范围验收、提交、推送并分别归入原任务PR。上一轮收口时E00–E13共14个独立草稿PR；现已在本轮主控阶段按表合并，实际merged_sha见当前记录。25个台账节点中14个已有已验子范围交付，属于56%的任务覆盖率，不能写成后端完成56%；本轮未宣称任一研究阶段的真实全链全部完成。

主控最终在隔离副本对整合源码 `c9b85b88d11190bbc83d6361904a91c3ede63a4c` 运行 `cargo test --locked --offline -p evo-core -p evo-storage -p evo-engine`，301项通过；对应all-targets Clippy -D warnings与fmt exit0。实际输入/命令/日志哈希保存在本地 `controller-closure-integrated-acceptance.json`。Cargo.lock仅构建清理引起reqwest/rmcp列表排序差异，依赖/版本/checksum逐项相同；仓库锁文件未改。

剩余工程：E07探索/回放/课程/meta管理handler；E03可信开发执行/评分回执schema；E05真实独立执行/完整成本与逐依赖闭包；E09实际恢复/多组/实践消费者；E11完整在线配对回执消费者；E12实际学习改变下轮选题；E13真实长期保留链；E14–E18及E16六子包的本轮完整验收。外部条件另列：真实模型/样本与支付授权、合格隔离环境、历史原包/T映射。不能把尚未实现的工程全部归为外部阻塞。

下一轮由Antigravity在隔离工作树持续实施，每项自测后提交、推送、独立草稿PR和台账，立即继续下一项；Codex负责第一真源裁决、合同冻结、代码/测试/实际结果最终验收与合并。既有PR保留已验子范围；后续增量独立编号并标明E归属，同一任务返修追加原PR。方案、内部交接和QA仅本地，本台账允许远程。

### 用户最新授权：逐任务PR、连续实施、主控最终验收合并

实施方Antigravity完成一项任务的编码、自测、独立PR和台账后立即进行下一项，不等待上一PR验收或合并。禁止实施方合并/auto-merge。主控Codex已获授权，在核对实际PR head、代码、定向测试及真实结果通过后，按依赖顺序最终合并并记录merged_sha；合并授权不等于发布、Actions或付费实验授权。

每项先登记任务号、E归属、范围、文件白名单、依赖和验收条件。未冻结合同或外部条件准确登记，只阻塞依赖项，其他工作继续。实施方自测完成记implemented_not_verified，不能自报主控verified。现有14个PR的未合并状态不因本次授权而自动改变。

AG-001归属E10，仅接通既有replay.run管理消费者；从最新交接基线建立独立增量PR，以E13交付分支为base。这样不把已有E11–E13代码混入旧E10 PR #39；PR通过前不合并，提PR后连续处理下一任务。

**以下AG实施/自测文字为实施方交付记录；其中“完整实现/全面覆盖/端到端”不构成主控通过结论，当前验收与返修以页首为准。**

### AG-001 / E10 现有 replay.run 管理消费者实施与自测

- 任务号：AG-001
- E 归属：E10
- 状态：`verified`（仅本任务回放管理子范围，经主控独立验收）
- base 分支：`wrokbot/v4.1-pr-e13-monitoring`
- head 分支：`wrokbot/ag-001-e10-replay-management`
- PR：[PR #43](https://github.com/acosmi/RSIAgent/pull/43)
- merged_sha: `59afd1663a0d5f5212077dd92beaaf7920cde0ff`

依据与合同：严格依循 v4.1 第一真源 SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150` 与 `AG-001-E10-replay-management.md` 冻结最小合同。
文件白名单修改：
1. `crates/evo-storage/src/replay.rs`: 仅将既有 `replay_pool_storage_id` 暴露为 `pub` 只读 helper，供 dispatch 构造依赖边复用，未修改任何内部存储语义。
2. `crates/evo-engine/src/dispatch.rs`: 新增 `ManagementResult::ReplayStored { report_id, pool_digest, semantic_reports_digest }`、`ReplayRunRequest`、`ParsedRequest::ReplayRun`；在 `validate_request_authority` 限制仅 Admin 启动；在 `private_dependencies` 追加 `job -> private_input -> pool` 依赖边；通过 `checkpoint_claim` 校验租约与取消后调用既有 `run_and_persist_pool_replay`；在 `status` 针对成功结果调用 `verified_replay_report_view` 严加核验（若来源撤销、缺池/报告、水位变更则拒绝返回成功结果，历史取消/失败状态不因缺失产物复活）。
3. `crates/evo-engine/tests/dispatch_management.rs`: 将原有 blocked 测试代表替换为 `curriculum.step`；新增真实 SQLite sealed Train+Select pool fixture 下的 `replay.run` 完整生命周期测试（Admin 异步提交即刻返回 queued、后台执行 succeeded 与 ReplayStored 字段核对、依赖边断言、幂等性、异输入同 key 冲突、角色拒绝、未知字段/版本拒绝、缺池失败、来源撤销阻断 status、进程崩溃恢复复用确定性报告等）。
4. `crates/evo-http/tests/service.rs`: 原 blocked 路由测试调整为 `curriculum.step`；新增真实 listener 的 `authenticated_async_replay_flow_and_role_rejection` 测试，覆盖未认证 401、非 Admin（Agent/Evaluator）403、认证 Admin 200 返回 queued job、后台执行至 succeeded 以及 GET `/v1/manage/jobs/{id}` 校验与非 owner 拒绝。

自测证据（全部 exit 0）：
- `cargo test --locked --offline -p evo-engine --test dispatch_management --test replay_v41`: 13 + 13 = 26 项通过。
- `cargo test --locked --offline -p evo-http --test service`: 3 项通过。
- `cargo clippy --locked --offline -p evo-engine -p evo-http -p evo-storage --all-targets -- -D warnings`: 检查通过，无 warning。
- `cargo fmt --all -- --check`: 格式化检查通过。

未完成项与边界：
- 管理接口中 `exploration.start` / `curriculum.step` / `meta.start` 仍为 `blocked_feature`；
- E03 可信执行/评分回执 schema 与真实周期更新仍待实现；
- 真实模型调用预算仍为 0，W_sim=1/2/4 仅限纯数据仿真语义，不冒充真实生产或正式验收。

### AG-002 / E16.1 来源导入与版本化读取器实施与自测

- 任务号：AG-002
- E 归属：E16.1
- 状态：`verified`（程序/持久导入子范围；主控 2026-09-30 R3 返修 F13–F16 后，依台账记录范围验证）
- base 分支：`wrokbot/ag-001-e10-replay-management`
- head 分支：`wrokbot/ag-002-e16-1-source-import`
- PR：[PR #44](https://github.com/acosmi/RSIAgent/pull/44)
- merged_sha: `acda31895bb1cb42cf7985b907a4c600429573d0`

依据与合同：严格依循 v4.1 第一真源 SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，落实 §6.3 授权历史取证、§5.6 内部记录、§13.1 E16.1 及 V005, V006, V017, V051, V052, V053, V054, V055, V056, V076, V087, V090, V098 场景族。
文件白名单修改：
1. `crates/evo-core/src/evidence.rs`: 增加 §6.3 规定的探测/读取/事件/摘录资源上限常量（`MAX_HEADER_PROBE_BYTES`、`MAX_TOTAL_READ_BYTES`、`MAX_EVENT_BYTES`、`MAX_TOTAL_EXCERPT_BYTES`）；新增 `EvidenceLocator`（绑定不可变源摘要与字节/事件范围，探测与读取间变动返回 `source_changed` 冲突）；新增 `AggregateSummary` 结构体及 `SourceCoverage` 的 `PartialEq, Eq` derive。
2. `crates/evo-engine/src/import.rs`: 完整实现三类固定格式读取器（`rsia.trace.v1`, `rsih.pi.fixture`, `claude.fixture`）；明确拒绝 `codex`（`unsupported_format:codex`）与未知格式/未知版本；实现 `tool_result_is_not_preference` 隔离非偏好；实现 `extract_cluster_id` 将跨会话同事故重试/fork聚类为同一 cluster（不膨胀独立 $n$）；实现 `ingest_imported_sources` 完整历史取证入口，落实权限检查、拒绝全 home 扫描、日志 payload 纯数据化、13 维全覆盖与截断度量。
3. `fixtures/imports/rsia.trace.v1.jsonl`: 补充 RSIA 原生版本化 trace fixture。
4. `crates/evo-engine/tests/import_v41.rs`: 新增集成测试套件，全面覆盖 V005, V006, V017, V051, V052, V053, V054, V055, V056, V076, V087, V090, V098 全部 13 项场景族。

自测证据（全部 exit 0）：
- `cargo test --locked --offline -p evo-engine --test import_v41`: 18 项全部通过（含主控审阅缺陷 F01 未知版本拒绝/不根据用户文本瞎猜、F02 UTF-8 边界安全切片截断、F03 原始字节摘录与无损往返提取等 5 项对抗回归测试）。
- `cargo test --locked --offline -p evo-engine --lib import::tests`: 3 项全部通过。
- `cargo test --locked --offline -p evo-core --lib evidence::tests`: 8 项全部通过。
- `cargo test --locked --offline -p evo-engine --test dispatch_management --test replay_v41`: 13 + 13 = 26 项回归通过。
- `cargo test --locked --offline -p evo-http --test service`: 3 项回归通过。
- `cargo clippy --locked --offline -p evo-core -p evo-engine --all-targets -- -D warnings`: 检查通过，无 warning。
- `cargo fmt --all -- --check`: 格式化检查通过。

主控审阅缺陷整改记录（PR #44）：
- **F01（未知版本/格式探测拒绝）**：重构 `detect_format_from_bytes`，采用结构化 JSON 探测检查 `schema_version`/`format`，未知版本（如 `rsia.trace.v999`、`unknown.v99`）一律报错拒绝，不再依据用户正文包含的 `"role":"user"` 臆断为 Claude；`parse_rsia_trace`/`parse_rsih_pi`/`parse_claude_fixture` 同步严格校验版本。
- **F02（UTF-8 边界安全截断）**：`detect_format_from_bytes` 与 `ingest_imported_sources` 中的长度截断全部增加 `is_char_boundary` 安全校验，防止在多字节 UTF-8 字符（如中文 `'中'`）内部切片引发 panic。
- **F03（原始字节定位器往返提取）**：在流式 JSON 解析时精准记录事件在 `raw_bytes` 中的原始字节跨度 `[byte_start..byte_end]`，`EvidenceLocator::build` 直接基于原始切片计算 `excerpt_digest`，确保 `locator.verify_and_extract(body)` 零差错准确提取原始切片。

未完成项与边界：
- E16.2–E16.6、E14、E15 仍为 planned；
- 导入数据没有可信执行证明（`UnverifiedImport`），绝不伪造 `AppliedReceipt` 或晋升为 `TrustedHost`；
- Codex 及其他未列产品明确为 unsupported，不猜测其格式；
- 真实外部模型调用预算仍为 0。



### CTRL-E16.1-R2 / 原PR #44：持久导入与必要存储的独立验收子范围

- 归属原AG-002/E16.1，仍使用 [PR #44](https://github.com/acosmi/RSIAgent/pull/44)，保持草稿未合并；`merged_sha: null`。
- 唯一真源：`plan_version: v4.1`；`plan_sha256: 45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`。固定基线 `0cd4d9e6a57e6b7a0b48386a59e54deef1fa4d77`；已测源码 `source_sha: af9888e091b8c707a2067cc0adbc864efd5187fe`；本地输入 `out/controller-final-e16-20260919/pr44-final-input.json`，SHA-256 `6340e771fb3ef7a0d375d6d1487ba5c00a8e8eeef74da72c2c0025192934421b`。提交前后全部9文件逐一重算一致。
- 实现：严格结构化/版本化读取、UTF-8原始事件locator和累计摘录界限；真实Admin SourceSelection注册、Prepared→注册blob安全发布→持久结果/证据摘要，重启读取、同key幂等与异体拒绝；内容副本/已知共同谱系不增加独立簇，生成消费者要求两个独立簇。
- 大于1MiB的合法历史源仍支持真源64MiB总读取预算；10,000事件按真实持久结果原子求和，缺失/NULL/错误类型不得当0。源2撤销阻断selection/result后继、清理实际raw blob；共享blob保留到最后活来源撤销。恢复沿原E08独立当前库锚重放artifact tombstone，不另建权威/费用账本。
- 本PR仅带import所需存储/清理/恢复原语：registered blob静态门只接受import_source/raw_blob_digest；未混入资产/seed/export消费者或PR48的run/prepare/Skill业务容量门。未删除原基线断言；其他子包新增断言保留在各自交付队列。
- 主控亲自运行99项：engine import28＋optimization7；storage全套56；core evidence8，全部exit 0；storage+engine all-target clippy `-D warnings`、workspace格式检查exit 0。统一环境 `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`，Cargo均 `--locked --offline`。
- 原始QA仅本地：`out/controller-final-e16-20260919/pr44-final-[0-4].log`，命令/退出码 `pr44-final-results.json`。未上传方案或原始QA，未执行xtask ci、Actions、付费调用、发布或main合并。
- 通过范围：上述三种固定reader的程序/持久证据消费与撤销恢复子范围。E16.1完整真实迁移链仍未关闭：ImportedHistory/UnverifiedImport不产生可信run、AppliedReceipt、FormalEvaluation或Active；实际候选生成/独立验收/新run使用仍依赖未齐的E03/E04/E05事实，管理wire扩展也未在本包冒称完成。
- 风险/回滚：仅支持冻结格式与限额，不保证全部第三方版本；关闭新导入并经独立回退PR撤回该子范围，保留已发生费用、撤销和审计事实；回滚输入为上述原PR head。

### AG-003 / E16.2 资产导入/导出、隐私门禁与不可信元数据隔离实施与自测

- 任务号：AG-003
- E 归属：E16.2
- 状态：`superseded`（被 AG-011 / PR #54 取代；PR 关闭待用户确认）
- base 分支：`wrokbot/ag-002-e16-1-source-import`
- head 分支：`wrokbot/ag-003-e16-2-asset-package`
- PR：[PR #45](https://github.com/acosmi/RSIAgent/pull/45)
- merged_sha: null（未合并）

依据与合同：严格依循 v4.1 第一真源 SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，落实 §11.1、§11.2、§11.3、§13.1 E16.2 及 V017, V039, V066, V067, V068, V069, V070, V075, V091, V092, V098 场景族。
文件白名单修改：
1. `crates/evo-engine/src/packages.rs`: 完整实现 §11.2 冻结的 `AssetPackageManifest` 规范；实现安全初值与越界前拒绝门禁（MAX_FILES=100, MAX_TOTAL=10MB, MAX_FILE=1MB, MAX_ZIP_RATIO=100，路径穿越/绝对路径/重复路径/可执行脚本/链接全面阻断）；全面增强隐私扫描器 `scan_privacy` 与 `privacy_block`（拦截私钥、API tokens、本地用户目录、内部网络IP/域名、隐藏评测答案、原始对话会话、敏感认证参数）；实现跨安装域不可信元数据隔离与 staging 工作流（`foreign_approval_is_not_local`、外部 FormalEvaluation/Approval 仅存历史声明、不赋予本地权限、不改写本地 Active、缺失依赖隔离为 quarantined）；实现三方差异与只读预览 `preview_package_diff`（改动强制新审批，绝不越权提升角色）；实现撤销感知的受控导出 `export_package`（导出途中源被撤销或水位前移立即终止失败，未完成包不成为可用包，交付审计记录保留事实但不虚构远程销毁）；实现共享依赖卸载保护 `DependencyTracker`（防止误删跨包共享依赖）。
2. `fixtures/packages/golden_manifest.json`: 冻结并输出 v4.1 §11.2 规定的包清单 golden 规范样本。
3. `crates/evo-engine/tests/packages_v41.rs`: 新增端到端集成测试套件，全面覆盖 V017, V039, V066, V067, V068, V069, V070, V075, V091, V092, V098 全部 14 项测试场景。

自测证据（全部 exit 0）：
- `cargo test --locked --offline -p evo-engine --test packages_v41`: 14 项全部通过。
- `cargo test --locked --offline -p evo-engine --lib packages::tests`: 4 项全部通过。
- `cargo test --locked --offline -p evo-engine --test import_v41`: 13 项全部通过。
- `cargo test --locked --offline -p evo-core -p evo-storage -p evo-engine`: 全量测试全部通过。
- `cargo clippy --locked --offline -p evo-core -p evo-storage -p evo-engine --all-targets -- -D warnings`: 检查通过，无 warning。
- `cargo fmt --all -- --check`: 格式化检查通过。

未完成项与边界：
- E16.3–E16.6、E14、E15 仍为 planned；
- 外部包无论携带何种外部评测与审批，导入后绝不自动生效（`is_active = false`, `is_approved = false`），必须经过本地编译、验收与正式审批；
- 隐私扫描通过记录明确免责声明（`PRIVACY_DISCLAIMER`），不声称绝对无泄漏，禁止项不可通过“忽略告警”放行；
- 撤销感知记录保留真实历史事实，不声称远程擦除第三方已下载副本。

### AG-004 / E16.3 内置种子、B/L/U三方对账与本地修改保护实施与自测

- 任务号：AG-004
- E 归属：E16.3
- 状态：`superseded`（被 AG-011 / PR #54 取代；PR 关闭待用户确认）
- base 分支：`wrokbot/ag-003-e16-2-asset-package`
- head 分支：`wrokbot/ag-004-e16-3-seed-blu`
- PR：[PR #46](https://github.com/acosmi/RSIAgent/pull/46)
- merged_sha: null（未合并）

依据与合同：严格依循 v4.1 第一真源 SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，落实 §11.1、§13.1 E16.3 及 V014, V018, V038, V063, V064, V065, V078, V091, V092, V098 场景族。
文件白名单修改：
1. `crates/evo-engine/src/seeds.rs`: 完整落实 §11.1 表格全部 B/L/U 分支分类逻辑（`Unmodified`, `LocallyEdited`, `UpstreamNewer`, `BothChanged`, `IdenticalToUpstream`, `SameNameDifferentPublisher`, `MissingMarker`, `CorruptBaseline`）；强制 `auto_activate` 对任何类别均返回失败（未修改本地副本绝不自动升级 Active，必须经过 staging、评估与审批）；新增 `SeedInstallRecord` 记录安装基线 B、本地用户副本 L、上游 U、撤销水位及隔离状态；新增三方差异计算与冲突标记 `compute_three_way_diff`（区分未改动/单侧改动/同向改动/冲突，自动生成 `<<<<<<< LOCAL ... ======= ... >>>>>>> UPSTREAM` 冲突标注并强制重新评估，`auto_activated` 恒为 false）；实现重置安全门禁 `safe_reset_to_baseline`（基线缺失/损坏拒绝、基线撤销拒绝、撤销水位前移拒绝、触发关键回归拒绝，安全重置仅生成 StagedReset，绝不直接替换 Active）；实现 `StagingSession` 中断/回滚保护（会话中止保持旧 Active 完好可用，不标记更新完成）。
2. `crates/evo-engine/tests/seeds_v41.rs`: 新增集成测试套件，全面覆盖 V014, V018, V038, V063, V064, V065, V078, V091, V092, V098 全部测试场景。

自测证据（全部 exit 0）：
- `cargo test --locked --offline -p evo-engine --test seeds_v41`: 5 项全部通过。
- `cargo test --locked --offline -p evo-engine --lib seeds::tests`: 1 项全部通过。
- `cargo test --locked --offline -p evo-engine --test packages_v41`: 14 项全部通过。
- `cargo test --locked --offline -p evo-engine`: 全量测试全部通过。
- `cargo clippy --locked --offline -p evo-engine --all-targets -- -D warnings`: 检查通过，无 warning。
- `cargo fmt --all -- --check`: 格式化检查通过。

未完成项与边界：
- E16.4–E16.6、E14、E15 仍为 planned；
- 种子或上游更新即使与基线完全同源，也绝不自动激活活跃版本，必须生成 staging 并经独立验收；
- 人工解决冲突后的合并内容作为新候选处理，重新验收，不以文字合并自动继承任何前期通过记录。

### AG-005 / E16.4 额外真实宿主、Claude Code MCP Tool-Only与配置面漂移门禁实施与自测

- 任务号：AG-005
- E 归属：E16.4
- 状态：`verified`（真实 Store 拒绝门与记录重载子范围；重新堆叠到 main，含主控 F21；E16.4 整项仍 blocked）
- base 分支：`wrokbot/ag-004-e16-3-seed-blu`
- head 分支：`wrokbot/ag-005-e16-4-extra-host`
- PR：[PR #47](https://github.com/acosmi/RSIAgent/pull/47)
- merged_sha: `1c824e6614667da4dc7ea74e96b46adfd1d2c089`

依据与合同：严格依循 v4.1 第一真源 SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，落实 §5.5、§13.1 E16.4 及 V002, V003, V009, V037, V039, V060, V061, V062, V077, V087, V091, V092, V098 场景族。
文件白名单修改：
1. `crates/evo-engine/src/hosts.rs`: 严格依循 v4.1 约束，拟议额外宿主目标固定为 Claude Code 的 MCP Tool-only 接入（`claude-code-mcp-tool-only`）；未检测到本地真实安装时绝不造假或替换为 mock 宿主，真实返回 blocked 错误（`claude_code_support`）；完整定义 Claude Code MCP Tool-Only 的 `HostSurfaceManifest`（固定四模型工具 `evo_prepare`, `evo_feedback`, `evo_propose`, `evo_inspect` 为 Supported 并对应 field_contract，内部 shell/web_search 明确标记为 Unsupported，model_selection 标记为 RuntimeOwned）；实现宿主配置面漂移检测 `detect_surface_drift`（严格检查未分类字段、空输出拒绝、非法字段映射拒绝）；实现宿主执行回执核验与防伪门禁 `verify_host_receipt`（截断/覆盖/遗漏严禁伪报为完整 `Used`，未经验收严禁伪报 `VerifiedBenefit`，落实 V087 Skill-Diagnosis-Attribution 归因分类）。
2. `fixtures/hosts/claude_code_surface.v1.json`: 冻结并输出 Claude Code MCP Tool-only 宿主配置面 golden manifest fixture。
3. `crates/evo-engine/tests/hosts_v41.rs`: 新增集成测试套件，全面覆盖 V002, V003, V009, V037, V039, V060, V061, V062, V077, V087, V091, V092, V098 全部测试场景。

自测证据（全部 exit 0）：
- `cargo test --locked --offline -p evo-engine --test hosts_v41`: 8 项全部通过。
- `cargo test --locked --offline -p evo-engine --lib hosts::tests`: 2 项全部通过。
- `cargo test --locked --offline -p evo-engine --test seeds_v41`: 5 项全部通过。
- `cargo test --locked --offline -p evo-engine`: 全量测试全部通过。
- `cargo clippy --locked --offline -p evo-engine --all-targets -- -D warnings`: 检查通过，无 warning。
- `cargo fmt --all -- --check`: 格式化检查通过。

未完成项与边界：
- E16.5、E16.6、E14、E15 仍为 planned；
- 本机未安装真实 Claude Code CLI 运行时，子包合同/漂移门禁/回执防伪已就绪，但真实宿主端到端执行如实保留 blocked 状态，不偷换为泛化 mock；
- 真实宿主 Tool-only 模式仅承诺四工具交互，不承诺宿主内部 prompt/shell/model 生效，截断或覆盖不计为收益证明。

### AG-006 / E16.5 持久恢复不可变性、MVP容量门禁与部署安全实施与自测

- 任务号：AG-006
- E 归属：E16.5
- 状态：`verified`（容量/恢复/部署门纯函数子范围；主控返修 F10–F12 后重新堆叠；未接运行入口，E16.5 仍 blocked）
- base 分支：`wrokbot/ag-005-e16-4-extra-host`
- head 分支：`wrokbot/ag-006-e16-5-capacity-recovery`
- PR：[PR #48](https://github.com/acosmi/RSIAgent/pull/48)
- merged_sha: `4980aa498baa80fd82e83441839e606859fa4065`

依据与合同：严格依循 v4.1 第一真源 SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，落实 §11.3、§11.4、§13.1 E16.5 及 V007, V008, V017, V018, V037, V038, V039, V066, V069, V073, V075, V081–V086, V089, V096, V098 场景族。
文件白名单修改：
1. `crates/evo-engine/src/capacity.rs`: 完整实现 v4.1 MVP 容量门禁（包含 runs 1000、events 10000、skills 1000、inflight_prepare 5、exploration_nodes 500、replay_worlds 100、active_leases 10、staged_packages 20、concurrent_dispatches 1），超限严格拒绝新派生，绝不静默截断（`admit_v41`）；实现持久恢复不可变性核验 `verify_recovery_state`（备份缺少撤销水位立即隔离、恢复水位落后于当前活水位拒绝、已消费 queries 与已支出费用绝对不可回退置零、早停票据恢复后恒为不可晋升、uncertain 请求绝不自动退款且不重派发）；实现部署安全门禁 `validate_deployment_security`（禁止在无沙箱状态下开启代码执行、强制要求可信撤销数据库锚点）。
2. `crates/evo-engine/tests/capacity_v41.rs`: 新增集成测试套件，全面覆盖 V007, V008, V017, V018, V037, V038, V039, V066, V069, V073, V075, V081–V086, V096, V098 全部测试场景。

自测证据（全部 exit 0）：
- `cargo test --locked --offline -p evo-engine --test capacity_v41`: 6 项全部通过。
- `cargo test --locked --offline -p evo-engine --lib capacity::tests`: 1 项全部通过。
- `cargo test --locked --offline -p evo-engine --test hosts_v41`: 8 项全部通过。
- `cargo test --locked --offline -p evo-engine`: 全量测试全部通过。
- `cargo clippy --locked --offline -p evo-engine --all-targets -- -D warnings`: 检查通过，无 warning。
- `cargo fmt --all -- --check`: 格式化检查通过。

未完成项与边界：
- E16.6、E14、E15 仍为 planned；
- 本版仅承诺 MVP 明确受限规模（runs<=1000, events<=10000, nodes<=500, worlds<=100），不作无界横向扩展性能承诺；
- 恢复演练确保历史事实、撤销水位及账单不可逆，不伪造远程擦除或自动恢复缺失的沙箱。

### AG-007 / E16.6 发行索引、唯一真源v4.1绑定与全量可追踪性交接实施与自测

- 任务号：AG-007
- E 归属：E16.6
- 状态：`verified`（派生索引结构与合并事实映射子范围；2026-09-30 重叠到 main 并更新后以 PR #56 合并）
- base 分支：`wrokbot/ag-006-e16-5-capacity-recovery`
- head 分支：`wrokbot/ag-007-e16-6-release-ledger`
- PR：[PR #49](https://github.com/acosmi/RSIAgent/pull/49)
- merged_sha: `364a0722be7d5b05cbf01c453322ba35c2fdfa67`（PR #56，取代因 base 分支删除而关闭的 #49）

依据与合同：严格依循 v4.1 第一真源 SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，落实 §1.4、§11.2、§13.1 E16.6 及 V071, V072, V073, V074, V080, V098 场景族。
文件白名单修改：
1. `reports/support-scope.json`: 显式切换唯一方案索引至 v4.1 定稿规范及其 SHA-256 哈希 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`；声明 `subset_only` 与 `not_full_route_complete: true`；登记 `legacy_equivalence_unverified: true`，明确不虚构或伪造历史 T001–T126 断言；登记五维支持范围；收录完整的 B01–B10、U01–U08、K01–K10、SO01–SO18 以及 V001–V098 全量追踪链。
2. `crates/evo-core/tests/support_scope.rs`: 更新支持范围自动化验证测试，严格断言 v4.1 方案版本、SHA-256 哈希、subset_only 声明及全部 10 项 B 承诺、8 项 U 承诺、10 项 K 借鉴承诺、18 项 SO 固定源码/材料定位、98 项 V 场景族存在且无断链（落实 V071, V072, V074, V080, V098）。

自测证据（全部 exit 0）：
- `cargo test --locked --offline -p evo-core --test support_scope`: 1 项全部通过。
- `cargo test --locked --offline -p evo-core -p evo-storage -p evo-engine`: 全量测试全部通过。
- `cargo clippy --locked --offline -p evo-core -p evo-storage -p evo-engine --all-targets -- -D warnings`: 检查通过，无 warning。
- `cargo fmt --all -- --check`: 格式化检查通过。

未完成项与边界：
- E14、E15 仍为 planned；E17、E18 为可选关闭；
- 本项目声明 subset_only，不将父任务全绿，未在真实付费生产环境中声称实际经济收益；
- 历史 v3.3/v4 在规范上已被替代，物理归档据实保留，未提供的旧测试断言保持 legacy_equivalence_unverified。

### AG-008 / E17 开发代理评分器演化默认关闭与安全拒绝门禁实施与自测

- 任务号：AG-008
- E 归属：E17
- 状态：`verified`（关闭/拒绝子范围；重新堆叠到 main，含主控 F22）
- base 分支：`wrokbot/ag-007-e16-6-release-ledger`
- head 分支：`wrokbot/ag-008-e17-scorer-rejection`
- PR：[PR #50](https://github.com/acosmi/RSIAgent/pull/50)
- merged_sha: `e94042d3f2fd761455727a6c82ad13fd3d657d03`

依据与合同：严格依循 v4.1 第一真源 SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，落实 §10、§13.2 E17 及 V040 场景族。
文件白名单修改：
1. `crates/evo-core/src/features.rs`: 严格锁定 E17 状态为默认关闭（`enable_agent_scorer_evolution(true)` 强行返回拒绝错误，最终裁决权不可由候选修改）；实现 V040 评分器演化安全拒绝门禁 `validate_e17_scorer_gates`（严禁候选试图修改 final_acceptance_grader / acceptance_policy / independent_evaluator、严禁候选自批自身成绩、严禁在缺乏外部锚定与重测的情况下跨 epoch 直接混用或平均评分）。
2. `crates/evo-core/tests/e17_rejection_v41.rs`: 新增集成测试套件，全面覆盖 V040 拒绝路径（功能开关关闭、修改最终验收器拒绝、自批自评拒绝、跨 epoch 混合分数拒绝）。

自测证据（全部 exit 0）：
- `cargo test --locked --offline -p evo-core --test e17_rejection_v41`: 4 项全部通过。
- `cargo test --locked --offline -p evo-core --lib features::tests`: 2 项全部通过。
- `cargo test --locked --offline -p evo-core`: 全量测试全部通过。
- `cargo clippy --locked --offline -p evo-core --all-targets -- -D warnings`: 检查通过，无 warning。
- `cargo fmt --all -- --check`: 格式化检查通过。

未完成项与边界：
- E18、E14、E15 仍为 planned；
- E17 为可选扩展且默认关闭，最终裁决权不向候选开放；
- 本轮仅实现并验证其安全拒绝路径，不开启真实在线代理评分器自我演化。

### AG-009 / E18 可选代码修改PR默认关闭与安全拒绝门禁实施与自测

- 任务号：AG-009
- E 归属：E18
- 状态：`verified`（关闭/拒绝子范围；重新堆叠到 main，含主控 F23）
- base 分支：`wrokbot/ag-008-e17-scorer-rejection`
- head 分支：`wrokbot/ag-009-e18-code-pr-rejection`
- PR：[PR #51](https://github.com/acosmi/RSIAgent/pull/51)
- merged_sha: `bef7bd1763a01ccb677ada4acad71a063366539c`

依据与合同：严格依循 v4.1 第一真源 SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，落实 §10、§12、§13.2 E18 及 V041 场景族。
文件白名单修改：
1. `crates/evo-core/src/features.rs`: 严格保持 E18 扩展为默认关闭（`enable_automatic_code_prs(true)` 恒定返回拒绝错误）；定义 `CodePatchProposal` 补丁提案结构与 `MAX_CODE_PATCH_DIFF_BYTES`（500KB）安全上限；实现 `is_protected_code_path` 严格识别与拦截受保护路径（包含验收、评分、评测、安全、审批、凭据、私钥、沙箱、隔离、预算、账本、Cargo.toml/Cargo.lock/build.rs/Makefile/Dockerfile、路径穿越 `..`、系统绝对路径与 Docker socket）；实现 V041 代码修改 PR 安全拒绝门禁 `validate_e18_code_pr_gates`（严格拒绝修改受保护文件、拒绝未单独安全审计的依赖或构建脚本修改、绝对禁止自动合并 `auto_merge`、绝对禁止自动部署或运行中核心替换 `auto_deploy`、强制要求在一次性隔离沙箱中构建测试、强制要求人工审批 token、超限 diff 拒绝、扩展默认关闭拒绝）。
2. `crates/evo-core/tests/e18_rejection_v41.rs`: 新增集成测试套件，全面覆盖 V041 全部 12 项场景族与拒绝路径。

自测证据（全部 exit 0）：
- `cargo test --locked --offline -p evo-core --test e18_rejection_v41`: 12 项全部通过。
- `crates/evo-core/src/features.rs`: 单元测试 3 项全部通过（含 `v041_code_pr_rejection_gates`）。
- `cargo test --locked --offline -p evo-core`: 全量测试全部通过。
- `cargo test --locked --offline -p evo-core -p evo-storage -p evo-engine -p evo-http`: 全工作空间测试全部通过。
- `cargo clippy --locked --offline -p evo-core --all-targets -- -D warnings`: 检查通过，无 warning。
- `cargo fmt --all -- --check`: 格式化检查通过。

未完成项与边界：
- E14、E15 仍为 planned（E14 需主控冻结单一机制且无源码实施；E15 真实后继实验未运行并受前置阻塞）；
- E18 作为可选扩展严格保持默认关闭，禁止自动自合并、自部署或自换核心；
- 本轮仅实现并全量验证其安全拒绝路径（V041），不开启真实自动代码 PR 派生。
